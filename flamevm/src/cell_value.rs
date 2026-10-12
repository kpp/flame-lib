use std::{convert::TryInto, ops::Deref, sync::Arc};

use cells::{Cell, CellError, CellRef, MAX_CELL_PAYLOAD};

use crate::{Commitment, Contract, Instruction, Point, Predicate, Scalar, Script, VMError};

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
    /// Source-byte script with private instruction witnesses, used by `signcall`.
    Script(Vec<Instruction>),
    /// Contract ID bytes; the public body is supplied in the transaction graph.
    Contract(Contract),
}

impl CellWitness {
    pub(crate) fn len(&self) -> usize {
        match self {
            Self::Script(instructions) => instructions.iter().map(Instruction::source_size).sum(),
            _ => 32,
        }
    }

    pub(crate) fn to_bytes_vec(&self) -> Vec<u8> {
        match self {
            Self::Point(point) => point.to_bytes().to_vec(),
            Self::Scalar(scalar) => scalar.to_bytes().to_vec(),
            Self::Contract(contract) => contract.id().to_vec(),
            Self::Script(instructions) => {
                let mut bytes = Vec::new();
                for instruction in instructions {
                    instruction.encode_source(&mut bytes);
                }
                bytes
            }
        }
    }

    pub(crate) fn into_script(self) -> Result<Script, VMError> {
        let Self::Script(instructions) = self else {
            return Err(VMError::TypeNotCell);
        };
        // signcall signs source bytes; native refs omitted by that format cannot be hidden.
        if instructions.iter().any(|instruction| match instruction {
            Instruction::PushCell(Some(_)) => true,
            Instruction::CellLiteral(value) => value.payload().is_err(),
            _ => false,
        }) {
            return Err(CellError::InvalidFormat.into());
        }
        Ok(Script::Transparent(instructions))
    }
}

impl CellValue {
    /// Constructs a payload-only Cell, checking capacity before admission.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, CellError> {
        Ok(Cell::new(bytes, vec![])?.into())
    }

    /// Constructs public bytes and attaches their matching private annotation.
    pub fn from_witness(witness: CellWitness) -> Result<Self, CellError> {
        let length = witness.len();
        if length > MAX_CELL_PAYLOAD {
            return Err(CellError::PayloadTooLarge {
                actual: length,
                max: MAX_CELL_PAYLOAD,
            });
        }
        let mut value = Self::from_bytes(witness.to_bytes_vec())?;
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
    pub fn contract(contract: Contract) -> Self {
        Self::from_witness(CellWitness::Contract(contract))
            .expect("32-byte Contract ID fits in a Cell")
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
