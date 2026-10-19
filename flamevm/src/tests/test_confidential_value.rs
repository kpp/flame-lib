//! Tests for confidential value.

#![allow(unused_imports)]

use super::test_helpers::*;

fn borrow_components(
    secret_qty: bool,
    secret_flv: bool,
) -> (ScriptBuilder, Commitment, Commitment) {
    let qty = if secret_qty {
        Commitment::blinded_with_factor(Scalar::from(42u64), DalekScalar::from(11u64))
    } else {
        Commitment::unblinded(Scalar::from(42u64))
    };
    let flv = if secret_flv {
        Commitment::blinded_with_factor(Scalar::from(7u64), DalekScalar::from(13u64))
    } else {
        Commitment::unblinded(Scalar::from(7u64))
    };
    let mut program = ScriptBuilder::new();
    program = if secret_qty {
        program
            .push_point_typed(Point::commitment(qty.clone()))
            .commit()
    } else {
        program.push_int(42u64)
    };
    program = if secret_flv {
        program
            .push_point_typed(Point::commitment(flv.clone()))
            .commit()
    } else {
        program.push_int(7u64)
    };
    (program.borrow(), qty, flv)
}

#[test]
fn borrow_supports_all_public_secret_component_combinations() {
    for (secret_qty, secret_flv) in [(false, false), (false, true), (true, false), (true, true)] {
        let (program, qty, flv) = borrow_components(secret_qty, secret_flv);
        let count = program.instructions().len();
        let generators = PedersenGens::default();
        let mut prover = Prover::new(&generators);
        let mut vm = vm_external_with_script(program);
        for _ in 0..count {
            vm.step_external(&mut prover).unwrap();
        }
        match vm.current_call.stack.as_slice() {
            [Value::ClearToken(debt), Value::ClearToken(credit)] => {
                assert!(!secret_qty && !secret_flv);
                assert_eq!(debt.qty(), Scalar::from(-42i64));
                assert_eq!(credit.qty(), Scalar::from(42u64));
                assert_eq!(debt.flv(), Scalar::from(7u64));
                assert_eq!(credit.flv(), Scalar::from(7u64));
            }
            [Value::WideToken(debt), Value::Token(credit)] => {
                assert!(secret_qty || secret_flv);
                assert_eq!(credit.qty, qty);
                assert_eq!(credit.flv, flv);
                let assignment = debt.0.assignment.as_deref().unwrap();
                assert_eq!(assignment.q, -spacesuit::SignedInteger::from(42u64));
                assert_eq!(assignment.f, DalekScalar::from(7u64));
            }
            _ => panic!(
                "unexpected borrow result for ({}, {})",
                secret_qty, secret_flv
            ),
        }
    }
}

#[test]
fn mixed_borrows_balance_and_have_identical_prover_verifier_gas() {
    let limits = crate::Limits { gas: 1_000_000 };
    for (secret_qty, secret_flv) in [(false, false), (false, true), (true, false), (true, true)] {
        let (program, _, flv) = borrow_components(secret_qty, secret_flv);
        let unsigned = program
            .push_literal(CellValue::commitment(Commitment::unblinded(Scalar::ZERO)))
            .push_literal(CellValue::commitment(flv))
            .push_int(2u64)
            .push_int(1u64)
            .mix()
            .retire()
            .build_tx(dummy_header(), limits)
            .unwrap();
        let gas = unsigned.metrics().gas_used;
        let external = unsigned.without_signature().unwrap();
        let decoded = crate::ExternalTx::from_bytes_bounded(
            &external.to_bytes().unwrap(),
            1,
            1_000_000,
            1_000_000,
        )
        .unwrap();
        let (_, metrics) = decoded.verify_with_metrics(limits).unwrap();
        assert_eq!(metrics.gas_used, gas, "({secret_qty}, {secret_flv})");
    }
}

#[test]
fn mixed_borrow_rejects_negative_and_overwide_public_quantities() {
    for qty in [Scalar::from(-1i64), Scalar::from(u128::from(u64::MAX) + 1)] {
        let generators = PedersenGens::default();
        let mut prover = Prover::new(&generators);
        let mut vm = vm_external_with_script(ScriptBuilder::new().borrow());
        vm.push_value(Value::Scalar(qty));
        vm.push_value(Value::Variable(Variable::Committed(Commitment::unblinded(
            Scalar::from(7u64),
        ))));
        assert!(matches!(
            vm.step_external(&mut prover),
            Err(VMError::InvalidBitrange)
        ));
    }
}

#[test]
fn mixed_borrow_accepts_u64_max_quantity_and_full_scalar_flavor() {
    let qty = Scalar::from(u64::MAX);
    let flv = -Scalar::ONE;
    for (secret_qty, secret_flv) in [(false, true), (true, false)] {
        let qty_commitment = Commitment::unblinded(qty);
        let flv_commitment = Commitment::unblinded(flv);
        let mut vm = vm_external_with_script(ScriptBuilder::new().borrow());
        vm.push_value(if secret_qty {
            Value::Variable(Variable::Committed(qty_commitment.clone()))
        } else {
            Value::Scalar(qty)
        });
        vm.push_value(if secret_flv {
            Value::Variable(Variable::Committed(flv_commitment.clone()))
        } else {
            Value::Scalar(flv)
        });
        let generators = PedersenGens::default();
        let mut prover = Prover::new(&generators);
        vm.step_external(&mut prover).unwrap();
        let [Value::WideToken(_), Value::Token(token)] = vm.current_call.stack.as_slice() else {
            panic!("expected encrypted borrow pair")
        };
        assert_eq!(token.qty, qty_commitment);
        assert_eq!(token.flv, flv_commitment);
    }
}

#[test]
fn mixed_borrow_is_external_only() {
    for (qty, flv) in [
        (
            Value::Scalar(Scalar::ONE),
            Value::Variable(Variable::Committed(Commitment::unblinded(Scalar::from(
                7u64,
            )))),
        ),
        (
            Value::Variable(Variable::Committed(Commitment::unblinded(Scalar::ONE))),
            Value::Scalar(Scalar::from(7u64)),
        ),
    ] {
        let mut vm = vm_with_script(ScriptBuilder::new().borrow());
        vm.current_call.stack = vec![qty, flv];
        assert!(matches!(vm.step_internal(), Err(VMError::ExternalOnly)));
    }
}

#[test]
fn borrowed_token_components_require_public_commitments_not_allocated_registers() {
    for program in [
        ScriptBuilder::new()
            .alloc(Some(Scalar::from(42u64)))
            .push_int(7u64)
            .borrow(),
        ScriptBuilder::new()
            .push_int(42u64)
            .alloc(Some(Scalar::from(7u64)))
            .borrow(),
    ] {
        assert!(matches!(
            program.build_tx(dummy_header(), crate::Limits { gas: 1_000_000 }),
            Err(VMError::TypeNotCommittedVariable)
        ));
    }
}

#[test]
fn encrypted_borrow_produces_widetoken_and_token_pair() {
    // pushpoint(open commitment for qty=42) commit
    // pushpoint(open commitment for flv=7) commit
    // borrow
    // After borrow the stack is [WideToken(-42, 7), Token(42, 7)].
    // Neither is droppable on its own; we can't end a script with
    // them sitting on the stack. So we inspect the borrow result
    // by hand-driving the VM and stopping after the borrow.
    let pc_gens = PedersenGens::default();
    let qty_int = Scalar::from(42u64);
    let flv_int = Scalar::from(7u64);
    let qty_blind = curve25519_dalek::scalar::Scalar::from(11u64);
    let flv_blind = curve25519_dalek::scalar::Scalar::from(13u64);
    let qty_commit = Commitment::blinded_with_factor(qty_int, qty_blind);
    let flv_commit = Commitment::blinded_with_factor(flv_int, flv_blind);
    let program = ScriptBuilder::new()
        .push_point_typed(Point::commitment(qty_commit))
        .commit()
        .push_point_typed(Point::commitment(flv_commit))
        .commit()
        .borrow();
    let mut prover = Prover::new(&pc_gens);
    let mut vm = VM::new(
        dummy_header(),
        CallFrame::new(
            program.into_instructions(),
            CallKind::ExternalRoot,
            1_000_000,
        )
        .with_readwrite(true),
    );
    // Step until borrow has executed (5 instructions: 4 setup + borrow).
    for _ in 0..5 {
        vm.step_external(&mut prover).expect("step ok");
    }
    // Stack: [WideToken, Token].
    assert_eq!(vm.current_call.stack.len(), 2);
    match (&vm.current_call.stack[0], &vm.current_call.stack[1]) {
        (Value::WideToken(_), Value::Token(t)) => {
            // +T side has the original qty commitment witness preserved.
            assert_eq!(t.qty.assignment(), Some(Scalar::from(42u64)));
            assert_eq!(t.flv.assignment(), Some(Scalar::from(7u64)));
        }
        _ => panic!("expected [WideToken, Token]"),
    }
}

#[test]
fn mix_with_single_in_single_out_balances() {
    // The simplest cloak: 1 input Token, 1 output Token with the
    // same (qty, flv). Effectively a no-op shuffle that exercises
    // the cloak gadget's range proof on the output.
    // Pre-load the input Token and supply its output commitments to mix(1, 1).
    let pc_gens = PedersenGens::default();
    let qty_int = Scalar::from(10u64);
    let flv_int = Scalar::from(7u64);
    let qty_blind = curve25519_dalek::scalar::Scalar::from(11u64);
    let flv_blind = curve25519_dalek::scalar::Scalar::from(13u64);
    let qty_commit = Commitment::blinded_with_factor(qty_int, qty_blind);
    let flv_commit = Commitment::blinded_with_factor(flv_int, flv_blind);
    let token = Token::new(qty_commit.clone(), flv_commit.clone());

    // Build a ScriptBuilder that supplies the output's commitment pair
    // and runs mix. We pre-push the Token via the harness.
    let program = ScriptBuilder::new()
        // Output commitments (pushed deepest first per op_mix):
        // first qty, then flv. Mix pops them in reverse: first
        // pop flv (top), then qty.
        .push_literal(CellValue::commitment(qty_commit))
        .push_literal(CellValue::commitment(flv_commit))
        .push_int(1u64) // m (input count)
        .push_int(1u64) // n (output count)
        .mix();
    let mut prover = Prover::new(&pc_gens);
    let mut vm = VM::new(
        dummy_header(),
        CallFrame::new(
            program.into_instructions(),
            CallKind::ExternalRoot,
            1_000_000,
        )
        .with_readwrite(true),
    );
    // Pre-load the Token at the bottom of the stack (mix pops it as input).
    vm.push_value(Value::Token(token));
    // Step exactly 5 times (the program's 5 instructions). Don't
    // drive to completion — that would trigger StackNotClean on
    // the leftover Token output (which is fine; the cloak gadget
    // built its constraints regardless).
    for _ in 0..5 {
        vm.step_external(&mut prover).expect("step ok");
    }
    // Stack: [Token(10, 7)] — the single mix output.
    assert_eq!(vm.current_call.stack.len(), 1);
    match &vm.current_call.stack[0] {
        Value::Token(_) => {}
        _ => panic!("expected single Token output"),
    }
    // Confirm prover.into_proof works on the accumulated CS.
    let _proof = prover.into_proof().expect("proof builds");
}

#[test]
fn cleartext_borrow_unaffected_by_overload() {
    // push:5, push:7, borrow → (ClearToken(-5,7), ClearToken(5,7))
    // remains the existing Phase-8 behavior because top-two aren't
    // Variables (the new dispatch peek doesn't catch them).
    let mut vm = vm_with_script(
        ScriptBuilder::new()
            .push_int(5u64)
            .push_int(7u64)
            .borrow()
            .to_bytecode(),
    );
    run_to_end(&mut vm).expect("cleartext borrow ok");
    assert_eq!(vm.current_call.stack.len(), 2);
}

#[test]
fn op_mix_m_zero_rejects() {
    // Pre-load: push n=1, m=0, then mix. Pop order: n (top), m,
    // then commitments+tokens. m=0 should reject before any
    // further pops, surfacing `MixDegenerate`.
    let mut vm = vm_external_with_script(vec![]);
    // Push m=0, n=1.
    vm.push_value(Value::Scalar(Scalar::from(0u64))); // m
    vm.push_value(Value::Scalar(Scalar::from(1u64))); // n
    let mut delegate = StubDelegate::new();
    let err = vm.op_mix(&mut delegate).unwrap_err();
    assert!(matches!(err, VMError::MixDegenerate));
}

#[test]
fn op_mix_n_zero_rejects() {
    let mut vm = vm_external_with_script(vec![]);
    vm.push_value(Value::Scalar(Scalar::from(1u64))); // m
    vm.push_value(Value::Scalar(Scalar::from(0u64))); // n
    let mut delegate = StubDelegate::new();
    let err = vm.op_mix(&mut delegate).unwrap_err();
    assert!(matches!(err, VMError::MixDegenerate));
}
