use std::sync::Arc;

use cells::{Cell, CellError, CellRef};

/// Owned VM cursor; copies share immutable data, never cursor state or authority.
#[derive(Clone, Debug)]
pub struct Slice {
    cell: Arc<Cell>,
    byte_offset: usize,
    ref_indices: [u8; cells::MAX_CELL_REFS],
    ref_count: u8,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_removal_order_preserves_bytes_refs_hashes_and_independent_copies() {
        fn check(slice: Slice, refs: Vec<CellRef>, payload: *const u8) {
            assert_eq!(slice.bytes(), &[2, 3]);
            assert_eq!(slice.bytes().as_ptr(), payload);
            assert_eq!(
                slice.refs().map(CellRef::id).collect::<Vec<_>>(),
                refs.iter().map(CellRef::id).collect::<Vec<_>>()
            );
            let expected = Cell::new(vec![2, 3], refs.clone()).unwrap();
            let actual = slice.commitment().unwrap();
            assert_eq!(actual.hashes(), expected.commitment().hashes());
            assert_eq!(actual.depths(), expected.commitment().depths());
            let mut invalid = slice.clone();
            assert!(invalid.load_ref(refs.len()).is_none());
            assert_eq!(invalid.commitment().unwrap().id(), actual.id());
            for count in 0..=refs.len() {
                let mut skipped = slice.clone();
                skipped.advance_refs(count);
                let expected = Cell::new(vec![2, 3], refs[count..].to_vec()).unwrap();
                assert_eq!(skipped.commitment().unwrap().id(), expected.id());
            }
            for index in 0..refs.len() {
                let mut next = slice.clone();
                let mut remaining = refs.clone();
                assert_eq!(
                    next.load_ref(index).unwrap().id(),
                    remaining.remove(index).id()
                );
                assert_eq!(slice.remaining_refs(), refs.len());
                check(next, remaining, payload);
            }
        }

        let child = Cell::new(vec![9], vec![]).unwrap();
        let refs = vec![
            child.clone().into(),
            child.prune(1).unwrap().into(),
            child.prune(4).unwrap().into(),
            CellRef::from(child).to_unloaded(),
        ];
        for count in 0..=cells::MAX_CELL_REFS {
            let cell = Arc::new(Cell::new(vec![1, 2, 3], refs[..count].to_vec()).unwrap());
            let mut slice = Slice::new(cell).unwrap();
            slice.advance_bytes(1);
            let payload = slice.bytes().as_ptr();
            check(slice, refs[..count].to_vec(), payload);
        }
    }
}

impl Slice {
    pub fn new(cell: Arc<Cell>) -> Result<Self, CellError> {
        if cell.is_pruned() {
            return Err(CellError::PrunedCell);
        }
        let ref_count = cell.refs().len() as u8;
        Ok(Self {
            cell,
            byte_offset: 0,
            ref_indices: std::array::from_fn(|index| index as u8),
            ref_count,
        })
    }

    pub fn remaining_bytes(&self) -> usize {
        self.bytes().len()
    }
    pub fn remaining_refs(&self) -> usize {
        self.ref_count as usize
    }

    /// Factual commitment of the unread byte/ref streams as an ordinary Cell.
    pub fn commitment(&self) -> Result<cells::CellCommitment, CellError> {
        Cell::compute_commitment(self.bytes(), self.refs())
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        &self.cell.payload()[self.byte_offset..]
    }
    pub(crate) fn refs(&self) -> impl ExactSizeIterator<Item = &CellRef> + Clone {
        self.ref_indices[..self.remaining_refs()]
            .iter()
            .map(move |&index| &self.cell.refs()[index as usize])
    }

    pub(crate) fn load_ref(&mut self, index: usize) -> Option<CellRef> {
        let count = self.remaining_refs();
        if index >= count {
            return None;
        }
        let reference = self.cell.refs()[self.ref_indices[index] as usize].clone();
        self.ref_indices.copy_within(index + 1..count, index);
        self.ref_count -= 1;
        Some(reference)
    }

    pub(crate) fn advance_bytes(&mut self, count: usize) {
        assert!(count <= self.remaining_bytes());
        self.byte_offset += count;
    }

    pub(crate) fn advance_refs(&mut self, count: usize) {
        let remaining = self.remaining_refs();
        assert!(count <= remaining);
        self.ref_indices.copy_within(count..remaining, 0);
        self.ref_count -= count as u8;
    }
}
