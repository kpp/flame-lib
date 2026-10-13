//! Tests for commitments.

#![allow(unused_imports)]

use super::test_helpers::*;

#[test]
fn op_expr_pushes_constant_expression() {
    // Pre-load a Scalar on the stack, dispatch `expr`,
    // confirm the result is Expression::Constant.
    let mut vm = vm_external_with_script(ScriptBuilder::new().expr().to_bytecode());
    vm.push_value(Value::Scalar(Scalar::from(99u64)));
    let mut delegate = StubDelegate::new();
    vm.step_external(&mut delegate).expect("expr ok");
    assert_eq!(vm.current_call.stack.len(), 1);
    match &vm.current_call.stack[0] {
        Value::Expression(Expression::Constant(i)) => {
            assert_eq!(*i, Scalar::from(99u64));
        }
        _ => panic!("expected Expression::Constant"),
    }
}

#[test]
fn op_commit_pushes_variable() {
    // Pre-load a Point carrying an open Commitment witness, dispatch
    // `commit`, confirm the result is a Variable with the open
    // commitment preserved.
    let mut vm = vm_external_with_script(ScriptBuilder::new().commit().to_bytecode());
    let c = Commitment::unblinded(Scalar::from(42u64));
    vm.push_value(Value::Point(Point::commitment(c.clone())));
    let mut delegate = StubDelegate::new();
    vm.step_external(&mut delegate).expect("commit ok");
    assert_eq!(vm.current_call.stack.len(), 1);
    match &vm.current_call.stack[0] {
        Value::Variable(Variable::Committed(commitment)) => {
            assert_eq!(commitment.assignment(), Some(Scalar::from(42u64)));
        }
        _ => panic!("expected Variable"),
    }
}

#[test]
fn raw_cell_bytes_can_be_read_as_a_point_then_committed() {
    let commitment = Commitment::unblinded(Scalar::from(42u64));
    let mut vm = vm_external_with_script(
        ScriptBuilder::new()
            .push_cell(Cell::new(commitment.to_point().as_bytes().to_vec(), vec![]).unwrap())
            .slice()
            .read_point()
            .verify()
            .roll_k(1)
            .drop_()
            .commit(),
    );
    run_to_end(&mut vm).unwrap();
    let [Value::Variable(Variable::Committed(Commitment::Closed(point)))] =
        vm.current_call.stack.as_slice()
    else {
        panic!("raw serialization must produce a closed commitment")
    };
    assert_eq!(*point, commitment.to_point());
}

#[test]
fn removed_scalar_opcode_is_rejected() {
    let mut vm = vm_external_with_script(vec![0x60]);
    assert!(matches!(
        run_to_end(&mut vm),
        Err(VMError::UnknownOpcode(0x60))
    ));
}

#[test]
fn commit_ignores_unrelated_private_point_annotations() {
    let limits = crate::Limits { gas: 1_000_000 };
    let point = CompressedRistretto([0; 32]);
    let mut results = Vec::new();
    for point in [
        Point::from_compressed(point),
        Point::predicate(Predicate::opaque(point)),
    ] {
        let unsigned = ScriptBuilder::new()
            .push_point_typed(point)
            .commit()
            .drop_()
            .build_tx(dummy_header(), limits)
            .unwrap();
        results.push((unsigned.txid(), unsigned.metrics()));
        unsigned
            .without_signature()
            .unwrap()
            .verify(limits)
            .unwrap();
    }
    assert_eq!(results[0], results[1]);
}

#[test]
fn prove_then_verify_with_commit_expr_eq() {
    // pushpoint <open commitment witness> ; commit ; expr ;
    // alloc(42) ; expr ; eq ; verify.
    // Both the commit-side and alloc-side Expressions point to
    // value 42 → eq holds → verify succeeds.
    let pc_gens = PedersenGens::default();
    let witness_int = Scalar::from(42u64);
    // Use a blinding factor that we'll need to encode into the
    // ScriptBuilder as a witness-bearing Point.
    let blinding = curve25519_dalek::scalar::Scalar::from(7u64);
    let c = Commitment::blinded_with_factor(witness_int, blinding);
    let program = ScriptBuilder::new()
        // Only the public point is encoded; the prover retains its opening.
        // The verifier sees a public Point, which becomes
        // Commitment::Closed(point) — sufficient for the CS to
        // bind to the same point the prover used.
        .push_point_typed(Point::commitment(c))
        .commit()
        .expr()
        .alloc(Some(witness_int))
        .expr()
        .eq()
        .verify();
    let _pp = Prover::prove(&pc_gens, program, dummy_header(), 1_000_000).expect("prove succeeds");
    let TxResult {
        bytecode,
        proof,
        cells,
        ..
    } = _pp;
    let proof = proof.expect("proof set");
    let pc_gens_v = PedersenGens::default();
    Verifier::verify_with_cells(
        &pc_gens_v,
        bytecode,
        &proof,
        dummy_header(),
        1_000_000,
        None,
        &cells,
    )
    .expect("verify succeeds");
}

#[test]
fn op_decrypt_succeeds_on_matching_witness() {
    // Build a Token from cleartext (q, f); decrypt with the
    // correct (q, f, q', f') quartet pushes ClearToken(q, f).
    // The two Pedersen-opening checks are deferred into the
    // batch verifier; consume the batch at the end to confirm
    // it accepts.
    let q = Scalar::from(100u64);
    let f = Scalar::from(7u64);
    let q_blind = Scalar::from(11u64);
    let f_blind = Scalar::from(13u64);
    let qty_commit =
        Commitment::blinded_with_factor(q, curve25519_dalek::scalar::Scalar::from(11u64));
    let flv_commit =
        Commitment::blinded_with_factor(f, curve25519_dalek::scalar::Scalar::from(13u64));
    let token = Token::new(qty_commit, flv_commit);

    let mut vm = vm_external_with_script(ScriptBuilder::new().decrypt().to_bytecode());
    vm.push_value(Value::Token(token));
    vm.push_value(Value::Scalar(f));
    vm.push_value(Value::Scalar(f_blind));
    vm.push_value(Value::Scalar(q));
    vm.push_value(Value::Scalar(q_blind));
    let mut delegate = StubDelegate::new();
    vm.step_external(&mut delegate).expect("decrypt ok");

    assert_eq!(vm.current_call.stack.len(), 1);
    match &vm.current_call.stack[0] {
        Value::ClearToken(ct) => {
            assert_eq!(ct.qty(), q);
            assert_eq!(ct.flv(), f);
        }
        _ => panic!("expected ClearToken"),
    }

    // Drain the batch and confirm both deferred openings verify.
    let batch = core::mem::replace(
        &mut delegate.batch,
        musig::BatchVerifier::new(rand::thread_rng()),
    );
    batch.verify().expect("batch verifies on correct witness");
}

#[test]
fn op_decrypt_rejects_wrong_witness() {
    // Mismatched blinding → the Pedersen-opening MSM doesn't sum to
    // identity. Under the deferred batch verification design, the
    // `decrypt` opcode itself succeeds (it just appends two
    // statements to the batch verifier); the rejection surfaces when
    // the batch is drained — same lane as Schnorr / MuSig / MSM
    // verification failures, mapped to
    // `BatchSignatureVerificationFailed` by `Verifier::verify`.
    let q = Scalar::from(100u64);
    let f = Scalar::from(7u64);
    let qty_commit =
        Commitment::blinded_with_factor(q, curve25519_dalek::scalar::Scalar::from(11u64));
    let flv_commit =
        Commitment::blinded_with_factor(f, curve25519_dalek::scalar::Scalar::from(13u64));
    let token = Token::new(qty_commit, flv_commit);
    let mut vm = vm_external_with_script(vec![0x9a]);
    vm.push_value(Value::Token(token));
    vm.push_value(Value::Scalar(f));
    vm.push_value(Value::Scalar(Scalar::from(99u64))); // wrong f_blind
    vm.push_value(Value::Scalar(q));
    vm.push_value(Value::Scalar(Scalar::from(11u64)));
    let mut delegate = StubDelegate::new();
    vm.step_external(&mut delegate)
        .expect("op_decrypt defers the check, so the step itself succeeds");

    // ClearToken got pushed (deferred design — the prover's claim is
    // taken at face value until the batch runs).
    assert!(matches!(vm.current_call.stack[0], Value::ClearToken(_)));

    // Drain the batch — wrong blinding means the deferred MSM doesn't
    // sum to identity. The verifier rejects.
    let batch = core::mem::replace(
        &mut delegate.batch,
        musig::BatchVerifier::new(rand::thread_rng()),
    );
    batch
        .verify()
        .expect_err("batch must reject mismatched blinding");
}

#[test]
fn expr_scalar_in_internal_context_errors_external_only() {
    let mut vm = vm_with_script(ScriptBuilder::new().push_int(1u64).expr().to_bytecode());
    let err = run_to_end(&mut vm).unwrap_err();
    assert!(matches!(err, VMError::ExternalOnly));
}

#[test]
fn commit_in_internal_context_errors_external_only() {
    let mut vm = vm_with_script(ScriptBuilder::new().commit().to_bytecode());
    let err = run_to_end(&mut vm).unwrap_err();
    assert!(matches!(err, VMError::ExternalOnly));
}

#[test]
fn decrypt_checks_matching_opening_immediately_in_internal_context() {
    let q = Scalar::from(100u64);
    let f = Scalar::from(7u64);
    let q_blind = Scalar::from(11u64);
    let f_blind = Scalar::from(13u64);
    let token = Token::new(
        Commitment::blinded_with_factor(q, DalekScalar::from(11u64)),
        Commitment::blinded_with_factor(f, DalekScalar::from(13u64)),
    );
    let mut vm = vm_with_script(ScriptBuilder::new().decrypt().to_bytecode());
    for value in [
        Value::Token(token),
        Value::Scalar(f),
        Value::Scalar(f_blind),
        Value::Scalar(q),
        Value::Scalar(q_blind),
    ] {
        vm.push_value(value);
    }

    vm.step_internal()
        .expect("matching opening succeeds immediately");
    assert!(matches!(
        vm.current_call.stack.as_slice(),
        [Value::ClearToken(_)]
    ));
}

#[test]
fn decrypt_rejects_wrong_opening_immediately_in_internal_context() {
    let q = Scalar::from(100u64);
    let f = Scalar::from(7u64);
    let token = Token::new(
        Commitment::blinded_with_factor(q, DalekScalar::from(11u64)),
        Commitment::blinded_with_factor(f, DalekScalar::from(13u64)),
    );
    let mut vm = vm_with_script(ScriptBuilder::new().decrypt().to_bytecode());
    for value in [
        Value::Token(token),
        Value::Scalar(f),
        Value::Scalar(Scalar::from(99u64)),
        Value::Scalar(q),
        Value::Scalar(Scalar::from(11u64)),
    ] {
        vm.push_value(value);
    }

    assert!(matches!(
        vm.step_internal(),
        Err(VMError::CommitmentOpeningMismatch)
    ));
}
