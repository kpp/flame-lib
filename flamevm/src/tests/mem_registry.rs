//! Small in-memory [`ActorRegistry`] used by FlameVM unit tests.

use cells::{project_cell, read_cell, Cell, CellEncode, CellIndex, CellReader, CellRef};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::actor::{ActorID, ActorRegistry, StoragePurchase};
use crate::errors::VMError;
use crate::value::Value;

#[derive(Clone)]
pub struct TestActor {
    pub code: Cell,
    pub state: Option<Value>,
    pub capacity: u64,
    pub root: Cell,
    cells: Arc<CellIndex>,
    checked_out: bool,
    committed_usage: u64,
}

/// Legacy bytecode fixtures are compiled once; native graph fixtures stay intact.
pub trait TestCode {
    fn into_code(self) -> Result<Cell, VMError>;
}

impl TestCode for Vec<u8> {
    fn into_code(self) -> Result<Cell, VMError> {
        Ok(crate::code_cell(&self)?)
    }
}

impl TestCode for Cell {
    fn into_code(self) -> Result<Cell, VMError> {
        Ok(self)
    }
}

impl TestActor {
    pub fn is_checked_out(&self) -> bool {
        self.checked_out
    }
}

fn owned_cells(root: &Cell, prior: &CellIndex) -> Result<CellIndex, VMError> {
    if root.level() >= cells::MAX_CELL_LEVEL {
        return Err(cells::CellError::InvalidLevel.into());
    }
    let mut source = prior.clone();
    let mut cells = CellIndex::new();
    let mut seen = BTreeSet::new();
    let mut pending = vec![Arc::new(root.clone())];
    while let Some(cell) = pending.pop() {
        if !seen.insert(cell.id()) {
            continue;
        }
        for child in cell.refs() {
            match read_cell(&mut source, child) {
                Ok(body) => pending.push(body),
                Err(cells::CellError::MissingCell(_)) => {}
                Err(error) => return Err(error.into()),
            }
        }
        cells.insert(cell)?;
    }
    Ok(cells)
}

fn resident_usage(cells: &CellIndex) -> u64 {
    cells
        .iter()
        .map(|(_, cell)| cell.record_size() as u64)
        .sum()
}

#[derive(Clone)]
struct CheckpointFrame {
    actor_undo: BTreeMap<[u8; 32], Option<TestActor>>,
}

#[derive(Clone, Default)]
pub struct MemRegistry {
    actors: BTreeMap<[u8; 32], TestActor>,
    checkpoints: Vec<CheckpointFrame>,
    storage_quote: Option<StoragePurchase>,
}

impl MemRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    fn record_actor(&mut self, id: [u8; 32]) {
        let Self {
            actors,
            checkpoints,
            ..
        } = self;
        if let Some(frame) = checkpoints.last_mut() {
            frame
                .actor_undo
                .entry(id)
                .or_insert_with(|| actors.get(&id).cloned());
        }
    }

    pub fn actor(&self, id: &ActorID) -> Option<&TestActor> {
        self.actors.get(&id.to_hash())
    }

    pub fn set_storage_quote(&mut self, quote: Option<StoragePurchase>) {
        self.storage_quote = quote;
    }

    pub fn freeze(&mut self, id: &ActorID) -> Result<Cell, VMError> {
        let key = id.to_hash();
        let actor = self.actor(id).ok_or(VMError::ActorNotFound)?;
        if actor.is_checked_out() {
            return Err(VMError::ActorEmpty);
        }
        let original = actor.root.clone();
        let pruned = original.prune(original.level() + 1)?;
        self.record_actor(key);
        let actor = self.actors.get_mut(&key).unwrap();
        actor.root = pruned.clone();
        actor.code = pruned;
        actor.state = None;
        actor.checked_out = false;
        actor.cells = Arc::new(CellIndex::new());
        actor.committed_usage = 0;
        actor.capacity = 0;
        Ok(original)
    }

    pub fn deploy(
        &mut self,
        id: ActorID,
        code: impl TestCode,
        state: Value,
        capacity: u64,
    ) -> Result<(), VMError> {
        self.deploy_with_capacity(id, code, state, capacity)
    }

    pub fn deploy_with_capacity(
        &mut self,
        id: ActorID,
        code: impl TestCode,
        state: Value,
        capacity: u64,
    ) -> Result<(), VMError> {
        let code = code.into_code()?;
        let key = id.to_hash();
        if self.actors.contains_key(&key) {
            return Err(VMError::ActorAlreadyExists);
        }
        if !state.is_portable() {
            return Err(VMError::NonPortableInState);
        }
        let root = Cell::new(
            Vec::new(),
            vec![code.clone().into(), state.to_cell()?.into()],
        )?;
        let cells = Arc::new(owned_cells(&root, &CellIndex::new())?);
        let committed_usage = resident_usage(&cells);
        self.record_actor(key);
        self.actors.insert(
            key,
            TestActor {
                code,
                state: Some(state),
                capacity,
                root,
                cells,
                checked_out: false,
                committed_usage,
            },
        );
        Ok(())
    }

    pub fn load_state(&mut self, id: &ActorID) -> Result<Value, VMError> {
        let mut cells = self.actor_cells(id)?.as_ref().clone();
        self.load_state_with_cells(id, &mut cells)
    }

    fn take_state(&mut self, id: &ActorID, cells: &mut dyn CellReader) -> Result<Value, VMError> {
        let key = id.to_hash();
        let actor = self.actors.get(&key).ok_or(VMError::ActorNotFound)?;
        if actor.root.is_pruned() {
            return Err(VMError::ActorFrozen);
        }
        if actor.is_checked_out() {
            return Err(VMError::ActorEmpty);
        }
        let decoded = if actor.state.is_none() {
            let cell = read_cell(cells, &actor.root.refs()[1])?;
            Some(Value::from_trusted_cell(&cell, cells)?)
        } else {
            None
        };
        self.record_actor(key);
        let actor = self.actors.get_mut(&key).unwrap();
        actor.checked_out = true;
        Ok(actor.state.take().or(decoded).unwrap())
    }

    pub fn save_state(&mut self, id: &ActorID, state: Value) -> Result<(), VMError> {
        if !state.is_portable() {
            return Err(VMError::NonPortableInState);
        }
        let key = id.to_hash();
        let actor = self.actors.get(&key).ok_or(VMError::ActorNotFound)?;
        if !actor.is_checked_out() {
            return Err(VMError::SaveWithoutLoad);
        }
        let root = Cell::new(
            Vec::new(),
            vec![actor.root.refs()[0].clone(), state.to_cell()?.into()],
        )?;
        let cells = Arc::new(owned_cells(&root, &actor.cells)?);
        let usage = resident_usage(&cells);
        self.record_actor(key);
        let actor = self.actors.get_mut(&key).unwrap();
        actor.committed_usage = usage;
        actor.root = root;
        actor.cells = cells;
        actor.state = Some(state);
        actor.checked_out = false;
        Ok(())
    }

    pub fn load_code(&self, id: &ActorID) -> Result<Cell, VMError> {
        let actor = self
            .actors
            .get(&id.to_hash())
            .ok_or(VMError::ActorNotFound)?;
        if actor.root.is_pruned() {
            return Err(VMError::ActorFrozen);
        }
        if actor.is_checked_out() {
            return Err(VMError::ActorEmpty);
        }
        let mut cells = actor.cells.as_ref().clone();
        Ok(read_cell(&mut cells, &actor.root.refs()[0])?
            .as_ref()
            .clone())
    }

    pub fn set_code(&mut self, id: &ActorID, code: impl TestCode) -> Result<(), VMError> {
        let code = code.into_code()?;
        let key = id.to_hash();
        let actor = self.actors.get(&key).ok_or(VMError::ActorNotFound)?;
        if actor.is_checked_out() {
            return Err(VMError::ActorEmpty);
        }
        let root = Cell::new(
            Vec::new(),
            vec![code.clone().into(), actor.root.refs()[1].clone()],
        )?;
        let cells = Arc::new(owned_cells(&root, &actor.cells)?);
        let usage = resident_usage(&cells);
        self.record_actor(key);
        let actor = self.actors.get_mut(&key).unwrap();
        actor.code = code;
        actor.root = root;
        actor.cells = cells;
        actor.committed_usage = usage;
        Ok(())
    }

    pub fn exists(&self, id: &ActorID) -> bool {
        self.actors.contains_key(&id.to_hash())
    }

    pub fn push_checkpoint(&mut self) {
        self.checkpoints.push(CheckpointFrame {
            actor_undo: BTreeMap::new(),
        });
    }

    pub fn pop_checkpoint_commit(&mut self) {
        let Some(frame) = self.checkpoints.pop() else {
            return;
        };
        if let Some(parent) = self.checkpoints.last_mut() {
            for (id, prior) in frame.actor_undo {
                parent.actor_undo.entry(id).or_insert(prior);
            }
        }
    }

    pub fn pop_checkpoint_rollback(&mut self) {
        let Some(frame) = self.checkpoints.pop() else {
            return;
        };
        for (id, prior) in frame.actor_undo {
            match prior {
                Some(actor) => {
                    self.actors.insert(id, actor);
                }
                None => {
                    self.actors.remove(&id);
                }
            }
        }
    }
}

impl ActorRegistry for MemRegistry {
    fn is_frozen(&self, id: &ActorID) -> Result<bool, VMError> {
        Ok(self
            .actor(id)
            .ok_or(VMError::ActorNotFound)?
            .root
            .is_pruned())
    }

    fn actor_cells(&self, id: &ActorID) -> Result<Arc<CellIndex>, VMError> {
        Ok(Arc::clone(
            &self.actor(id).ok_or(VMError::ActorNotFound)?.cells,
        ))
    }

    fn restore_with_cells(
        &mut self,
        id: &ActorID,
        reference: &CellRef,
        reader: &mut dyn CellReader,
    ) -> Result<(), VMError> {
        let actor = self.actor(id).ok_or(VMError::ActorNotFound)?;
        if actor.is_checked_out() {
            return Err(VMError::ActorEmpty);
        }
        let expected = &actor.root;
        let cap = expected.level() - u8::from(expected.is_pruned());
        let proof = read_cell(reader, reference)?;
        if proof.is_pruned() || !proof.payload().is_empty() || proof.refs().len() != 2 {
            return Err(cells::CellError::InvalidFormat.into());
        }
        if proof.level() > cap + 1 {
            return Err(cells::CellError::InvalidLevel.into());
        }
        let root = project_cell(&proof, cap)?;
        if root.level_mask() != expected.level_mask() & ((1u16 << cap) - 1)
            || (0..=cap).any(|level| {
                root.hash(level) != expected.hash(level)
                    || root.depth(level) != expected.depth(level)
            })
        {
            return Err(cells::CellError::CellCommitmentMismatch(expected.hash(cap)?).into());
        }
        let mut cells = actor.cells.as_ref().clone();
        let mut pending = vec![proof];
        let mut seen = BTreeSet::new();
        while let Some(cell) = pending.pop() {
            if !seen.insert(cell.id()) || (cell.is_pruned() && cell.level() > cap) {
                continue;
            }
            cells.insert(Arc::new(project_cell(&cell, cap)?))?;
            for child in cell.refs() {
                match read_cell(reader, child) {
                    Ok(body) => pending.push(body),
                    Err(cells::CellError::MissingCell(_)) => {}
                    Err(error) => return Err(error.into()),
                }
            }
        }
        let cells = owned_cells(&root, &cells)?;
        let code = cells.get(&root.refs()[0].id());
        let usage = resident_usage(&cells);
        self.record_actor(id.to_hash());
        let actor = self.actors.get_mut(&id.to_hash()).unwrap();
        actor.root = root;
        if let Some(code) = code {
            actor.code = code.as_ref().clone();
        }
        actor.state = None;
        actor.checked_out = false;
        actor.cells = Arc::new(cells);
        actor.committed_usage = usage;
        Ok(())
    }

    fn load_state(&mut self, id: &ActorID) -> Result<Value, VMError> {
        MemRegistry::load_state(self, id)
    }

    fn load_state_with_cells(
        &mut self,
        id: &ActorID,
        cells: &mut dyn CellReader,
    ) -> Result<Value, VMError> {
        self.take_state(id, cells)
    }

    fn load_program_with_cells(
        &self,
        id: &ActorID,
        cells: &mut dyn CellReader,
    ) -> Result<Cell, VMError> {
        let actor = self.actor(id).ok_or(VMError::ActorNotFound)?;
        if actor.root.is_pruned() {
            return Err(VMError::ActorFrozen);
        }
        if actor.is_checked_out() {
            return Err(VMError::ActorEmpty);
        }
        Ok(read_cell(cells, &actor.root.refs()[0])?.as_ref().clone())
    }

    fn save_state(&mut self, id: &ActorID, state: Value) -> Result<(), VMError> {
        MemRegistry::save_state(self, id, state)
    }

    fn load_code(&self, id: &ActorID) -> Result<Cell, VMError> {
        MemRegistry::load_code(self, id)
    }

    fn actor_code_bytes(&self, id: &ActorID) -> Result<u64, VMError> {
        let actor = self.actor(id).ok_or(VMError::ActorNotFound)?;
        if actor.is_checked_out() {
            return Err(VMError::ActorEmpty);
        }
        Ok(actor.code.payload().len() as u64)
    }

    fn actor_state_bytes(&self, id: &ActorID) -> Result<u64, VMError> {
        let actor = self.actor(id).ok_or(VMError::ActorNotFound)?;
        if actor.is_checked_out() {
            return Err(VMError::ActorEmpty);
        }
        let code_bytes = cells::CellIndex::collect(std::sync::Arc::new(actor.code.clone()))?
            .iter()
            .try_fold(0u64, |size, (_, cell)| {
                size.checked_add(cell.record_size() as u64)
                    .ok_or(VMError::StorageArithmeticOverflow)
            })?;
        actor
            .committed_usage
            .checked_sub(code_bytes)
            .ok_or(VMError::StorageArithmeticOverflow)
    }

    fn set_code(&mut self, id: &ActorID, code: Cell) -> Result<(), VMError> {
        MemRegistry::set_code(self, id, code)
    }

    fn actor_usage(&self, id: &ActorID) -> Result<u64, VMError> {
        self.actor(id)
            .map(|actor| actor.committed_usage)
            .ok_or(VMError::ActorNotFound)
    }

    fn actor_capacity(&self, id: &ActorID, _height: u64) -> Result<u64, VMError> {
        self.actor(id)
            .map(|actor| actor.capacity)
            .ok_or(VMError::ActorNotFound)
    }

    fn quote_storage(
        &self,
        id: &ActorID,
        _bytes: u64,
        _height: u64,
    ) -> Result<Option<StoragePurchase>, VMError> {
        if !self.exists(id) {
            return Err(VMError::ActorNotFound);
        }
        Ok(self.storage_quote)
    }

    fn purchase_storage(
        &mut self,
        id: &ActorID,
        bytes: u64,
        height: u64,
    ) -> Result<Option<StoragePurchase>, VMError> {
        let Some(quote) = self.quote_storage(id, bytes, height)? else {
            return Ok(None);
        };
        let key = id.to_hash();
        self.record_actor(key);
        let actor = self.actors.get_mut(&key).unwrap();
        actor.capacity = actor
            .capacity
            .checked_add(bytes)
            .ok_or(VMError::StorageArithmeticOverflow)?;
        Ok(Some(quote))
    }

    fn validate_actor_storage(&self, id: &ActorID, _height: u64) -> Result<(), VMError> {
        let actor = self.actor(id).ok_or(VMError::ActorNotFound)?;
        if actor.committed_usage > actor.capacity {
            return Err(VMError::StorageCapacityExceeded);
        }
        Ok(())
    }

    fn exists(&self, id: &ActorID) -> bool {
        MemRegistry::exists(self, id)
    }

    fn push_checkpoint(&mut self) {
        MemRegistry::push_checkpoint(self)
    }

    fn pop_checkpoint_commit(&mut self) {
        MemRegistry::pop_checkpoint_commit(self)
    }

    fn pop_checkpoint_rollback(&mut self) {
        MemRegistry::pop_checkpoint_rollback(self)
    }

    fn commit_tx_destructions(&mut self) -> Vec<ActorID> {
        let destroyed: Vec<_> = self
            .actors
            .iter()
            .filter(|(_, actor)| actor.is_checked_out())
            .map(|(id, _)| ActorID::from(*id))
            .collect();
        for id in &destroyed {
            self.actors.remove(&id.to_hash());
        }
        destroyed
    }

    fn deploy(&mut self, id: ActorID, code: Cell, state: Value) -> Result<(), VMError> {
        MemRegistry::deploy_with_capacity(self, id, code, state, 0)
    }
}
