//! Tests for `MultiscalarMul` — lazy point arithmetic, batched verify.
//!
//! Covers the arithmetic lift (Point/Point/Scalar/MSM dispatch in
//! `op_add` / `op_neg` / `op_mul`), the linear-type invariants
//! (non-copyable, droppable, non-portable, non-wire-encodable),
//! and the deferred-batch semantics of `verify` on an MSM —
//! including the end-to-end Prover→Verifier path where a non-identity
//! MSM is detected only at finalize via `BatchSignatureVerificationFailed`.

#![allow(unused_imports)]

use super::test_helpers::*;
use crate::MultiscalarMul;

#[test]
fn evalpoint_computes_public_sums_in_external_and_actor_contexts() {
    use curve25519_dalek::constants::RISTRETTO_BASEPOINT_POINT as G;
    for msm in [
        MultiscalarMul::default(),
        MultiscalarMul::term(DalekScalar::from(3u64), G.compress()),
        MultiscalarMul::term(DalekScalar::from(7u64), G.compress())
            .push_term(-DalekScalar::from(4u64), G.compress()),
    ] {
        let expected = msm.clone().evaluate().unwrap().to_bytes();
        for external in [false, true] {
            let program = ScriptBuilder::new().eval_point();
            let mut vm = if external {
                vm_external_with_script(program)
            } else {
                vm_with_script(program)
            };
            vm.current_call.stack = vec![Value::MultiscalarMul(msm.clone())];
            if external {
                vm.step_external(&mut StubDelegate::new()).unwrap();
            } else {
                vm.step_internal().unwrap();
            }
            assert!(
                matches!(&vm.current_call.stack[0], Value::Point(Point::Opaque(point))
                if point.as_bytes() == &expected)
            );
            assert!(vm.current_call.stack[0].is_copyable());
            assert!(vm.current_call.stack[0].is_portable());
            assert!(vm.current_call.stack[0].is_droppable());
            assert!(vm.deferred_sigs.is_empty());
            assert_eq!(vm.current_call.gas_used, msm_eval_gas(msm.len()).unwrap());
        }
    }
    assert_eq!(
        MultiscalarMul::default().evaluate().unwrap().to_bytes(),
        [0; 32]
    );
    let result = MultiscalarMul::term(DalekScalar::from(3u64), G.compress())
        .evaluate()
        .unwrap();
    assert_eq!(
        result.to_bytes(),
        (G * DalekScalar::from(3u64)).compress().to_bytes()
    );
}

#[test]
fn evalpoint_checks_malformed_points_even_with_zero_coefficients() {
    let invalid = CompressedRistretto([0xff; 32]);
    for scalar in [DalekScalar::ZERO, DalekScalar::ONE] {
        let mut vm = vm_with_script(ScriptBuilder::new().eval_point());
        vm.current_call.stack = vec![Value::MultiscalarMul(MultiscalarMul::term(scalar, invalid))];
        assert!(matches!(vm.step_internal(), Err(VMError::InvalidPoint)));
        assert!(vm.current_call.stack.is_empty());
        assert_eq!(vm.current_call.gas_used, 1_044);
    }
}

#[test]
fn type_dependent_verify_and_evalpoint_prices_are_exact_and_precharged() {
    use curve25519_dalek::constants::RISTRETTO_BASEPOINT_COMPRESSED as G;
    for terms in [0, 1, 2, 8, 64] {
        let mut msm = MultiscalarMul::default();
        for _ in 0..terms {
            msm = msm.push_term(DalekScalar::ZERO, G);
        }
        let mut verify = vm_external_with_script(ScriptBuilder::new().verify());
        verify.current_call.stack = vec![Value::MultiscalarMul(msm.clone())];
        verify.step_external(&mut StubDelegate::new()).unwrap();
        assert_eq!(verify.current_call.gas_used, 221 + 40 * terms as u64);
        let price = verify.current_call.gas_used * 4;
        for budget in [price - 1, price] {
            let mut eval = vm_with_script(ScriptBuilder::new().eval_point());
            eval.current_call.gas_limit = budget;
            eval.current_call.stack = vec![Value::MultiscalarMul(msm.clone())];
            let result = eval.step_internal();
            if budget == price {
                result.unwrap();
                assert_eq!(eval.current_call.gas_used, price);
                assert!(matches!(eval.current_call.stack[0], Value::Point(_)));
            } else {
                assert!(matches!(result, Err(VMError::OutOfGas)));
                assert!(matches!(
                    eval.current_call.stack[0],
                    Value::MultiscalarMul(_)
                ));
            }
        }
    }
    for value in [
        Value::Scalar(Scalar::ONE),
        Value::Constraint(Constraint::Cleartext(true)),
    ] {
        let mut vm = vm_external_with_script(ScriptBuilder::new().verify());
        vm.current_call.stack = vec![value];
        vm.step_external(&mut StubDelegate::new()).unwrap();
        assert_eq!(vm.current_call.gas_used, 1);
    }
    let equality = Constraint::eq(
        Expression::LinearCombination(vec![], None),
        Expression::constant(Scalar::ZERO),
    );
    for constraint in [
        equality.clone(),
        Constraint::not(equality.clone()),
        Constraint::or(equality.clone(), equality),
    ] {
        let price = 1
            + Value::Constraint(constraint.clone()).clone_gas()
            + 120 * constraint.multiplier_count() as u64;
        let mut vm = vm_external_with_script(ScriptBuilder::new().verify());
        vm.current_call.stack = vec![Value::Constraint(constraint)];
        vm.step_external(&mut StubDelegate::new()).unwrap();
        assert_eq!(vm.current_call.gas_used, price);
    }
    #[cfg(target_pointer_width = "64")]
    {
        assert!(matches!(msm_verify_gas(usize::MAX), Err(VMError::OutOfGas)));
        assert!(matches!(
            msm_eval_gas(usize::MAX / 100),
            Err(VMError::OutOfGas)
        ));
    }
}

#[test]
fn evalpoint_rejects_other_types_without_consuming_them() {
    for value in [
        Value::Scalar(Scalar::ONE),
        Value::Point(Point::from_bytes([0; 32])),
        Value::Expression(Expression::constant(Scalar::ONE)),
        Value::ClearToken(ClearToken::new(Scalar::ONE, FLAME_FLAVOR)),
    ] {
        let mut vm = vm_with_script(ScriptBuilder::new().eval_point());
        vm.current_call.stack = vec![value];
        assert!(matches!(
            vm.step_internal(),
            Err(VMError::TypeNotMultiscalarMul)
        ));
        assert_eq!(vm.current_call.stack.len(), 1);
    }
    assert!(matches!(
        vm_with_script(ScriptBuilder::new().eval_point()).step_internal(),
        Err(VMError::StackUnderflow)
    ));
    assert_eq!(Instruction::EvalPoint.encode_to_vec(), vec![0x89]);
    assert!(matches!(
        Instruction::parse(&mut &[0x89][..]),
        Ok(Instruction::EvalPoint)
    ));
}

#[test]
fn evalpoint_result_branches_and_serializes_with_prover_verifier_agreement() {
    use curve25519_dalek::constants::RISTRETTO_BASEPOINT_POINT as G;
    let expected = (G * DalekScalar::from(3u64)).compress().to_bytes();
    let program = ScriptBuilder::new()
        .push_int(3u64)
        .push_point(G.compress().to_bytes())
        .mul()
        .eval_point()
        .push_point(expected)
        .eq()
        .jumpif(0)
        .push_int(0u64)
        .verify()
        .label(0)
        .drop_()
        .builder()
        .roll_k(1)
        .write_point()
        .endcell()
        .log();
    let pc = PedersenGens::default();
    let result = Prover::prove(&pc, program, dummy_header(), 1_000_000).unwrap();
    let verified = Verifier::verify_cell_with_cells(
        &pc,
        result.program.clone(),
        result.proof.as_ref().unwrap(),
        dummy_header(),
        1_000_000,
        None,
        &result.cells,
    )
    .unwrap();
    assert_eq!(
        (result.txid, result.gas_used),
        (verified.txid, verified.gas_used)
    );
    assert!(matches!(&verified.txlog[1], TxEntry::Data(bytes) if bytes == &expected));
    let mut internal = vm_with_script(
        ScriptBuilder::new()
            .push_int(0u64)
            .push_point(G.compress().to_bytes())
            .mul()
            .eval_point()
            .push_point([0; 32])
            .eq()
            .verify()
            .drop_()
            .drop_(),
    );
    run_to_end(&mut internal).unwrap();
}

#[test]
fn evalpoint_failure_restores_locked_payload_arguments_and_parent_debt() {
    let code = ScriptBuilder::new()
        .push_int(0u64)
        .push_point([0xff; 32])
        .mul()
        .eval_point()
        .to_cell()
        .unwrap();
    let key = Predicate::unspendable_key();
    let contract = Contract::new(
        Predicate::taproot(key, &code).unwrap(),
        Anchor([7; 32]),
        Value::ClearToken(ClearToken::new(Scalar::from(17u64), FLAME_FLAVOR)),
    )
    .unwrap();
    let id = contract.id();
    let mut vm = vm_with_script(ScriptBuilder::new().open());
    vm.current_call.stack = vec![
        Value::ClearToken(ClearToken::new(Scalar::from(-9i64), FLAME_FLAVOR)),
        Value::Contract(Box::new(contract)),
        Value::Point(Point::from_compressed(key)),
        Value::Cell(code.into()),
        Value::Scalar(Scalar::from(10_000u64)),
        Value::ClearToken(ClearToken::new(Scalar::from(9u64), FLAME_FLAVOR)),
        Value::Scalar(Scalar::ONE),
    ];
    run_to_end(&mut vm).unwrap();
    assert_eq!(vm.current_call.stack.len(), 5);
    assert!(
        matches!(&vm.current_call.stack[0], Value::ClearToken(t) if t.qty() == Scalar::from(-9i64))
    );
    assert!(matches!(&vm.current_call.stack[1], Value::Contract(c) if c.id() == id));
    assert!(
        matches!(&vm.current_call.stack[2], Value::ClearToken(t) if t.qty() == Scalar::from(9u64))
    );
    assert_int(&vm.current_call.stack[3], Scalar::ONE);
    assert_int(&vm.current_call.stack[4], Scalar::ZERO);
    assert!(vm.current_call.gas_used < 10_000);
}

// ── Construction via arithmetic ─────────────────────────────────

/// `point + point` → `MultiscalarMul` with two unit-scalar terms.
#[test]
fn op_add_point_point_lifts_to_msm() {
    let mut vm = vm_with_script(
        ScriptBuilder::new()
            .push_point([0x55; 32])
            .push_point([0x66; 32])
            .add()
            .to_bytecode(),
    );
    run_to_end(&mut vm).unwrap();
    match &vm.current_call.stack[0] {
        Value::MultiscalarMul(m) => assert_eq!(m.len(), 2),
        other => panic!("expected MSM, got {}", value_kind(other)),
    }
}

/// `int * point` → MSM with one term `(int_as_scalar, point)`.
#[test]
fn op_mul_int_point_lifts_to_msm() {
    let mut vm = vm_with_script(
        ScriptBuilder::new()
            .push_int(3u64)
            .push_point([0x55; 32])
            .mul()
            .to_bytecode(),
    );
    run_to_end(&mut vm).unwrap();
    match &vm.current_call.stack[0] {
        Value::MultiscalarMul(m) => assert_eq!(m.len(), 1),
        other => panic!("expected MSM, got {}", value_kind(other)),
    }
}

/// `point * int` matches `int * point` (commutative dispatch).
#[test]
fn op_mul_point_int_lifts_to_msm() {
    let mut vm = vm_with_script(
        ScriptBuilder::new()
            .push_point([0x55; 32])
            .push_int(3u64)
            .mul()
            .to_bytecode(),
    );
    run_to_end(&mut vm).unwrap();
    assert!(matches!(vm.current_call.stack[0], Value::MultiscalarMul(_)));
}

/// `neg point` → MSM with one term `(-1, point)`.
#[test]
fn op_neg_point_lifts_to_msm() {
    let mut vm = vm_with_script(
        ScriptBuilder::new()
            .push_point([0x55; 32])
            .neg()
            .to_bytecode(),
    );
    run_to_end(&mut vm).unwrap();
    match &vm.current_call.stack[0] {
        Value::MultiscalarMul(m) => assert_eq!(m.len(), 1),
        other => panic!("expected MSM, got {}", value_kind(other)),
    }
}

/// `msm + msm` concatenates term vectors.
#[test]
fn op_add_msm_msm_concatenates() {
    // Build two MSMs (each of size 2 via point+point), then add them.
    let script = ScriptBuilder::new()
        .push_point([0x11; 32])
        .push_point([0x22; 32])
        .add() // MSM_A
        .push_point([0x33; 32])
        .push_point([0x44; 32])
        .add() // MSM_B
        .add() // MSM_A + MSM_B
        .to_bytecode();
    let mut vm = vm_with_script(script);
    run_to_end(&mut vm).unwrap();
    match &vm.current_call.stack[0] {
        Value::MultiscalarMul(m) => assert_eq!(m.len(), 4),
        other => panic!("expected MSM, got {}", value_kind(other)),
    }
}

/// `msm * int` scales all coefficients (`len` unchanged).
#[test]
fn op_mul_msm_int_scales() {
    let script = ScriptBuilder::new()
        .push_point([0x11; 32])
        .push_point([0x22; 32])
        .add() // MSM size 2
        .push_int(7u64)
        .mul() // scaled MSM
        .to_bytecode();
    let mut vm = vm_with_script(script);
    run_to_end(&mut vm).unwrap();
    match &vm.current_call.stack[0] {
        Value::MultiscalarMul(m) => assert_eq!(m.len(), 2),
        other => panic!("expected MSM, got {}", value_kind(other)),
    }
}

/// `point + msm` and `msm + point` both append (commutative).
#[test]
fn op_add_msm_point_appends_either_order() {
    let script = ScriptBuilder::new()
        .push_point([0x11; 32])
        .push_point([0x22; 32])
        .add() // MSM size 2
        .push_point([0x33; 32])
        .add() // MSM + point
        .to_bytecode();
    let mut vm = vm_with_script(script);
    run_to_end(&mut vm).unwrap();
    assert!(matches!(&vm.current_call.stack[0],
        Value::MultiscalarMul(m) if m.len() == 3));
}

// ── Rejected operand combinations ───────────────────────────────

/// `point * point` is a quadratic group-element product — no
/// Sigma-protocol semantics → hard fail.
#[test]
fn op_mul_point_point_rejected() {
    let mut vm = vm_with_script(
        ScriptBuilder::new()
            .push_point([0x55; 32])
            .push_point([0x66; 32])
            .mul()
            .to_bytecode(),
    );
    assert!(matches!(
        run_to_end(&mut vm).unwrap_err(),
        VMError::TypeNotScalar
    ));
}

/// `msm * msm` is also quadratic in group elements → hard fail.
#[test]
fn op_mul_msm_msm_rejected() {
    let script = ScriptBuilder::new()
        .push_point([0x11; 32])
        .push_point([0x22; 32])
        .add()
        .push_point([0x33; 32])
        .push_point([0x44; 32])
        .add()
        .mul()
        .to_bytecode();
    let mut vm = vm_with_script(script);
    assert!(matches!(
        run_to_end(&mut vm).unwrap_err(),
        VMError::TypeNotScalar
    ));
}

// ── Linear-type invariants ──────────────────────────────────────

/// MSM is non-copyable: `dup` rejects.
#[test]
fn msm_dup_rejects() {
    let mut vm = vm_with_script(
        ScriptBuilder::new()
            .push_point([0x55; 32])
            .neg()
            .dup_k(0)
            .to_bytecode(),
    );
    assert!(matches!(
        run_to_end(&mut vm).unwrap_err(),
        VMError::TypeNotCopyable
    ));
}

/// MSM IS droppable: it's a pure-computation accumulator with no
/// embedded asset value. `drop` succeeds and the stack ends clean.
#[test]
fn msm_drop_succeeds() {
    let mut vm = vm_with_script(
        ScriptBuilder::new()
            .push_point([0x55; 32])
            .neg()
            .drop_()
            .to_bytecode(),
    );
    run_to_end(&mut vm).expect("MSM is droppable (pure computation)");
    assert!(vm.current_call.stack.is_empty());
}

/// MSM is non-portable: cannot be sealed into a contract payload.
#[test]
fn msm_in_contract_payload_rejected() {
    let script = ScriptBuilder::new()
        .push_point([0x55; 32])
        .neg() // → MSM
        .push_point([0xaa; 32]) // predicate
        .contract()
        .to_bytecode();
    let mut vm = vm_with_script(script);
    vm.last_anchor = Some(Anchor([0x42; 32]));
    assert!(matches!(
        run_to_end(&mut vm).unwrap_err(),
        VMError::NonPortableInOutput
    ));
}

// ── Deferred batch verify semantics ─────────────────────────────

/// Identity MSM via `0 * G` — verify succeeds end-to-end through
/// Prover → Verifier. The batched check `sum = identity` holds
/// because `0 * G = identity`. The point still has to *decompress*
/// (Dalek's `optional_multiscalar_mul` decompresses before scaling),
/// so we use the canonical basepoint here.
#[test]
fn verify_msm_identity_succeeds_end_to_end() {
    use bulletproofs::PedersenGens;
    let pc_gens = PedersenGens::default();
    let g_bytes = *curve25519_dalek::constants::RISTRETTO_BASEPOINT_COMPRESSED.as_bytes();
    // push:0  pushpoint(G)  mul  verify
    let prog = ScriptBuilder::new()
        .push_int(0u64)
        .push_point(g_bytes)
        .mul()
        .verify();
    let result = Prover::prove(&pc_gens, prog, dummy_header(), 1_000_000).expect("prove succeeds");
    let TxResult {
        program,
        proof,
        cells,
        ..
    } = result;
    let proof = proof.expect("proof set");
    let pc_gens_v = PedersenGens::default();
    Verifier::verify_cell_with_cells(
        &pc_gens_v,
        program,
        &proof,
        dummy_header(),
        1_000_000,
        None,
        &cells,
    )
    .expect("verify succeeds (0*P = identity)");
}

/// Identity MSM via `P + (-P)` — also succeeds.
#[test]
fn verify_msm_negation_sum_identity_succeeds() {
    use bulletproofs::PedersenGens;
    let pc_gens = PedersenGens::default();
    // Use the basepoint (canonical valid Ristretto point) for both
    // terms so decompression succeeds on the verifier.
    let g = curve25519_dalek::constants::RISTRETTO_BASEPOINT_COMPRESSED;
    let g_bytes = *g.as_bytes();
    // pushpoint(G)  pushpoint(G)  neg  add  verify
    // Stack after neg: [G_msm(-1)] then push G → [MSM(-1,G), G]; add → MSM with terms [(-1,G), (1,G)].
    let prog = ScriptBuilder::new()
        .push_point(g_bytes)
        .neg() // → MSM(-1, G)
        .push_point(g_bytes)
        .add() // → MSM with [(-1,G), (1,G)]
        .verify();
    let result = Prover::prove(&pc_gens, prog, dummy_header(), 1_000_000).expect("prove");
    let TxResult {
        program,
        proof,
        cells,
        ..
    } = result;
    let proof = proof.expect("proof set");
    let pc_gens_v = PedersenGens::default();
    Verifier::verify_cell_with_cells(
        &pc_gens_v,
        program,
        &proof,
        dummy_header(),
        1_000_000,
        None,
        &cells,
    )
    .expect("verify");
}

/// Non-identity MSM: `1 * G` where G is the basepoint — the sum is G
/// itself, not identity. Verifier's batch check fails.
#[test]
fn verify_msm_nonidentity_rejected_at_batch() {
    use bulletproofs::PedersenGens;
    let pc_gens = PedersenGens::default();
    let g = curve25519_dalek::constants::RISTRETTO_BASEPOINT_COMPRESSED;
    let g_bytes = *g.as_bytes();
    // push:1  pushpoint(G)  mul  verify
    let prog = ScriptBuilder::new()
        .push_int(1u64)
        .push_point(g_bytes)
        .mul()
        .verify();
    let result = Prover::prove(&pc_gens, prog, dummy_header(), 1_000_000)
        .expect("prover produces proof regardless of MSM correctness");
    let TxResult {
        program,
        proof,
        cells,
        ..
    } = result;
    let proof = proof.expect("proof set");
    let pc_gens_v = PedersenGens::default();
    let err = Verifier::verify_cell_with_cells(
        &pc_gens_v,
        program,
        &proof,
        dummy_header(),
        1_000_000,
        None,
        &cells,
    )
    .expect_err("verify must reject");
    assert!(matches!(err, VMError::BatchSignatureVerificationFailed));
}

/// `verify` attempts point decompression when scheduling the MSM but preserves
/// failure as `None`; final batch verification rejects that deferred failure.
#[test]
fn verify_msm_invalid_point_rejected_at_batch() {
    use bulletproofs::PedersenGens;
    let pc_gens = PedersenGens::default();
    // 0xff-filled bytes do not decompress as a valid Ristretto point.
    let bad = [0xff; 32];
    // push:0  pushpoint(bad)  mul  verify — even with coefficient 0,
    // `verify` records failed decompression as `None`, so scheduling succeeds
    // but final batch acceptance fails.
    let prog = ScriptBuilder::new()
        .push_int(0u64)
        .push_point(bad)
        .mul()
        .verify();
    let result = Prover::prove(&pc_gens, prog, dummy_header(), 1_000_000).expect("prove");
    let TxResult {
        program,
        proof,
        cells,
        ..
    } = result;
    let proof = proof.expect("proof set");
    let pc_gens_v = PedersenGens::default();
    let err = Verifier::verify_cell_with_cells(
        &pc_gens_v,
        program,
        &proof,
        dummy_header(),
        1_000_000,
        None,
        &cells,
    )
    .expect_err("verify must reject");
    assert!(matches!(err, VMError::BatchSignatureVerificationFailed));
}

/// `verify` on an MSM in internal context errors `ExternalOnly`.
#[test]
fn verify_msm_in_internal_context_rejected() {
    let mut vm = vm_with_script(
        ScriptBuilder::new()
            .push_point([0x55; 32])
            .neg()
            .verify()
            .to_bytecode(),
    );
    assert!(matches!(
        run_to_end(&mut vm).unwrap_err(),
        VMError::ExternalOnly
    ));
}

#[test]
fn verify_msm_debits_decompression_and_finalization_work() {
    let point = curve25519_dalek::constants::RISTRETTO_BASEPOINT_COMPRESSED;
    let msm = MultiscalarMul::term(DalekScalar::ZERO, point);
    let expected = GAS_PER_INSTRUCTION
        + 1 // rollback/operand growth for the one-term MSM
        + linear_gas(GAS_MSM_VERIFY_BASE, GAS_MSM_VERIFY_TERM, 1).unwrap()
        + 2; // scalar and decompressed-point batch vectors

    let mut vm = vm_external_with_script(ScriptBuilder::new().verify().to_bytecode());
    vm.current_call.stack = vec![Value::MultiscalarMul(msm.clone())];
    vm.current_call.gas_limit = expected;
    let mut delegate = make_stub_delegate();
    assert!(vm
        .step_external(&mut delegate)
        .expect("exact budget succeeds"));
    assert_eq!(vm.current_call.gas_used, expected);

    let mut short = vm_external_with_script(ScriptBuilder::new().verify().to_bytecode());
    short.current_call.stack = vec![Value::MultiscalarMul(msm)];
    short.current_call.gas_limit = expected - 1;
    let mut delegate = make_stub_delegate();
    assert!(matches!(
        short.step_external(&mut delegate),
        Err(VMError::OutOfGas)
    ));
}

// ── Per-frame batch isolation ────────────────────────────────────

/// Builds an outer program that consumes a contract (to seed the anchor)
/// then opens it via the script-leaf path, with `inner` as the leaf
/// program. Returns a ScriptBuilder ready for `Prover::prove`.
fn open_with_inner(inner: ScriptBuilder, recover_failed_contract: bool) -> ScriptBuilder {
    open_with_test_inner(inner, recover_failed_contract)
}

/// A failed nested call that appended a *non-identity* MSM to its
/// pending batch must not pollute the parent's batch. After the
/// parent finishes cleanly (with no MSM of its own), the verifier
/// batch should sum to the identity — i.e. the proof verifies.
///
/// Without per-frame batching this would fail with
/// `BatchSignatureVerificationFailed` because the bogus 1·G ≠ 0
/// statement from the failed callee would survive in the global batch.
#[test]
fn failed_call_msm_does_not_pollute_parent_batch() {
    use bulletproofs::PedersenGens;
    let pc_gens = PedersenGens::default();
    let g_bytes = *curve25519_dalek::constants::RISTRETTO_BASEPOINT_COMPRESSED.as_bytes();

    // Inner contract-open script: append a NON-identity MSM (1·G) to the
    // child frame's batch, then deliberately fail via `verify(0)` so
    // the whole frame is rolled back into a `0` marker on the parent.
    let inner = ScriptBuilder::new()
        .push_int(1u64)
        .push_point(g_bytes)
        .mul()
        .verify() // appends 1·G to pending_batch
        .push_int(0u64)
        .verify() // VerifyFailed → frame unwinds
        .push_int(0u64)
        .return_();
    let outer = open_with_inner(inner, true)
        // Trivially-true constraint to give the proof something to check.
        .alloc(Some(Scalar::from(7u64)))
        .alloc(Some(Scalar::from(7u64)))
        .eq()
        .verify();
    let result = Prover::prove(&pc_gens, outer, dummy_header(), 1_000_000).expect("prove ok");
    let TxResult {
        program,
        proof,
        cells,
        ..
    } = result;
    let proof = proof.expect("proof set");

    // Verifier-side must accept: the parent's batch contains only the
    // (identity) contribution from its own clean execution; the
    // child's polluting 1·G never reaches the global batch.
    let pc_gens_v = PedersenGens::default();
    Verifier::verify_cell_with_cells(
        &pc_gens_v,
        program,
        &proof,
        dummy_header(),
        1_000_000,
        None,
        &cells,
    )
    .expect("verify must accept — failed call's MSM was discarded");
}

/// Inverse: when the contract-open succeeds cleanly, its non-identity MSM
/// IS merged into the parent's batch and the verifier rejects.
/// Confirms the per-frame design isn't accidentally swallowing every
/// MSM, only those from failed frames.
#[test]
fn clean_call_msm_propagates_to_parent_batch() {
    use bulletproofs::PedersenGens;
    let pc_gens = PedersenGens::default();
    let g_bytes = *curve25519_dalek::constants::RISTRETTO_BASEPOINT_COMPRESSED.as_bytes();

    // Inner: append 1·G to batch, then return cleanly.
    let inner = ScriptBuilder::new()
        .push_int(1u64)
        .push_point(g_bytes)
        .mul()
        .verify()
        .push_int(0u64)
        .return_();
    let outer = open_with_inner(inner, false)
        .verify() // pop success marker
        .drop_(); // drop count

    let result = Prover::prove(&pc_gens, outer, dummy_header(), 1_000_000)
        .expect("prove ok (prover always builds something)");
    let TxResult {
        program,
        proof,
        cells,
        ..
    } = result;
    let proof = proof.expect("proof set");

    let pc_gens_v = PedersenGens::default();
    let err = Verifier::verify_cell_with_cells(
        &pc_gens_v,
        program,
        &proof,
        dummy_header(),
        1_000_000,
        None,
        &cells,
    )
    .expect_err("verify must reject — clean call merged the 1·G MSM into the parent batch");
    assert!(matches!(err, VMError::BatchSignatureVerificationFailed));
}
