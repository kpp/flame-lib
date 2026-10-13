//! Routing addresses encoded as an expected Cell sum type.

use crate::{ActorAddress, Predicate, Value};
use cells::{CellBuilder, CellDecode, CellEncode, CellError, CellReader, CellRef, CellSlice};

#[derive(Debug)]
pub enum Address {
    Predicate(Predicate),
    MessageTarget {
        dst: ActorAddress,
        payload: Value,
        gas: u64,
    },
}

impl Address {
    pub const TAG_PREDICATE: u8 = 0;
    pub const TAG_MESSAGE_TARGET: u8 = 1;

    pub fn to_bytes(&self) -> Result<Vec<u8>, CellError> {
        Ok(self.to_envelope()?.encode())
    }
}

impl CellEncode for Address {
    fn encode(&self, builder: &mut CellBuilder) -> Result<(), CellError> {
        match self {
            Self::Predicate(predicate) => {
                builder.store_u8(Self::TAG_PREDICATE)?.store(predicate)?;
            }
            Self::MessageTarget { dst, payload, gas } => {
                if !payload.is_portable() {
                    return Err(CellError::InvalidFormat);
                }
                let payload = payload.to_cell()?;
                builder
                    .store_u8(Self::TAG_MESSAGE_TARGET)?
                    .store(dst)?
                    .store_u64(*gas)?;
                builder.store_ref(CellRef::resident(payload))?;
            }
        }
        Ok(())
    }
}

impl CellDecode for Address {
    fn decode<R: CellReader + ?Sized>(
        slice: &mut CellSlice<'_>,
        cells: &mut R,
    ) -> Result<Self, CellError> {
        match slice.load_u8()? {
            Self::TAG_PREDICATE => Ok(Self::Predicate(Predicate::decode(slice, cells)?)),
            Self::TAG_MESSAGE_TARGET => {
                let dst = ActorAddress::decode(slice, cells)?;
                let gas = slice.load_u64()?;
                let cell = cells::read_cell(cells, &slice.load_ref()?)?;
                let payload = Value::from_cell(&cell, cells)?;
                if !payload.is_portable() {
                    return Err(CellError::InvalidFormat);
                }
                Ok(Self::MessageTarget { dst, payload, gas })
            }
            _ => Err(CellError::InvalidFormat),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ClearToken, Dict, FLAME_FLAVOR, Scalar, Token};
    use cells::Cell;

    #[test]
    fn routing_roundtrip_and_invalid_tags() {
        let payloads = [
            Value::Scalar(Scalar::from(9u64)),
            Value::Token(Token::cleartext(Scalar::from(5u64), FLAME_FLAVOR).unwrap()),
            Value::Dict(Dict::from_values(vec![Value::Scalar(Scalar::from(7u64))])),
        ];
        for payload in payloads {
            let expected = payload.to_cell().unwrap().id();
            let address = Address::MessageTarget {
                dst: ActorAddress::Init {
                    ctor: Cell::new(vec![1, 2, 3], vec![]).unwrap().into(),
                },
                payload,
                gas: 42,
            };
            let cell = address.to_cell().unwrap();
            assert_eq!(cell.refs()[1].id(), expected);
            match Address::from_cell(&cell, &mut ()).unwrap() {
                Address::MessageTarget { dst, payload, gas } => {
                    match dst {
                        ActorAddress::Init { ctor } => {
                            assert_eq!(ctor.as_resident().unwrap().payload(), &[1, 2, 3]);
                        }
                        _ => panic!("wrong actor address variant"),
                    }
                    assert_eq!(payload.to_cell().unwrap().id(), expected);
                    assert_eq!(gas, 42);
                }
                _ => panic!("wrong address variant"),
            }
        }
        assert!(matches!(
            Address::from_cell(&Cell::new(vec![2], vec![]).unwrap(), &mut ()),
            Err(CellError::InvalidFormat)
        ));
    }

    #[test]
    fn nonportable_payloads_are_rejected_on_both_boundaries() {
        let negative = Value::ClearToken(ClearToken::new(-Scalar::ONE, FLAME_FLAVOR));
        for payload in [
            negative.clone(),
            Value::Dict(Dict::from_values(vec![negative])),
        ] {
            let dst = ActorAddress::from(crate::ActorID::from([0x11; 32]));
            let mut forged = CellBuilder::new();
            forged
                .store_u8(Address::TAG_MESSAGE_TARGET)
                .unwrap()
                .store(&dst)
                .unwrap()
                .store_u64(42)
                .unwrap()
                .store_ref(payload.to_cell().unwrap().into())
                .unwrap();
            assert!(matches!(
                Address::from_cell(&forged.build(), &mut ()),
                Err(CellError::InvalidFormat)
            ));
            assert!(matches!(
                Address::MessageTarget {
                    dst,
                    payload,
                    gas: 42
                }
                .to_cell(),
                Err(CellError::InvalidFormat)
            ));
        }
    }
}
