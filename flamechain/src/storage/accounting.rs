use std::collections::BTreeMap;
use std::sync::Arc;

use cells::{Cell, CellError, CellID, CellIndex};

#[cfg(test)]
mod benchmarks;
#[cfg(test)]
mod tests;

/// Derived, actor-local accounting; neither reference counts nor totals are encoded.
#[derive(Clone, Default)]
pub(super) struct RetainedCells {
    root: Option<CellID>,
    // Include missing bodies: supplying one later activates all its existing edges.
    references: Arc<BTreeMap<CellID, u64>>,
    pub(super) cells: Arc<CellIndex>,
    pub(super) bytes: u64,
}

impl RetainedCells {
    /// `supplied` contains only explicit resident inputs (or authenticated restore
    /// bodies), never an execution witness resolver. Unchanged owned subgraphs
    /// are already indexed and do not need to be walked again.
    pub(super) fn replace_root(
        &mut self,
        root: CellID,
        supplied: &CellIndex,
    ) -> Result<(), CellError> {
        if !supplied.contains(&root) && !self.cells.contains(&root) {
            return Err(CellError::MissingCell(root));
        }
        // COW keeps prior snapshots and failed updates intact.
        let mut next = self.clone();
        if self.root != Some(root) {
            next.add_references(vec![root], supplied)?;
        }

        // Equal content hashes do not imply equal availability. A supplied body
        // can open an unloaded edge of a parent retained before this update.
        let mut pending = Vec::new();
        for (id, cell) in supplied.iter() {
            if next.references.contains_key(id) && !next.cells.contains(id) {
                next.add_body(cell, &mut pending)?;
            }
        }
        next.add_references(pending, supplied)?;

        // Add first so shared subgraphs never temporarily lose their last owner.
        if let Some(previous) = self.root.filter(|id| *id != root) {
            next.remove_reference(previous)?;
        }
        next.root = Some(root);
        *self = next;
        Ok(())
    }

    fn add_body(&mut self, cell: &Arc<Cell>, pending: &mut Vec<CellID>) -> Result<(), CellError> {
        self.bytes = self
            .bytes
            .checked_add(cell.record_size() as u64)
            .ok_or(CellError::LimitExceeded)?;
        Arc::make_mut(&mut self.cells).insert(Arc::clone(cell))?;
        // Outgoing edges belong to the distinct body, not to each incoming path.
        pending.extend(cell.refs().iter().map(|r| r.id()));
        Ok(())
    }

    fn add_references(
        &mut self,
        mut pending: Vec<CellID>,
        supplied: &CellIndex,
    ) -> Result<(), CellError> {
        while let Some(id) = pending.pop() {
            let count = Arc::make_mut(&mut self.references).entry(id).or_default();
            *count = count.checked_add(1).ok_or(CellError::LimitExceeded)?;
            if !self.cells.contains(&id) {
                if let Some(cell) = supplied.get(&id) {
                    self.add_body(&cell, &mut pending)?;
                }
            }
        }
        Ok(())
    }

    fn remove_reference(&mut self, root: CellID) -> Result<(), CellError> {
        let mut pending = vec![root];
        while let Some(id) = pending.pop() {
            let references = Arc::make_mut(&mut self.references);
            let count = references.get_mut(&id).ok_or(CellError::InvalidFormat)?;
            *count = count.checked_sub(1).ok_or(CellError::InvalidFormat)?;
            if *count != 0 {
                continue;
            }
            references.remove(&id);
            if let Some(cell) = Arc::make_mut(&mut self.cells).remove(&id) {
                self.bytes = self
                    .bytes
                    .checked_sub(cell.record_size() as u64)
                    .ok_or(CellError::InvalidFormat)?;
                pending.extend(cell.refs().iter().map(|r| r.id()));
            }
        }
        Ok(())
    }
}
