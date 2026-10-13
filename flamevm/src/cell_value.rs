use std::{convert::TryInto, ops::Deref, sync::Arc};

use cells::{Cell, CellEncode, CellError, CellRef};

use crate::{Commitment, Contract, Instruction, Point, Predicate, Scalar};

/// A VM Cell handle with optional private, per-value prover annotations.
/// Identity and serialization depend only on `reference`, never the annotation.
#[derive(Clone, Debug)]
pub struct CellValue {
    pub(crate) reference: CellRef,
    pub(crate) witness: Option<Arc<CellWitness>>,
}

/// Private prover annotations. Public bytes and Cell identity never contain these.
#[derive(Clone, Debug)]
pub enum CellWitness {
    Point(Point),
    Scalar(Scalar),
    /// Native program graph with private instruction witnesses.
    Script(Vec<Instruction>),
    /// Public Contract root with private openings of its payload values.
    Contract(Contract),
}

impl CellWitness {
    pub(crate) fn to_cell(&self) -> Result<Cell, CellError> {
        match self {
            Self::Point(point) => Cell::new(point.to_bytes().to_vec(), vec![]),
            Self::Scalar(scalar) => Cell::new(scalar.to_bytes().to_vec(), vec![]),
            Self::Contract(contract) => contract.to_cell(),
            Self::Script(instructions) => crate::script::instruction_cell(instructions),
        }
    }
}

impl CellValue {
    /// Constructs a payload-only Cell, checking capacity before admission.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, CellError> {
        Ok(Cell::new(bytes, vec![])?.into())
    }

    /// Constructs the public Cell graph and attaches its matching private annotation.
    pub fn from_witness(witness: CellWitness) -> Result<Self, CellError> {
        let mut value = Self::from(witness.to_cell()?);
        value.witness = Some(Arc::new(witness));
        Ok(value)
    }

    pub fn point(point: Point) -> Self {
        Self::from_witness(CellWitness::Point(point)).expect("32-byte Point fits in a Cell")
    }
    pub fn commitment(commitment: Commitment) -> Self {
        Self::point(Point::commitment(commitment))
    }
    pub fn predicate(predicate: Predicate) -> Self {
        Self::point(Point::predicate(predicate))
    }
    pub fn scalar(scalar: impl Into<Scalar>) -> Self {
        Self::from_witness(CellWitness::Scalar(scalar.into()))
            .expect("32-byte Scalar fits in a Cell")
    }
    pub fn contract(contract: Contract) -> Result<Self, CellError> {
        Self::from_witness(CellWitness::Contract(contract))
    }
    pub fn script(instructions: Vec<Instruction>) -> Result<Self, CellError> {
        Self::from_witness(CellWitness::Script(instructions))
    }
    pub fn witness(&self) -> Option<&CellWitness> {
        self.witness.as_deref()
    }
    pub fn reference(&self) -> &CellRef {
        &self.reference
    }

    /// Returns the resident public Cell without serializing private annotations.
    pub fn to_cell(&self) -> Result<Cell, CellError> {
        self.reference
            .as_resident()
            .cloned()
            .ok_or(CellError::MissingCell(self.id()))
    }

    /// Byte-only host APIs must not silently flatten references or pruned bodies.
    pub fn payload(&self) -> Result<&[u8], CellError> {
        let cell = self
            .reference
            .as_resident()
            .ok_or(CellError::MissingCell(self.id()))?;
        if cell.is_pruned() || !cell.refs().is_empty() {
            return Err(CellError::InvalidFormat);
        }
        Ok(cell.payload())
    }
}

impl From<CellValue> for CellRef {
    fn from(value: CellValue) -> Self {
        value.reference
    }
}

impl From<CellRef> for CellValue {
    fn from(reference: CellRef) -> Self {
        Self {
            reference,
            witness: None,
        }
    }
}

impl From<Cell> for CellValue {
    fn from(cell: Cell) -> Self {
        CellRef::from(cell).into()
    }
}

impl From<Arc<Cell>> for CellValue {
    fn from(cell: Arc<Cell>) -> Self {
        CellRef::from(cell).into()
    }
}

impl Deref for CellValue {
    type Target = CellRef;
    fn deref(&self) -> &CellRef {
        &self.reference
    }
}

pub(crate) fn array32(bytes: &[u8]) -> Option<[u8; 32]> {
    bytes.try_into().ok()
}
