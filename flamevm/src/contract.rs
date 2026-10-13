//! Linear Contracts and Cell-backed Taproot predicates.

use bulletproofs::PedersenGens;
use cells::{
    CellBuilder, CellDecode, CellEncode, CellError, CellID, CellReader, CellRef, CellSlice,
};
use core::{any::Any, fmt};
use curve25519_dalek::constants::RISTRETTO_BASEPOINT_TABLE;
use curve25519_dalek::ristretto::CompressedRistretto;
use curve25519_dalek::scalar::Scalar;
use merlin::Transcript;

use crate::{errors::VMError, vm::Anchor, ScriptBuilder, Value};

/// The Cell ID of a Contract output: predicate, anchor, and one payload Value.
pub type ContractID = CellID;

/// Prover-side metadata; never part of a Predicate's 32-byte public encoding.
pub trait PredicateWitness: Any + Send + Sync + fmt::Debug {
    fn to_point(&self) -> CompressedRistretto;
    fn clone_witness(&self) -> Box<dyn PredicateWitness>;
    fn as_any(&self) -> &dyn Any;
}

/// Unlock condition P = X + H(X, program Cell hash(0)) · B.
pub struct Predicate {
    pub(crate) point: CompressedRistretto,
    pub(crate) witness: Option<Box<dyn PredicateWitness>>,
}

impl Clone for Predicate {
    fn clone(&self) -> Self {
        Self {
            point: self.point,
            witness: self.witness.as_ref().map(|w| w.clone_witness()),
        }
    }
}

impl fmt::Debug for Predicate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Predicate").field(&self.point).finish()
    }
}

/// Optional host-side builder for blinded executable policy trees.
/// Branch selection is ordinary bytecode, not part of the `open` opcode.
#[derive(Clone, Debug)]
pub struct PredicateTree {
    pub(crate) internal_key: CompressedRistretto,
    programs: Vec<cells::Cell>,
    root: CellRef,
    pub(crate) point: CompressedRistretto,
    scripts: Vec<ScriptBuilder>,
}

impl PredicateWitness for PredicateTree {
    fn to_point(&self) -> CompressedRistretto {
        self.point
    }
    fn clone_witness(&self) -> Box<dyn PredicateWitness> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl Predicate {
    pub fn opaque(point: CompressedRistretto) -> Self {
        Self {
            point,
            witness: None,
        }
    }

    pub fn with_witness<W: PredicateWitness>(witness: W) -> Self {
        Self {
            point: witness.to_point(),
            witness: Some(Box::new(witness)),
        }
    }

    pub fn tree(tree: PredicateTree) -> Self {
        Self::with_witness(tree)
    }
    pub fn to_point(&self) -> CompressedRistretto {
        self.point
    }
    pub fn to_opaque(&self) -> Self {
        Self::opaque(self.point)
    }

    pub fn witness_as<W: PredicateWitness>(&self) -> Option<&W> {
        self.witness.as_ref()?.as_any().downcast_ref::<W>()
    }

    pub fn verification_key(&self) -> CompressedRistretto {
        self.point
    }

    /// Unknown discrete log, disabling the key-path spend.
    pub fn unspendable_key() -> CompressedRistretto {
        PedersenGens::default().B_blinding.compress()
    }

    /// Creates a predicate directly from any policy Cell's level-zero commitment.
    pub fn taproot(
        internal_key: CompressedRistretto,
        program: &cells::Cell,
    ) -> Result<Self, VMError> {
        let internal = internal_key.decompress().ok_or(VMError::InvalidPoint)?;
        let tweak = taproot_tweak(&internal_key, &program.hash(0)?);
        Ok(Self::opaque(
            (internal + RISTRETTO_BASEPOINT_TABLE * &tweak).compress(),
        ))
    }

    /// Authentication does not execute code or turn pruning records into bodies.
    pub fn verify_taproot(
        &self,
        internal_key: &CompressedRistretto,
        program: &cells::Cell,
    ) -> Result<(), VMError> {
        let predicate = Self::taproot(*internal_key, program).map_err(|error| match error {
            VMError::InvalidPoint => VMError::TaprootProofMismatch,
            other => other,
        })?;
        if predicate.point != self.point {
            return Err(VMError::TaprootProofMismatch);
        }
        Ok(())
    }
}

impl CellEncode for Predicate {
    fn encode(&self, builder: &mut CellBuilder) -> Result<(), CellError> {
        builder.store_bytes(self.point.as_bytes())?;
        Ok(())
    }
}

impl CellDecode for Predicate {
    fn decode<R: CellReader + ?Sized>(
        slice: &mut CellSlice<'_>,
        cells: &mut R,
    ) -> Result<Self, CellError> {
        Ok(Self::opaque(CompressedRistretto(<[u8; 32]>::decode(
            slice, cells,
        )?)))
    }
}

impl PredicateTree {
    pub fn new(
        internal_key: Option<CompressedRistretto>,
        programs: Vec<Vec<u8>>,
        blinding_key: [u8; 32],
    ) -> Result<Self, VMError> {
        let programs = programs
            .iter()
            .map(|program| crate::script::script_cell(program))
            .collect::<Result<Vec<_>, _>>()?;
        Self::from_cells(internal_key, programs, blinding_key)
    }

    pub fn from_cells(
        internal_key: Option<CompressedRistretto>,
        programs: Vec<cells::Cell>,
        blinding_key: [u8; 32],
    ) -> Result<Self, VMError> {
        if programs.is_empty() {
            return Err(VMError::EmptyPredicateTree);
        }
        let internal_key = internal_key.unwrap_or_else(Predicate::unspendable_key);
        let mut transcript = Transcript::new(b"flamevm.taproot.blinding");
        transcript.append_message(b"key", &blinding_key);
        for program in &programs {
            transcript.append_message(b"prog", &program.hash(0)?);
        }
        let mut leaves = Vec::with_capacity(programs.len());
        for program in &programs {
            let mut salt = [0; 32];
            transcript.challenge_bytes(b"salt", &mut salt);
            leaves.push(
                ScriptBuilder::new()
                    .push_cell(cells::Cell::new(salt.to_vec(), vec![])?)
                    .drop_()
                    .push_cell(program.clone())
                    .exec()
                    .to_cell()?,
            );
        }
        let root = dispatch_tree(&leaves)?;
        let point = Predicate::taproot(internal_key, &root)?.point;
        Ok(Self {
            internal_key,
            programs,
            root: root.into(),
            point,
            scripts: Vec::new(),
        })
    }

    pub fn from_scripts(
        internal_key: Option<CompressedRistretto>,
        programs: Vec<ScriptBuilder>,
        blinding_key: [u8; 32],
    ) -> Result<Self, VMError> {
        let public_programs = programs
            .iter()
            .map(ScriptBuilder::to_cell)
            .collect::<Result<Vec<_>, _>>()?;
        let mut tree = Self::from_cells(internal_key, public_programs, blinding_key)?;
        tree.scripts = programs;
        Ok(tree)
    }

    pub(crate) fn script_witness(&self, program_index: usize) -> Option<&ScriptBuilder> {
        self.scripts.get(program_index)
    }
    pub fn scripts_only(programs: Vec<Vec<u8>>, blinding_key: [u8; 32]) -> Result<Self, VMError> {
        Self::new(None, programs, blinding_key)
    }
    pub fn internal_key(&self) -> &CompressedRistretto {
        &self.internal_key
    }
    pub fn programs(&self) -> impl Iterator<Item = &cells::Cell> {
        self.programs.iter()
    }
    pub fn root(&self) -> &CellRef {
        &self.root
    }
    pub fn root_id(&self) -> CellID {
        self.root.id()
    }

    /// Supply one branch; the policy's dispatcher chooses the first unpruned child.
    pub fn opening_for(&self, program_index: usize) -> Result<TaprootOpening, VMError> {
        if program_index >= self.programs.len() {
            return Err(VMError::ProgramIndexOutOfRange);
        }
        let root = self.root.as_resident().expect("builder owns its policy");
        let root = select_branch(
            root,
            self.programs.len(),
            program_index,
            root.level().max(1),
        )?;
        debug_assert_eq!(root.hash(0)?, self.root.commitment().hash(0)?);
        Ok(TaprootOpening {
            internal_key: self.internal_key,
            root,
        })
    }
}

/// Host-side opening data; the VM receives the Point and Cell separately.
#[derive(Clone, Debug)]
pub struct TaprootOpening {
    pub internal_key: CompressedRistretto,
    pub root: cells::Cell,
}

fn dispatch_tree(leaves: &[cells::Cell]) -> Result<cells::Cell, CellError> {
    if leaves.len() == 1 {
        return Ok(leaves[0].clone());
    }
    let width = leaves.len().div_ceil(cells::MAX_CELL_REFS);
    let children = leaves
        .chunks(width)
        .map(dispatch_tree)
        .collect::<Result<Vec<_>, _>>()?;
    let mut code = ScriptBuilder::new().run_slice();
    for _ in &children {
        code = code.push_int(0u64).load_ref().cell_info();
        for _ in 0..4 {
            code = code.roll_k(1).drop_();
        }
        code = code.not().jumpif(0).drop_();
    }
    code = code
        .drop_()
        .push_int(0u64)
        .verify()
        .label(0)
        .roll_k(1)
        .drop_()
        .exec();
    cells::Cell::new(
        code.to_cell()?.payload().to_vec(),
        children.into_iter().map(CellRef::from).collect(),
    )
}

fn select_branch(
    root: &cells::Cell,
    count: usize,
    index: usize,
    level: u8,
) -> Result<cells::Cell, CellError> {
    if count == 1 {
        return Ok(root.clone());
    }
    let width = count.div_ceil(cells::MAX_CELL_REFS);
    let selected = index / width;
    let mut refs = Vec::with_capacity(root.refs().len());
    for (slot, reference) in root.refs().iter().enumerate() {
        let child = reference.as_resident().expect("builder owns its branches");
        let child = if slot == selected {
            select_branch(
                child,
                (count - slot * width).min(width),
                index % width,
                level,
            )?
        } else {
            child.prune(level)?
        };
        refs.push(child.into());
    }
    cells::Cell::new(root.payload().to_vec(), refs)
}

/// A linear Contract protecting a single portable payload Value.
#[derive(Clone, Debug)]
pub struct Contract {
    pub predicate: Predicate,
    pub anchor: Anchor,
    payload: Value,
}

impl Contract {
    /// Portability is enforced on entry into the Contract domain, not by codecs.
    pub fn new(predicate: Predicate, anchor: Anchor, payload: Value) -> Result<Self, VMError> {
        if !payload.is_portable() {
            return Err(VMError::NonPortableInOutput);
        }
        let contract = Self {
            predicate,
            anchor,
            payload,
        };
        // Admission is fallible even for portable values: excessive nesting or
        // payload length must never reach an infallible identity computation.
        contract.to_cell()?;
        Ok(contract)
    }
    pub fn payload(&self) -> &Value {
        &self.payload
    }
    pub fn into_payload(self) -> Value {
        self.payload
    }

    /// Restores a previously admitted Contract without scanning hidden Dicts.
    /// "Trusted" refers to the Dict counts/capability summaries checked when the
    /// output was created, not to its sender or a hash alone. The caller must
    /// bind this body to an accepted input commitment (including chain membership
    /// validation). Unlike ordinary `CellDecode`, hidden branches remain unloaded;
    /// accessed nodes and values still undergo their normal decoding checks.
    pub fn from_trusted_cell<R: CellReader + ?Sized>(
        cell: &cells::Cell,
        resolver: &mut R,
    ) -> Result<Self, CellError> {
        let mut slice = CellSlice::new(cell);
        let predicate = Predicate::decode(&mut slice, resolver)?;
        let anchor = Anchor(<[u8; 32]>::decode(&mut slice, resolver)?);
        let payload = Value::decode_trusted(&mut slice, resolver)?;
        slice.finish()?;
        Self::new(predicate, anchor, payload).map_err(|_| CellError::InvalidFormat)
    }
    pub(crate) fn clone_gas(&self) -> u64 {
        1u64.saturating_add(self.payload.clone_gas())
    }

    /// Plain SHA256 identity of the canonical output Cell record.
    pub fn id(&self) -> ContractID {
        self.to_cell()
            .expect("portable Contract payload has a Cell encoding")
            .id()
    }
}

/// Output encoding only: a Contract is not itself a portable VM Value.
impl CellEncode for Contract {
    fn encode(&self, builder: &mut CellBuilder) -> Result<(), CellError> {
        builder
            .store(&self.predicate)?
            .store_bytes(&self.anchor.0)?
            .store(&self.payload)?;
        Ok(())
    }
}

impl CellDecode for Contract {
    fn decode<R: CellReader + ?Sized>(
        slice: &mut CellSlice<'_>,
        cells: &mut R,
    ) -> Result<Self, CellError> {
        let predicate = Predicate::decode(slice, cells)?;
        let anchor = Anchor(<[u8; 32]>::decode(slice, cells)?);
        let payload = Value::decode(slice, cells)?;
        Self::new(predicate, anchor, payload).map_err(|_| CellError::InvalidFormat)
    }
}

/// Merlin is retained for proof binding, not Cell content addressing.
fn taproot_tweak(internal_key: &CompressedRistretto, root: &CellID) -> Scalar {
    let mut transcript = Transcript::new(b"flamevm.taproot");
    transcript.append_message(b"key", internal_key.as_bytes());
    transcript.append_message(b"root", root);
    let mut bytes = [0; 64];
    transcript.challenge_bytes(b"h", &mut bytes);
    Scalar::from_bytes_mod_order_wide(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cells::CellIndex;
    use std::sync::Arc;

    #[test]
    fn constructor_rejects_unencodable_depth_before_identity_is_used() {
        let mut value = Value::Scalar(crate::Scalar::ONE);
        for _ in 0..=crate::encoding::MAX_VALUE_DEPTH {
            value = Value::Dict(crate::Dict::from_values(vec![value]));
        }
        assert!(value.is_portable());
        assert!(matches!(
            Contract::new(
                Predicate::opaque(Predicate::unspendable_key()),
                Anchor([0; 32]),
                value
            ),
            Err(VMError::Cell(CellError::LimitExceeded))
        ));
    }

    #[test]
    fn contract_id_is_its_output_cell_id_and_payload_is_one_value() {
        let contract = Contract::new(
            Predicate::opaque(Predicate::unspendable_key()),
            Anchor([9; 32]),
            Value::Scalar(crate::Scalar::from(7u64)),
        )
        .unwrap();
        let cell = contract.to_cell().unwrap();
        assert_eq!(
            &cell.payload()[..32],
            contract.predicate.to_point().as_bytes()
        );
        assert_eq!(&cell.payload()[32..64], &[9; 32]);
        assert_eq!(contract.id(), cell.id());
        let decoded = Contract::from_cell(&cell, &mut ()).unwrap();
        assert_eq!(decoded.id(), contract.id());
        assert!(
            matches!(decoded.payload(), Value::Scalar(value) if *value == crate::Scalar::from(7u64))
        );
        assert!(matches!(
            Contract::new(
                Predicate::opaque(Predicate::unspendable_key()),
                Anchor([0; 32]),
                Value::Contract(Box::new(contract)),
            ),
            Err(VMError::NonPortableInOutput)
        ));
    }

    #[test]
    fn branch_opening_preserves_semantic_hash_without_revealing_other_programs() {
        let programs = vec![vec![1], vec![2; 20_000], vec![3]];
        let tree = PredicateTree::scripts_only(programs, [7; 32]).unwrap();
        let predicate = Predicate::opaque(tree.point);
        let opening = tree.opening_for(1).unwrap();
        let full = tree.root().as_resident().unwrap();
        assert_eq!(opening.root.hash(0).unwrap(), full.hash(0).unwrap());
        assert_ne!(opening.root.id(), full.id());
        predicate
            .verify_taproot(&opening.internal_key, &opening.root)
            .unwrap();
        for (index, reference) in opening.root.refs().iter().enumerate() {
            assert_eq!(reference.as_resident().unwrap().is_pruned(), index != 1);
        }
        let supplied = CellIndex::collect(Arc::new(opening.root.clone())).unwrap();
        for (index, program) in tree.programs().enumerate() {
            assert_eq!(supplied.contains(&program.id()), index == 1);
        }
        let forged = cells::Cell::new(vec![0], opening.root.refs().to_vec()).unwrap();
        assert!(matches!(
            predicate.verify_taproot(&opening.internal_key, &forged),
            Err(VMError::TaprootProofMismatch)
        ));
        let other_key = (RISTRETTO_BASEPOINT_TABLE * &Scalar::from(7u64)).compress();
        for key in [other_key, CompressedRistretto([0xff; 32])] {
            assert!(matches!(
                predicate.verify_taproot(&key, &opening.root),
                Err(VMError::TaprootProofMismatch)
            ));
        }
    }

    #[test]
    fn predicate_construction_is_deterministic_and_blinded() {
        let programs = vec![vec![1], vec![2], vec![3]];
        let a = PredicateTree::scripts_only(programs.clone(), [1; 32]).unwrap();
        let b = PredicateTree::scripts_only(programs.clone(), [1; 32]).unwrap();
        let c = PredicateTree::scripts_only(programs, [2; 32]).unwrap();
        assert_eq!(a.root_id(), b.root_id());
        assert_eq!(a.point, b.point);
        assert_ne!(a.root_id(), c.root_id());
        assert_ne!(a.point, c.point);
        assert_eq!(a.internal_key, Predicate::unspendable_key());
        assert!(matches!(
            a.opening_for(3),
            Err(VMError::ProgramIndexOutOfRange)
        ));
    }
}
