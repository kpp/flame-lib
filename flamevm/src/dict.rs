use std::collections::BTreeMap;
use std::convert::{TryFrom, TryInto};
use std::ops::Bound::{Excluded, Unbounded};

use cells::{
    Cell, CellBuilder, CellDecode, CellEncode, CellError, CellReader, CellRef, CellSlice, Trie,
};

use crate::errors::VMError;
use crate::Scalar;
use crate::Value;

/// Bound recursively owned runtime records independently of wire nesting.
/// Native wrapping is cheap, but Rust rollback cloning and destruction recurse.
const MAX_OWNED_DEPTH: usize = 2 * crate::encoding::MAX_VALUE_DEPTH;

/// Ordered map from `Scalar` keys to `Value`s.
///
/// New dictionaries use a native ordered map; no Cells are built until encoding.
/// Decoded dictionaries retain their authenticated [`Trie`] and load leaves on
/// demand. Their typed-value cache preserves live witnesses and runtime values.
///
/// Dicts are **never VM-copyable** (avoids variable gas for
/// `dup`/`getdup` and the linear-leak hazard). They
/// carry two independent sticky capability flags:
///
/// - `droppable` — true iff every value ever inserted was droppable.
///   Once a non-droppable value enters (a `Token` — portable but
///   linear), the flag stays false even if later removed, so a
///   non-empty token-bearing dict can't be silently `drop`ped. An empty
///   dict is always droppable so a fully drained container can be discarded.
/// - `portable` — true iff every value ever inserted was portable.
///   Once a non-portable value enters, the flag stays false even if
///   that value is later removed.
///
/// Owned runtime nesting is bounded separately from serialized nesting. The
/// conservative depth watermark shrinks only when the dictionary is drained.
#[derive(Clone, Debug)]
pub struct Dict {
    index: DictIndex,
    // Complete for Native; only hydrated leaves for CellBacked. Runtime-only
    // values in a backed Dict have placeholder leaves until encoding rejects them.
    values: BTreeMap<Scalar, Value>,
    // Committed by the Dict envelope, not needed by the low-level Trie.
    len: usize,
    droppable: bool,
    portable: bool,
    // Conservative high-water mark; reset when empty. Not serialized and never
    // inferred from opaque Cell graphs, which do not own decoded VM values.
    owned_depth: usize,
}

#[derive(Clone, Debug)]
enum DictIndex {
    Native,
    CellBacked(Box<Trie>),
}

impl Dict {
    /// Creates an empty dictionary. A new empty dict is vacuously
    /// droppable and portable.
    pub fn new() -> Self {
        Dict {
            index: DictIndex::Native,
            values: BTreeMap::new(),
            len: 0,
            droppable: true,
            portable: true,
            owned_depth: 1,
        }
    }

    /// Number of entries. O(1).
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns true if the dictionary has no entries. O(1).
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub(crate) fn is_native(&self) -> bool {
        matches!(self.index, DictIndex::Native)
    }

    /// Logical ordered-map lookup work, independent of allocator/tree layout.
    pub(crate) fn native_access_gas(&self) -> u64 {
        1 + (usize::BITS - self.len.leading_zeros()) as u64
    }

    fn trie(&self) -> &Trie {
        match &self.index {
            DictIndex::CellBacked(trie) => trie,
            DictIndex::Native => unreachable!("native Dict has no authenticated trie"),
        }
    }

    fn trie_mut(&mut self) -> &mut Trie {
        match &mut self.index {
            DictIndex::CellBacked(trie) => trie,
            DictIndex::Native => unreachable!("native Dict has no authenticated trie"),
        }
    }

    /// Iterates over fully hydrated entries in ascending key order.
    /// Lazy dictionaries must first use [`Self::hydrate_all`].
    pub fn entries(&self) -> impl Iterator<Item = (&Scalar, &Value)> {
        assert_eq!(
            self.values.len(),
            self.len(),
            "hydrate a lazy Dict before iterating values"
        );
        self.values.iter()
    }

    /// Prover metadata only; unlike entries(), this never loads hidden branches.
    pub(crate) fn cached_values(&self) -> impl Iterator<Item = &Value> {
        self.values.values()
    }

    /// Work for building native paths or reinstalling backed cached values.
    /// Values themselves are priced separately; untouched unloaded branches need
    /// no work. Estimating a native Dict never builds or resolves Cells.
    pub(crate) fn encoding_path_gas(&self) -> Result<u64, CellError> {
        if self.is_native() {
            let Some(first) = self.values.keys().next() else {
                return Ok(0);
            };
            let first = first.to_bytes();
            let mut differences = [0u8; 32];
            for key in self.values.keys() {
                for ((difference, byte), first) in
                    differences.iter_mut().zip(key.to_bytes()).zip(first)
                {
                    *difference |= byte ^ first;
                }
            }
            let varying_digits = differences.iter().fold(0u64, |count, byte| {
                count
                    + (0..4)
                        .filter(|shift| (byte >> (2 * shift)) & 3 != 0)
                        .count() as u64
            });
            let n = u64::try_from(self.len).map_err(|_| CellError::LimitExceeded)?;
            // Every branch consumes a varying radix-4 digit. Insertion reads
            // at most this path and creates at most two additional nodes.
            let height = n.min(varying_digits + 1);
            let references = n.min(4);
            let reference_size = 2 + 34 * (1 + self.encoding_mask().count_ones() as u64);
            // A node has a descriptor (2), header (3), <=32 label bytes, and
            // <=4 references. Charge one work unit per node and reference too.
            let node_work = 38 + references * (reference_size + 1);
            return n
                .checked_mul(2 * (height + 1) * node_work)
                .ok_or(CellError::LimitExceeded);
        }
        struct Meter(u64);
        impl CellReader for Meter {
            fn read(
                &mut self,
                id: cells::CellID,
                resident: Option<&std::sync::Arc<Cell>>,
            ) -> Result<std::sync::Arc<Cell>, CellError> {
                let cell = ().read(id, resident)?;
                // One traversal plus rebuilding/hashing the same path.
                let cost = 2 * (1 + cell.record_size() as u64 + cell.refs().len() as u64);
                self.0 = self.0.checked_add(cost).ok_or(CellError::LimitExceeded)?;
                Ok(cell)
            }
        }
        let mut meter = Meter(0);
        for key in self.values.keys() {
            self.trie()
                .get_ref(&key_path(key), &mut meter)?
                .ok_or(CellError::MalformedTrie)?;
        }
        Ok(meter.0)
    }

    /// Public reference levels propagate through ordinary Dict encoding.
    /// The bounded walk ignores private witnesses and never resolves a Cell.
    fn encoding_mask(&self) -> u16 {
        let mut pending = vec![(self, 0usize)];
        let mut mask = 0;
        while let Some((dict, depth)) = pending.pop() {
            if let DictIndex::CellBacked(trie) = &dict.index {
                if let Some(root) = trie.root() {
                    mask |= root.commitment().mask();
                }
            }
            if depth > crate::encoding::MAX_VALUE_DEPTH {
                continue;
            }
            for value in dict.values.values() {
                match value {
                    Value::Cell(cell) => mask |= cell.reference().commitment().mask(),
                    Value::Dict(child) => pending.push((child, depth + 1)),
                    _ => {}
                }
            }
        }
        mask
    }

    /// Looks up a previously hydrated value. Use [`Self::get_resolved`] for
    /// arbitrary dictionaries; unavailable data must never mean an absent key.
    pub fn get(&self, key: &Scalar) -> Option<&Value> {
        if let Some(value) = self.values.get(key) {
            return Some(value);
        }
        if self.is_native() {
            return None;
        }
        assert!(
            self.trie()
                .get_ref(&key_path(key), &mut ())
                .expect("use get_resolved for lazy Dicts")
                .is_none(),
            "use get_resolved to hydrate a Dict value"
        );
        None
    }

    /// Inserts a key-value pair. If the key already exists,
    /// replaces the value and returns the prior one. Updates both sticky
    /// capability flags from the new value.
    pub fn insert(&mut self, key: Scalar, value: Value) -> Option<Value> {
        self.insert_resolved(key, value, &mut ())
            .expect("use insert_resolved for lazy paths or runtime nesting limits")
    }

    /// Inserts a key-value pair, failing if the key is already occupied.
    /// Returns the rejected value on conflict so the caller can decide
    /// whether to discard or surface it. A rejected insertion does not
    /// update either sticky capability flag.
    #[allow(clippy::result_large_err)]
    pub fn insert_strict(&mut self, key: Scalar, value: Value) -> Result<(), Value> {
        self.insert_strict_resolved(key, value, &mut ())
            .expect("use insert_strict_resolved when Dict paths may be unloaded")
    }

    /// Removes a key and returns its value if present.
    /// Does **not** unset the sticky flags — see the struct doc-comment.
    pub fn remove(&mut self, key: &Scalar) -> Option<Value> {
        self.remove_resolved(key, &mut ())
            .expect("use remove_resolved when Dict paths may be unloaded")
    }

    /// Smallest key in the dict, or `None` if empty.
    pub fn first_key(&self) -> Option<Scalar> {
        self.first_key_resolved(&mut ())
            .expect("use first_key_resolved for unloaded Dicts")
    }

    /// Largest key in the dict, or `None` if empty.
    pub fn last_key(&self) -> Option<Scalar> {
        self.last_key_resolved(&mut ())
            .expect("use last_key_resolved for unloaded Dicts")
    }

    /// Smallest key strictly greater than `k`, or `None` if no such key
    /// exists.
    pub fn next_key_after(&self, k: &Scalar) -> Option<Scalar> {
        self.next_key_after_resolved(k, &mut ())
            .expect("use next_key_after_resolved for unloaded Dicts")
    }

    /// Loads and validates one value without copying its VM ownership.
    pub fn get_resolved<R: CellReader + ?Sized>(
        &mut self,
        key: &Scalar,
        resolver: &mut R,
    ) -> Result<Option<&Value>, CellError> {
        if self.is_native() {
            return Ok(self.values.get(key));
        }
        let cell = match self.trie().get(&key_path(key), resolver)? {
            Some(cell) => cell,
            None => return Ok(None),
        };
        // Unsupported runtime values have no public representation. A placeholder
        // is usable only alongside the authoritative in-memory typed value.
        if cell.payload().is_empty() && cell.refs().is_empty() {
            return self
                .values
                .get(key)
                .map(Some)
                .ok_or(CellError::InvalidFormat);
        }
        // ponytail: replay this bounded decode even on cache hits so logical
        // access/gas never depends on residency; replace with a nonallocating
        // encoding walk only if profiling justifies the second traversal API.
        let value = Value::from_trusted_cell(&cell, resolver)?;
        self.check_claimed_flags(&value)?;
        let owned_depth = Self::entry_owned_depth(&value)?;
        self.trie_mut()
            .insert_ref(&key_path(key), CellRef::resident(cell), resolver)?;
        self.values.entry(*key).or_insert(value);
        self.owned_depth = self.owned_depth.max(owned_depth);
        Ok(self.values.get(key))
    }

    /// Inserts atomically; traversal, decoding, or owned-depth failure returns
    /// the supplied value without transferring its ownership.
    #[allow(clippy::result_large_err)]
    pub fn insert_resolved<R: CellReader + ?Sized>(
        &mut self,
        key: Scalar,
        value: Value,
        resolver: &mut R,
    ) -> Result<Option<Value>, (CellError, Value)> {
        if let Err(error) = Self::entry_owned_depth(&value) {
            return Err((error, value));
        }
        if self.is_native() {
            let len = if self.values.contains_key(&key) {
                self.len
            } else if let Some(len) = self.len.checked_add(1) {
                len
            } else {
                return Err((CellError::MalformedTrie, value));
            };
            self.absorb_flags(&value);
            self.len = len;
            return Ok(self.values.insert(key, value));
        }
        let cell = match value.to_cell() {
            Ok(cell) => cell,
            Err(CellError::InvalidFormat | CellError::LimitExceeded) => CellBuilder::new().build(),
            Err(error) => return Err((error, value)),
        };
        let result = self
            .get_resolved(&key, resolver)
            .map(|prior| prior.is_some())
            .and_then(|replaces| {
                let len = if replaces {
                    self.len
                } else {
                    self.len.checked_add(1).ok_or(CellError::MalformedTrie)?
                };
                self.trie_mut()
                    .insert(&key_path(&key), cell, resolver)
                    .map(|_| len)
            });
        self.len = match result {
            Ok(len) => len,
            Err(error) => return Err((error, value)),
        };
        self.absorb_flags(&value);
        Ok(self.values.insert(key, value))
    }

    /// Strict insertion. An occupied key returns `Ok(Err(value))`; a loading
    /// failure returns `Err((error, value))`. Neither changes sticky flags.
    #[allow(clippy::result_large_err)]
    pub fn insert_strict_resolved<R: CellReader + ?Sized>(
        &mut self,
        key: Scalar,
        value: Value,
        resolver: &mut R,
    ) -> Result<Result<(), Value>, (CellError, Value)> {
        if self.is_native() {
            return if self.values.contains_key(&key) {
                Ok(Err(value))
            } else {
                self.insert_resolved(key, value, resolver).map(|_| Ok(()))
            };
        }
        match self.trie().get_ref(&key_path(&key), resolver) {
            Ok(Some(_)) => Ok(Err(value)),
            Ok(None) => self.insert_resolved(key, value, resolver).map(|_| Ok(())),
            Err(error) => Err((error, value)),
        }
    }

    /// Removes an owned value only after decoding and path rebuilding succeed.
    pub fn remove_resolved<R: CellReader + ?Sized>(
        &mut self,
        key: &Scalar,
        resolver: &mut R,
    ) -> Result<Option<Value>, CellError> {
        if self.get_resolved(key, resolver)?.is_none() {
            return Ok(None);
        }
        let len = self.len.checked_sub(1).ok_or(CellError::MalformedTrie)?;
        if self.is_native() {
            self.len = len;
            if len == 0 {
                self.owned_depth = 1;
            }
            return Ok(self.values.remove(key));
        }
        let mut trie = self.trie().clone();
        trie.remove(&key_path(key), resolver)?;
        if (len == 0) != trie.is_empty() {
            return Err(CellError::MalformedTrie);
        }
        *self.trie_mut() = trie;
        self.len = len;
        if len == 0 {
            self.owned_depth = 1;
        }
        Ok(self.values.remove(key))
    }

    pub fn first_key_resolved<R: CellReader + ?Sized>(
        &self,
        resolver: &mut R,
    ) -> Result<Option<Scalar>, CellError> {
        if self.is_native() {
            return Ok(self.values.keys().next().copied());
        }
        self.trie()
            .first_key(resolver)?
            .map(scalar_from_path)
            .transpose()
    }

    pub fn last_key_resolved<R: CellReader + ?Sized>(
        &self,
        resolver: &mut R,
    ) -> Result<Option<Scalar>, CellError> {
        if self.is_native() {
            return Ok(self.values.keys().next_back().copied());
        }
        self.trie()
            .last_key(resolver)?
            .map(scalar_from_path)
            .transpose()
    }

    pub fn next_key_after_resolved<R: CellReader + ?Sized>(
        &self,
        key: &Scalar,
        resolver: &mut R,
    ) -> Result<Option<Scalar>, CellError> {
        if self.is_native() {
            return Ok(self
                .values
                .range((Excluded(key), Unbounded))
                .next()
                .map(|(key, _)| *key));
        }
        self.trie()
            .next_key_after(&key_path(key), resolver)?
            .map(scalar_from_path)
            .transpose()
    }

    /// Validates an entire import, including its leaf count and scalar paths.
    /// Decoded values are installed only after all leaves pass validation.
    pub fn hydrate_all<R: CellReader + ?Sized>(
        &mut self,
        resolver: &mut R,
    ) -> Result<(), CellError> {
        self.hydrate_at(resolver, 1)
    }

    fn hydrate_at<R: CellReader + ?Sized>(
        &mut self,
        resolver: &mut R,
        depth: usize,
    ) -> Result<(), CellError> {
        if self.is_native() {
            return Ok(());
        }
        let entries = self.trie().entries_exact(self.len, resolver)?;
        let mut trie = self.trie().clone();
        let mut loaded = Vec::new();
        let mut owned_depth = self.owned_depth;
        for (path, reference) in entries {
            let key = scalar_from_path(path)?;
            if !self.values.contains_key(&key) {
                let cell = cells::read_cell(resolver, &reference)?;
                let value = crate::encoding::value_from_cell_at(&cell, resolver, depth, false)?;
                self.check_claimed_flags(&value)?;
                owned_depth = owned_depth.max(Self::entry_owned_depth(&value)?);
                trie.insert_ref(&key_path(&key), CellRef::resident(cell), resolver)?;
                loaded.push((key, value));
            }
        }
        *self.trie_mut() = trie;
        self.values.extend(loaded);
        self.owned_depth = owned_depth;
        Ok(())
    }

    /// Reads an envelope already authenticated by an owning state transition.
    /// Unlike ordinary [`CellDecode`], this does not resolve hidden branches.
    pub fn decode_trusted<R: CellReader + ?Sized>(
        slice: &mut CellSlice<'_>,
        _resolver: &mut R,
    ) -> Result<Self, CellError> {
        slice.try_load(|slice| {
            let len = usize::try_from(slice.load_u64()?).map_err(|_| CellError::LimitExceeded)?;
            let flags = slice.load_u8()?;
            if flags & !3 != 0 {
                return Err(CellError::InvalidFormat);
            }
            let trie = if len == 0 {
                Trie::new(32)?
            } else {
                Trie::from_cell(slice.load_ref()?, 32)?
            };
            Ok(Self {
                index: DictIndex::CellBacked(Box::new(trie)),
                values: BTreeMap::new(),
                len,
                portable: flags & 1 != 0,
                droppable: flags & 2 != 0,
                owned_depth: 1,
            })
        })
    }

    /// Opens a previously validated dictionary envelope without hydration.
    pub fn from_trusted_cell<R: CellReader + ?Sized>(
        cell: &Cell,
        resolver: &mut R,
    ) -> Result<Self, CellError> {
        let mut slice = CellSlice::new(cell);
        let dict = Self::decode_trusted(&mut slice, resolver)?;
        slice.finish()?;
        Ok(dict)
    }

    fn check_claimed_flags(&self, value: &Value) -> Result<(), CellError> {
        if (self.portable && !value.is_portable()) || (self.droppable && !value.is_droppable()) {
            return Err(CellError::InvalidFormat);
        }
        Ok(())
    }

    /// Dicts are never VM-copyable (avoids variable `dup`/`getdup` gas
    /// and the linear-leak hazard).
    pub fn is_copyable(&self) -> bool {
        false
    }

    /// Returns true iff this dict can be sealed into long-term storage.
    /// O(1): successful insertion of any non-portable value permanently
    /// clears the cached flag.
    pub fn is_portable(&self) -> bool {
        self.portable
    }

    /// Returns true iff this dict can be silently discarded by `drop`:
    /// it is empty, or every member ever inserted was droppable. A dict
    /// containing only `Variable` / `Constraint` / pure-computation
    /// values is droppable but not copyable.
    pub fn is_droppable(&self) -> bool {
        self.is_empty() || self.droppable
    }

    /// Logical heap work needed for a Rust-level rollback clone. Used only
    /// for gas charging; VM copyability remains governed by `try_clone`.
    pub(crate) fn clone_gas(&self) -> u64 {
        self.values
            .values()
            .fold(1 + self.values.len() as u64, |gas, value| {
                gas.saturating_add(value.clone_gas())
            })
    }

    /// VM-level clone — always fails: dicts are non-copyable.
    /// The `dup`/`getdup` family routes here, so a dict value can never
    /// be duplicated on the stack. The Rust-level `Clone` impl is the
    /// separate data-duplication path (registry snapshot / txlog entry).
    pub fn try_clone(&self) -> Result<Dict, VMError> {
        Err(VMError::TypeNotCopyable)
    }

    /// Builds a dict from a list of values with implicit keys 0, 1, 2, ...
    /// The resulting dict has sequential keys, which is the canonical input
    /// for list-style wire encoding.
    pub fn from_values(values: Vec<Value>) -> Self {
        let mut d = Dict::new();
        for (i, v) in values.into_iter().enumerate() {
            d.insert(Scalar::from(i as u64), v);
        }
        d
    }

    fn absorb_flags(&mut self, v: &Value) {
        self.droppable &= v.is_droppable();
        self.portable &= v.is_portable();
        self.owned_depth = self.owned_depth.max(1 + Self::value_owned_depth(v));
    }

    fn value_owned_depth(value: &Value) -> usize {
        match value {
            Value::Dict(dict) => dict.owned_depth,
            Value::Contract(contract) => 1 + Self::value_owned_depth(contract.payload()),
            _ => 0,
        }
    }

    fn entry_owned_depth(value: &Value) -> Result<usize, CellError> {
        let depth = 1 + Self::value_owned_depth(value);
        if depth > MAX_OWNED_DEPTH {
            return Err(CellError::LimitExceeded);
        }
        Ok(depth)
    }
}

impl CellEncode for Dict {
    fn encode(&self, builder: &mut CellBuilder) -> Result<(), CellError> {
        self.encode_at(builder, 1)
    }
}

impl Dict {
    pub(crate) fn encode_at(
        &self,
        builder: &mut CellBuilder,
        depth: usize,
    ) -> Result<(), CellError> {
        if depth > crate::encoding::MAX_VALUE_DEPTH {
            return Err(CellError::LimitExceeded);
        }
        let mut trie = match &self.index {
            DictIndex::Native => Trie::new(32)?,
            DictIndex::CellBacked(trie) => (**trie).clone(),
        };
        for (key, value) in &self.values {
            trie.insert(
                &key_path(key),
                crate::encoding::value_cell_at(value, depth)?,
                &mut (),
            )?;
        }
        builder.store_u64(u64::try_from(self.len()).map_err(|_| CellError::LimitExceeded)?)?;
        builder.store_u8(u8::from(self.portable) | (u8::from(self.droppable) << 1))?;
        if let Some(root) = trie.into_root() {
            builder.store_ref(root)?;
        }
        Ok(())
    }

    pub(crate) fn decode_at<R: CellReader + ?Sized>(
        slice: &mut CellSlice<'_>,
        resolver: &mut R,
        depth: usize,
    ) -> Result<Self, CellError> {
        if depth > crate::encoding::MAX_VALUE_DEPTH {
            return Err(CellError::LimitExceeded);
        }
        slice.try_load(|slice| {
            let mut dict = Self::decode_trusted(slice, resolver)?;
            dict.hydrate_at(resolver, depth)?;
            Ok(dict)
        })
    }
}

impl CellDecode for Dict {
    fn decode<R: CellReader + ?Sized>(
        slice: &mut CellSlice<'_>,
        resolver: &mut R,
    ) -> Result<Self, CellError> {
        Self::decode_at(slice, resolver, 1)
    }
}

/// Big-endian path representation preserves unsigned numeric Scalar order.
pub fn key_path(key: &Scalar) -> [u8; 32] {
    let mut bytes = key.to_bytes();
    bytes.reverse();
    bytes
}

fn scalar_from_path(path: Vec<u8>) -> Result<Scalar, CellError> {
    let mut bytes: [u8; 32] = path.try_into().map_err(|_| CellError::InvalidFormat)?;
    bytes.reverse();
    Scalar::from_bytes(bytes).ok_or(CellError::InvalidFormat)
}

impl Default for Dict {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CellValue, ClearToken, Merlin};
    use cells::{CellIndex, CellRef};
    use std::sync::Arc;

    #[test]
    fn native_records_never_resolve_cells_and_serialization_does_not_change_mode() {
        struct NoReads;
        impl CellReader for NoReads {
            fn read(
                &mut self,
                _: cells::CellID,
                _: Option<&Arc<Cell>>,
            ) -> Result<Arc<Cell>, CellError> {
                panic!("native Dict must not resolve Cells")
            }
        }
        let mut dict = Dict::new();
        let mut reader = NoReads;
        dict.insert_resolved(Scalar::ONE, v(7), &mut reader)
            .unwrap();
        dict.insert_strict_resolved(Scalar::ZERO, v(8), &mut reader)
            .unwrap()
            .unwrap();
        assert!(
            matches!(dict.get_resolved(&Scalar::ONE, &mut reader).unwrap(), Some(Value::Scalar(n)) if *n == Scalar::from(7u64))
        );
        assert_eq!(
            dict.first_key_resolved(&mut reader).unwrap(),
            Some(Scalar::ZERO)
        );
        assert_eq!(
            dict.last_key_resolved(&mut reader).unwrap(),
            Some(Scalar::ONE)
        );
        assert_eq!(
            dict.next_key_after_resolved(&Scalar::ZERO, &mut reader)
                .unwrap(),
            Some(Scalar::ONE)
        );
        dict.hydrate_all(&mut reader).unwrap();
        let gas = dict.encoding_path_gas().unwrap();
        let encoded = dict.to_cell().unwrap();
        assert!(dict.is_native());
        assert_eq!(dict.encoding_path_gas().unwrap(), gas);
        assert!(!Dict::from_cell(&encoded, &mut ()).unwrap().is_native());
        dict.remove_resolved(&Scalar::ONE, &mut reader)
            .unwrap()
            .unwrap();
        assert!(dict
            .remove_resolved(&Scalar::ONE, &mut reader)
            .unwrap()
            .is_none());
        dict.insert_resolved(
            Scalar::ONE,
            Value::Merlin(Merlin::new(b"runtime")),
            &mut reader,
        )
        .unwrap();
        assert!(dict.to_cell().is_err());
        assert!(dict.is_native());
    }

    #[test]
    fn native_and_backed_mutations_encode_identical_flags_and_factual_cells() {
        let pruned = Cell::from_pruned(1, vec![[7; 32]], vec![2]).unwrap();
        let mut native = Dict::from_values(vec![
            token(-1),
            Value::Cell(pruned.into()),
            Value::Dict(Dict::from_values(vec![v(8)])),
        ]);
        let encoded = native.to_cell().unwrap();
        let mut backed = Dict::from_cell(&encoded, &mut ()).unwrap();
        for dict in [&mut native, &mut backed] {
            dict.remove(&Scalar::ZERO).unwrap();
            dict.insert(Scalar::from(3u64), v(9));
        }
        assert!(!native.is_portable());
        assert!(!native.is_droppable());
        assert!(native.is_native());
        assert!(!backed.is_native());
        let left = native.to_cell().unwrap();
        let right = backed.to_cell().unwrap();
        assert_eq!(left.commitment(), right.commitment());
        assert_eq!(left.encode_record(), right.encode_record());
        assert_eq!(left.refs()[0].commitment().mask(), 1);
    }

    #[test]
    fn native_materialization_bound_covers_trie_reads_and_new_nodes() {
        use std::collections::HashSet;
        struct Meter(u64);
        impl CellReader for Meter {
            fn read(
                &mut self,
                id: cells::CellID,
                resident: Option<&Arc<Cell>>,
            ) -> Result<Arc<Cell>, CellError> {
                let cell = ().read(id, resident)?;
                self.0 += 1 + cell.record_size() as u64 + cell.refs().len() as u64;
                Ok(cell)
            }
        }
        fn collect(reference: &CellRef, ids: &mut HashSet<cells::CellID>, work: &mut u64) {
            if ids.insert(reference.id()) {
                let cell = reference.as_resident().unwrap();
                *work += 1 + cell.record_size() as u64 + cell.refs().len() as u64;
                for reference in cell.refs() {
                    collect(reference, ids, work);
                }
            }
        }
        for sparse in [false, true] {
            for n in [0u64, 1, 2, 4, 16, 64] {
                let mut dict = Dict::new();
                for i in 0..n {
                    let key = if sparse {
                        let mut bytes = [0u8; 32];
                        bytes[i as usize % 31] = i as u8 + 1;
                        Scalar::from_bytes(bytes).unwrap()
                    } else {
                        Scalar::from(i)
                    };
                    dict.insert(key, v(i));
                }
                let mut trie = Trie::new(32).unwrap();
                let mut meter = Meter(0);
                let mut ids = HashSet::new();
                let mut builds = 0;
                for (key, value) in dict.entries() {
                    let value = value.to_cell().unwrap();
                    // Value encoding is charged separately from the paths.
                    ids.insert(value.id());
                    trie.insert(&key_path(key), value, &mut meter).unwrap();
                    collect(trie.root().unwrap(), &mut ids, &mut builds);
                }
                assert!(dict.encoding_path_gas().unwrap() >= meter.0 + builds);
                assert!(dict.is_native());
            }
        }
    }

    #[test]
    fn native_nesting_is_bounded_before_clone_drop_or_linear_ownership_transfer() {
        let contract = crate::Contract::new(
            crate::Predicate::opaque(crate::Predicate::unspendable_key()),
            crate::Anchor([7; 32]),
            v(9),
        )
        .unwrap();
        let contract_id = contract.id();
        for leaf in [token(7), Value::Contract(Box::new(contract))] {
            let depth = Dict::value_owned_depth(&leaf);
            let mut nested = leaf;
            for _ in depth..MAX_OWNED_DEPTH {
                nested = Value::Dict(Dict::from_values(vec![nested]));
            }
            // Rollback snapshots and destruction stay safely bounded even for
            // records that were never encoded and contain linear resources.
            drop(nested.clone());
            for backed in [false, true] {
                let mut parent = Dict::from_values(vec![v(1)]);
                if backed {
                    parent = Dict::from_cell(&parent.to_cell().unwrap(), &mut ()).unwrap();
                }
                let before = parent.to_cell().unwrap().id();
                let (error, returned) = parent
                    .insert_resolved(Scalar::ONE, nested.clone(), &mut ())
                    .unwrap_err();
                assert_eq!(error, CellError::LimitExceeded);
                assert_eq!(parent.len(), 1);
                assert!(parent.is_portable());
                assert!(parent.is_droppable());
                assert_eq!(parent.to_cell().unwrap().id(), before);
                let mut value = returned;
                loop {
                    match value {
                        Value::Dict(mut dict) => value = dict.remove(&Scalar::ZERO).unwrap(),
                        Value::ClearToken(token) => {
                            assert_eq!(token.qty(), Scalar::from(7u64));
                            break;
                        }
                        Value::Contract(contract) => {
                            assert_eq!(contract.id(), contract_id);
                            break;
                        }
                        _ => panic!("linear leaf was lost"),
                    }
                }
            }
            drop(nested);
        }
    }

    #[test]
    fn owned_depth_watermark_resets_only_when_drained() {
        let mut child = Value::Scalar(Scalar::ZERO);
        for _ in 0..MAX_OWNED_DEPTH - 1 {
            child = Value::Dict(Dict::from_values(vec![child]));
        }
        let mut dict = Dict::from_values(vec![child, v(1)]);
        assert_eq!(dict.owned_depth, MAX_OWNED_DEPTH);
        dict.remove(&Scalar::ZERO).unwrap();
        assert_eq!(dict.owned_depth, MAX_OWNED_DEPTH);
        dict.remove(&Scalar::ONE).unwrap();
        assert_eq!(dict.owned_depth, 1);
        let outer = Dict::from_values(vec![Value::Dict(dict)]);
        assert_eq!(outer.owned_depth, 2);
    }

    #[test]
    fn decoded_depth_accounts_only_for_owned_typed_values() {
        let native = Dict::from_values(vec![Value::Dict(Dict::from_values(vec![v(1)]))]);
        assert_eq!(native.owned_depth, 2);
        let cell = native.to_cell().unwrap();
        let mut lazy = Dict::from_trusted_cell(&cell, &mut ()).unwrap();
        assert_eq!(lazy.owned_depth, 1);
        lazy.get_resolved(&Scalar::ZERO, &mut ()).unwrap();
        assert_eq!(lazy.owned_depth, 2);
        assert_eq!(Dict::from_cell(&cell, &mut ()).unwrap().owned_depth, 2);
    }

    #[test]
    fn encoding_path_gas_tracks_cached_paths_without_serializing_values() {
        let small = Dict::from_values(vec![v(1)]);
        let large = Dict::from_values((0..64).map(v).collect());
        assert_eq!(Dict::new().encoding_path_gas().unwrap(), 0);
        assert!(small.encoding_path_gas().unwrap() > 0);
        assert!(large.encoding_path_gas().unwrap() > small.encoding_path_gas().unwrap());

        let mut runtime = Dict::new();
        runtime.insert(Scalar::ZERO, Value::Merlin(Merlin::new(b"runtime")));
        assert!(runtime.to_cell().is_err());
        assert_eq!(
            runtime.encoding_path_gas().unwrap(),
            small.encoding_path_gas().unwrap()
        );

        let cell = small.to_cell().unwrap();
        let lazy = Dict::from_trusted_cell(&cell, &mut ()).unwrap();
        assert_eq!(lazy.encoding_path_gas().unwrap(), 0);
    }

    #[test]
    fn cached_and_unloaded_values_have_identical_logical_access_costs() {
        struct Meter {
            cells: CellIndex,
            gas: u64,
        }
        impl CellReader for Meter {
            fn read(
                &mut self,
                id: cells::CellID,
                resident: Option<&Arc<Cell>>,
            ) -> Result<Arc<Cell>, CellError> {
                let cell = self.cells.read(id, resident)?;
                self.gas += 1 + cell.record_size() as u64 + cell.refs().len() as u64;
                Ok(cell)
            }
        }
        let mut original = Dict::new();
        original.insert(
            Scalar::ONE,
            Value::Cell(CellValue::from_bytes(vec![9; cells::MAX_CELL_PAYLOAD]).unwrap()),
        );
        original.insert(Scalar::from(2u64), v(2));
        let root = Arc::new(original.to_cell().unwrap());
        let mut original = Dict::from_trusted_cell(&root, &mut ()).unwrap();
        let bag = CellIndex::collect(root.clone()).unwrap();
        let detached = bag.get(&root.id()).unwrap();
        let mut slice = CellSlice::new(&detached);
        let mut unloaded = Dict::decode_trusted(&mut slice, &mut ()).unwrap();
        slice.finish().unwrap();
        let mut costs = Vec::new();
        for dictionary in [&mut original, &mut unloaded] {
            for _ in 0..2 {
                let mut meter = Meter {
                    cells: bag.clone(),
                    gas: 0,
                };
                let Some(Value::Cell(value)) =
                    dictionary.get_resolved(&Scalar::ONE, &mut meter).unwrap()
                else {
                    panic!("expected Cell");
                };
                assert_eq!(
                    cells::read_cell(&mut meter, &value)
                        .unwrap()
                        .payload()
                        .len(),
                    cells::MAX_CELL_PAYLOAD
                );
                costs.push(meter.gas);
            }
        }
        assert!(costs.iter().all(|cost| *cost == costs[0]));
        assert!(costs[0] >= cells::MAX_CELL_PAYLOAD as u64);
    }

    fn v(n: u64) -> Value {
        Value::Scalar(Scalar::from(n))
    }

    #[test]
    fn empty_dict() {
        let d = Dict::new();
        assert_eq!(d.len(), 0);
        assert!(d.is_empty());
        assert!(d.is_portable());
        assert!(d.get(&Scalar::from(0u64)).is_none());
    }

    #[test]
    fn insert_basic() {
        let mut d = Dict::new();
        assert!(d.insert(Scalar::from(5u64), v(50)).is_none());
        assert!(d.insert(Scalar::from(1u64), v(10)).is_none());
        assert!(d.insert(Scalar::from(3u64), v(30)).is_none());
        let keys: Vec<_> = d.entries().map(|(k, _)| *k).collect();
        assert_eq!(
            keys,
            vec![Scalar::from(1u64), Scalar::from(3u64), Scalar::from(5u64)]
        );
    }

    #[test]
    fn insert_existing_replaces_and_returns_old() {
        let mut d = Dict::new();
        assert!(d.insert(Scalar::from(2u64), v(100)).is_none());
        let prior = d.insert(Scalar::from(2u64), v(200));
        match prior {
            Some(Value::Scalar(i)) => assert_eq!(i, Scalar::from(100u64)),
            _ => panic!("expected old Scalar"),
        }
        match d.get(&Scalar::from(2u64)) {
            Some(Value::Scalar(i)) => assert_eq!(*i, Scalar::from(200u64)),
            _ => panic!("expected Scalar"),
        }
        assert_eq!(d.len(), 1);
    }

    #[test]
    fn get_present_and_absent() {
        let mut d = Dict::new();
        d.insert(Scalar::from(1u64), v(10));
        d.insert(
            Scalar::from(99u64),
            Value::Cell(CellValue::from_bytes(b"hi".to_vec()).unwrap()),
        );
        assert!(d.get(&Scalar::from(50u64)).is_none());
        match d.get(&Scalar::from(99u64)) {
            Some(Value::Cell(cell)) => assert_eq!(cell.payload().unwrap(), b"hi"),
            _ => panic!("expected Cell"),
        }
    }

    #[test]
    fn remove_present_and_absent() {
        let mut d = Dict::new();
        d.insert(Scalar::from(1u64), v(10));
        d.insert(Scalar::from(2u64), v(20));
        let removed = d.remove(&Scalar::from(1u64));
        assert!(matches!(removed, Some(Value::Scalar(_))));
        assert_eq!(d.len(), 1);
        assert!(d.remove(&Scalar::from(1u64)).is_none());
        assert!(d.remove(&Scalar::from(42u64)).is_none());
    }

    #[test]
    fn scalar_keys_use_canonical_unsigned_order() {
        let mut d = Dict::new();
        d.insert(Scalar::from(2i64), v(2));
        d.insert(Scalar::from(-3i64), v(30));
        d.insert(Scalar::from(0i64), v(0));
        d.insert(Scalar::from(-1i64), v(10));
        let keys: Vec<_> = d.entries().map(|(k, _)| *k).collect();
        assert_eq!(
            keys,
            vec![
                Scalar::from(0i64),
                Scalar::from(2i64),
                Scalar::from(-3i64),
                Scalar::from(-1i64),
            ]
        );
    }

    #[test]
    fn from_values_produces_sequential_keys() {
        let d = Dict::from_values(vec![v(10), v(20), v(30)]);
        assert_eq!(d.len(), 3);
        assert!(d.is_portable());
        let keys: Vec<_> = d.entries().map(|(k, _)| *k).collect();
        assert_eq!(
            keys,
            vec![Scalar::from(0u64), Scalar::from(1u64), Scalar::from(2u64)]
        );
    }

    #[test]
    fn portability_is_sticky() {
        let mut d = Dict::new();
        d.insert(Scalar::ZERO, Value::Merlin(Merlin::new(b"test")));
        assert!(!d.is_portable());

        d.insert(Scalar::ZERO, v(1));
        assert!(!d.is_portable());
        d.remove(&Scalar::ZERO);
        assert!(!d.is_portable());
    }

    #[test]
    fn rejected_insert_does_not_clear_portability() {
        let mut d = Dict::new();
        d.insert(Scalar::ZERO, v(1));
        assert!(d
            .insert_strict(Scalar::ZERO, Value::Merlin(Merlin::new(b"test")))
            .is_err());
        assert!(d.is_portable());
    }

    #[test]
    fn constructors_absorb_nested_portability() {
        let child = Dict::from_values(vec![Value::Merlin(Merlin::new(b"child"))]);
        let parent = Dict::from_values(vec![Value::Dict(child)]);
        assert!(!parent.is_portable());

        let mut explicit = Dict::new();
        explicit.insert(Scalar::from(7u64), Value::Merlin(Merlin::new(b"explicit")));
        assert!(!explicit.is_portable());
    }

    fn token(qty: i64) -> Value {
        Value::ClearToken(ClearToken::new(Scalar::from(qty), Scalar::ONE))
    }

    #[test]
    fn cell_round_trip_keeps_sticky_flags_even_when_empty() {
        let mut original = Dict::from_values(vec![token(-1)]);
        original.remove(&Scalar::ZERO);
        assert!(original.is_empty());
        assert!(original.is_droppable());
        let encoded = original.to_cell().unwrap();
        let mut restored = Dict::from_cell(&encoded, &mut ()).unwrap();
        assert!(!restored.is_portable());
        assert!(restored.is_droppable());
        restored.insert(Scalar::ONE, v(7));
        assert!(!restored.is_droppable());
        assert!(!restored.is_portable());
    }

    #[test]
    fn untrusted_import_rejects_forged_flags_counts_and_scalar_keys() {
        let original = Dict::from_values(vec![token(-1)]).to_cell().unwrap();
        let mut payload = original.payload().to_vec();
        payload[8] |= 1;
        let forged = Cell::new(payload, original.refs().to_vec()).unwrap();
        assert!(matches!(
            Dict::from_cell(&forged, &mut ()),
            Err(CellError::InvalidFormat)
        ));

        let mut payload = original.payload().to_vec();
        payload[..8].copy_from_slice(&2u64.to_le_bytes());
        let forged = Cell::new(payload, original.refs().to_vec()).unwrap();
        assert!(matches!(
            Dict::from_cell(&forged, &mut ()),
            Err(CellError::MalformedTrie)
        ));

        let mut trie = Trie::new(32).unwrap();
        trie.insert(&[255; 32], v(1).to_cell().unwrap(), &mut ())
            .unwrap();
        let mut builder = CellBuilder::new();
        builder
            .store_u64(1)
            .unwrap()
            .store_u8(3)
            .unwrap()
            .store_ref(trie.into_root().unwrap())
            .unwrap();
        let forged = builder.build();
        assert!(matches!(
            Dict::from_cell(&forged, &mut ()),
            Err(CellError::InvalidFormat)
        ));
        let lazy = Dict::from_trusted_cell(&forged, &mut ()).unwrap();
        assert!(matches!(
            lazy.first_key_resolved(&mut ()),
            Err(CellError::InvalidFormat)
        ));
    }

    #[test]
    fn invalid_counts_cannot_overflow_or_lose_a_linear_value() {
        let original = Dict::from_values(vec![token(9)]).to_cell().unwrap();
        for count in [0usize, 2, usize::MAX] {
            let mut dict = Dict::from_trusted_cell(&original, &mut ()).unwrap();
            // Exercise defensive mutation checks even if prior admission was wrong.
            dict.len = count;
            let before = dict.trie().root_id();
            if count == usize::MAX {
                let (error, returned) = dict
                    .insert_resolved(Scalar::ONE, token(7), &mut ())
                    .unwrap_err();
                assert_eq!(error, CellError::MalformedTrie);
                assert!(matches!(returned, Value::ClearToken(t) if t.qty() == Scalar::from(7u64)));
                assert!(dict.get_resolved(&Scalar::ONE, &mut ()).unwrap().is_none());
            }
            assert!(matches!(
                dict.remove_resolved(&Scalar::ZERO, &mut ()),
                Err(CellError::MalformedTrie)
            ));
            assert_eq!(dict.trie().root_id(), before);
            assert_eq!(dict.len(), count);
            assert!(matches!(
                dict.get_resolved(&Scalar::ZERO, &mut ()).unwrap(),
                Some(Value::ClearToken(t)) if t.qty() == Scalar::from(9u64)
            ));
        }
    }

    #[test]
    fn lazy_access_and_mutation_preserve_unloaded_siblings() {
        let mut original = Dict::new();
        for key in [0u64, 255, 256, u64::MAX] {
            original.insert(Scalar::from(key), v(key));
        }
        let full = original.to_cell().unwrap();
        let mut bag = CellIndex::collect(Arc::new(full.clone())).unwrap();
        let unloaded = Cell::decode_record_exact(&full.encode_record()).unwrap();
        let mut lazy = Dict::from_trusted_cell(&unloaded, &mut ()).unwrap();
        assert!(lazy.values.is_empty());
        assert_eq!(lazy.to_cell().unwrap().id(), full.id());
        assert!(matches!(
            lazy.get_resolved(&Scalar::ZERO, &mut ()),
            Err(CellError::MissingCell(_))
        ));
        assert!(
            matches!(lazy.get_resolved(&Scalar::ZERO, &mut bag).unwrap(), Some(Value::Scalar(n)) if *n == Scalar::ZERO)
        );
        assert_eq!(lazy.values.len(), 1);
        assert_eq!(lazy.to_cell().unwrap().id(), full.id());
        lazy.insert_resolved(Scalar::from(256u64), v(77), &mut bag)
            .unwrap();
        original.insert(Scalar::from(256u64), v(77));
        assert_eq!(lazy.values.len(), 2);
        assert_eq!(
            lazy.to_cell().unwrap().id(),
            original.to_cell().unwrap().id()
        );
    }

    #[test]
    fn failed_collapse_keeps_linear_value_and_retry_cannot_duplicate_it() {
        let mut original = Dict::new();
        original.insert(Scalar::ZERO, token(9));
        original.insert(Scalar::from(256u64), v(7));
        let full = original.to_cell().unwrap();
        let mut bag = CellIndex::collect(Arc::new(full.clone())).unwrap();
        let root = full.refs()[0].as_resident().unwrap();
        let mut refs = root.refs().to_vec();
        refs[1] = refs[1].to_unloaded();
        let partial_root = Cell::new(root.payload().to_vec(), refs).unwrap();
        let partial = Cell::new(
            full.payload().to_vec(),
            vec![CellRef::resident(partial_root)],
        )
        .unwrap();
        let mut dict = Dict::from_trusted_cell(&partial, &mut ()).unwrap();
        assert!(matches!(
            dict.remove_resolved(&Scalar::ZERO, &mut ()),
            Err(CellError::MissingCell(_))
        ));
        assert_eq!(dict.len(), 2);
        assert!(
            matches!(dict.get(&Scalar::ZERO), Some(Value::ClearToken(t)) if t.qty() == Scalar::from(9u64))
        );
        assert_eq!(dict.to_cell().unwrap().id(), full.id());
        assert!(matches!(
            dict.remove_resolved(&Scalar::ZERO, &mut bag).unwrap(),
            Some(Value::ClearToken(_))
        ));
        assert!(dict
            .remove_resolved(&Scalar::ZERO, &mut bag)
            .unwrap()
            .is_none());
    }

    #[test]
    fn failed_insertion_returns_supplied_debt_without_changing_dictionary() {
        let full = Dict::from_values(vec![v(1)]).to_cell().unwrap();
        let unloaded = Cell::decode_record_exact(&full.encode_record()).unwrap();
        let mut dict = Dict::from_trusted_cell(&unloaded, &mut ()).unwrap();
        let (error, returned) = dict
            .insert_resolved(Scalar::ONE, token(-7), &mut ())
            .unwrap_err();
        assert!(matches!(error, CellError::MissingCell(_)));
        assert!(matches!(returned, Value::ClearToken(t) if t.qty() == Scalar::from(-7i64)));
        assert_eq!(dict.len(), 1);
        assert!(dict.is_portable());
        assert_eq!(dict.to_cell().unwrap().id(), full.id());
    }
}
