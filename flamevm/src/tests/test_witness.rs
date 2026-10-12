//! Tests for the `CellWitness::Contract` witness carrier and `op_input`.
//!
//! The prover pushes `CellValue::contract(c)` carrying private Token openings.
//! Bytecode contains only the ContractID, while the public Contract body is
//! included in the transaction's Cell index. Both paths authenticate that body first;
//! the prover then restores its matching private witnesses.

#![allow(unused_imports)]

use super::test_helpers::*;

/// Prover path: push `CellValue::contract(c)` whose Token carries
/// `Commitment::Open`. After `op_input` the Token on the stack
/// still has the open commitments — witnesses survived intact.
#[test]
fn input_cell_contract_preserves_open_commitments() {
    let token = make_open_token(100, 7, 11, 13);
    let contract = Contract::new(
        Predicate::opaque(CompressedRistretto([0xaa; 32])),
        Anchor([0x42; 32]),
        test_payload(vec![Value::Token(token)]),
    )
    .expect("payload is portable");
    let mut vm = vm_external_with_script(
        ScriptBuilder::new()
            .push_literal(CellValue::contract(contract))
            .input(),
    );
    run_to_end(&mut vm).expect("input ok");
    match &vm.current_call.stack[0] {
        Value::Contract(c) => match c.payload() {
            Value::Token(t) => {
                assert!(
                    t.qty.witness().is_some(),
                    "qty must be Open under prover-side CellWitness::Contract path"
                );
                assert!(
                    t.flv.witness().is_some(),
                    "flv must be Open under prover-side CellWitness::Contract path"
                );
            }
            _ => panic!("payload not Token"),
        },
        _ => panic!("stack[0] not Contract"),
    }
}

/// Verifier path: push a payload-only Cell containing the ContractID and provide its Cell index. After
/// `op_input` the Token has `Commitment::Closed` (no witness).
#[test]
fn input_cell_without_witness_yields_closed_commitments() {
    let token = make_open_token(100, 7, 11, 13);
    let contract = Contract::new(
        Predicate::opaque(CompressedRistretto([0xaa; 32])),
        Anchor([0x42; 32]),
        test_payload(vec![Value::Token(token)]),
    )
    .expect("payload is portable");
    let program = ScriptBuilder::new()
        .with_cells(contract.to_envelope().unwrap().cells().clone())
        .push_literal(CellValue::from_bytes(contract.id().to_vec()).unwrap())
        .input();
    let mut vm = vm_external_with_script(program);
    run_to_end(&mut vm).expect("input ok");
    match &vm.current_call.stack[0] {
        Value::Contract(c) => match c.payload() {
            Value::Token(t) => {
                assert!(
                    t.qty.witness().is_none(),
                    "qty must be Closed via opaque-bytes path"
                );
                assert!(
                    t.flv.witness().is_none(),
                    "flv must be Closed via opaque-bytes path"
                );
            }
            _ => panic!("payload not Token"),
        },
        _ => panic!("stack[0] not Contract"),
    }
}

/// Prover and verifier paths produce the same contract-id — the byte
/// encoding is canonical regardless of the Cell's private annotation.
#[test]
fn input_cell_with_and_without_witness_yield_same_contract_id() {
    let token1 = make_open_token(100, 7, 11, 13);
    let token2 = make_open_token(100, 7, 11, 13);
    let contract1 = Contract::new(
        Predicate::opaque(CompressedRistretto([0xaa; 32])),
        Anchor([0x42; 32]),
        test_payload(vec![Value::Token(token1)]),
    )
    .expect("payload is portable");
    let contract2 = Contract::new(
        Predicate::opaque(CompressedRistretto([0xaa; 32])),
        Anchor([0x42; 32]),
        test_payload(vec![Value::Token(token2)]),
    )
    .expect("payload is portable");
    let public = ScriptBuilder::new()
        .with_cells(contract2.to_envelope().unwrap().cells().clone())
        .push_literal(CellValue::from_bytes(contract2.id().to_vec()).unwrap())
        .input();

    let mut vm_p = vm_external_with_script(
        ScriptBuilder::new()
            .push_literal(CellValue::contract(contract1))
            .input(),
    );
    run_to_end(&mut vm_p).expect("input ok");
    let id_p = match &vm_p.current_call.stack[0] {
        Value::Contract(c) => c.id(),
        _ => panic!(),
    };

    let mut vm_v = vm_external_with_script(public);
    run_to_end(&mut vm_v).expect("input ok");
    let id_v = match &vm_v.current_call.stack[0] {
        Value::Contract(c) => c.id(),
        _ => panic!(),
    };

    assert_eq!(
        id_p, id_v,
        "contract id is wire-derived; same under both paths"
    );
}

/// Cloning a CellValue shares its typed witness and resident public Cell.
#[test]
fn cell_contract_clone_preserves_contract() {
    let token = make_open_token(7, 11, 13, 17);
    let contract = Contract::new(
        Predicate::opaque(CompressedRistretto([0xaa; 32])),
        Anchor([0x55; 32]),
        test_payload(vec![Value::Token(token)]),
    )
    .expect("payload is portable");
    let id = contract.id();
    let s = CellValue::contract(contract);
    let cloned = s.clone();
    // The clone serializes to identical canonical bytes.
    assert_eq!(s.payload().unwrap(), cloned.payload().unwrap());
    assert!(Arc::ptr_eq(
        s.witness.as_ref().unwrap(),
        cloned.witness.as_ref().unwrap()
    ));
    let Some(CellWitness::Contract(contract)) = cloned.witness() else {
        panic!("expected Contract witness")
    };
    assert_eq!(contract.id(), id);
    let Value::Token(token) = contract.payload() else {
        panic!("expected Token")
    };
    assert!(token.qty.witness().is_some());
    assert!(token.flv.witness().is_some());
}

#[test]
fn cell_witness_does_not_change_identity_or_public_encoding() {
    let annotated = CellValue::commitment(Commitment::unblinded(Scalar::from(17u64)));
    let plain = CellValue::from_bytes(annotated.payload().unwrap().to_vec()).unwrap();
    assert_eq!(annotated.id(), plain.id());
    let encoded = Value::Cell(annotated).to_cell().unwrap();
    assert_eq!(
        encoded.id(),
        Value::Cell(plain.clone()).to_cell().unwrap().id()
    );
    let Value::Cell(decoded) = Value::from_cell(&encoded, &mut ()).unwrap() else {
        panic!("expected Cell")
    };
    assert!(decoded.witness().is_none());
    assert_eq!(decoded.payload().unwrap(), plain.payload().unwrap());
}

#[test]
fn native_push_cell_preserves_private_commitment_annotation() {
    let mut vm = vm_external_with_script(
        ScriptBuilder::new()
            .push_cell(CellValue::commitment(Commitment::unblinded(Scalar::from(
                42u64,
            ))))
            .commit(),
    );
    run_to_end(&mut vm).unwrap();
    let Value::Variable(variable) = &vm.current_call.stack[0] else {
        panic!("expected Variable")
    };
    assert_eq!(variable.commitment.assignment(), Some(Scalar::from(42u64)));
}

#[test]
fn native_push_cell_private_commitment_proves_and_verifies_after_transport() {
    let quantity = Scalar::from(42u64);
    let commitment = Commitment::blinded_with_factor(quantity, DalekScalar::from(7u64));
    let limits = crate::Limits { gas: 1_000_000 };
    let unsigned = ScriptBuilder::new()
        .push_cell(CellValue::commitment(commitment))
        .commit()
        .expr()
        .alloc(Some(quantity))
        .eq()
        .verify()
        .build_tx(dummy_header(), limits)
        .unwrap();
    let txid = unsigned.txid();
    let gas_used = unsigned.metrics().gas_used;
    let external = unsigned.without_signature().unwrap();
    let decoded = crate::ExternalTx::from_bytes_bounded(
        &external.to_bytes().unwrap(),
        1,
        1_000_000,
        1_000_000,
    )
    .unwrap();
    assert_eq!(decoded.txid, txid);
    let (_, metrics) = decoded.verify_with_metrics(limits).unwrap();
    assert_eq!(metrics.gas_used, gas_used);
}

#[test]
fn native_push_cell_forged_private_witness_is_rejected_before_commit() {
    let witness = CellValue::commitment(Commitment::unblinded(Scalar::from(7u64)));
    let forged = CellValue {
        reference: Cell::new(vec![9; 32], vec![]).unwrap().into(),
        witness: witness.witness.clone(),
    };
    let mut vm = vm_external_with_script(ScriptBuilder::new().push_cell(forged).commit());
    assert!(matches!(
        run_to_end(&mut vm),
        Err(VMError::Cell(CellError::CellCommitmentMismatch(_)))
    ));
}
