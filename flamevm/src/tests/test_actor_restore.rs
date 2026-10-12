use super::test_helpers::*;
use crate::{empty_state, ActorRegistry, StoragePurchase};

fn funded_message(target: ActorAddress) -> Message {
    Message::new(
        target,
        None,
        Anchor([0x61; 32]),
        vec![Value::ClearToken(ClearToken::new(
            Scalar::from(77u64),
            FLAME_FLAVOR,
        ))],
        100_000,
        Predicate::opaque(Predicate::unspendable_key()),
    )
    .unwrap()
}

fn buy_storage() -> ScriptBuilder {
    ScriptBuilder::new()
        .push_int(1_024u64)
        .addstorage()
        .verify()
        .merge()
        .drop_()
        .drop_()
}

fn registry(code: Cell) -> (MemRegistry, ActorID) {
    let id = ActorID::from(code.id());
    let mut registry = MemRegistry::new();
    registry.set_storage_quote(Some(StoragePurchase {
        fee_sparks: Scalar::from(77u64),
        expiry_height: 52_500,
    }));
    registry
        .deploy(id, code, Value::Scalar(Scalar::from(42u64)), 10_000)
        .unwrap();
    (registry, id)
}

#[test]
fn restored_actor_buys_its_own_storage_and_records_restoration_before_purchase() {
    let (mut registry, id) = registry(buy_storage().to_cell().unwrap());
    let original = registry.freeze(&id).unwrap();
    let address = ActorAddress::Restore {
        addr: id,
        state: original.clone().into(),
    };
    let result = VM::execute_internal(
        dummy_header(),
        funded_message(address),
        &mut registry,
        &BlockContext { height: 1 },
    )
    .unwrap();
    assert!(!registry.is_frozen(&id).unwrap());
    assert_eq!(registry.actor(&id).unwrap().root.id(), original.id());
    assert_eq!(registry.actor(&id).unwrap().capacity, 1_024);
    let restore = result.txlog.iter().position(|entry|
        matches!(entry, TxEntry::ActorRestore { actor, state } if actor == &id && state.id() == original.id())
    ).unwrap();
    let purchase = result
        .txlog
        .iter()
        .position(|entry| matches!(entry, TxEntry::StoragePurchase { actor, .. } if actor == &id))
        .unwrap();
    assert!(restore < purchase);
    assert!(
        matches!(registry.load_state(&id).unwrap(), Value::Scalar(value) if value == Scalar::from(42u64))
    );
}

#[test]
fn failing_method_rolls_back_restoration_and_its_storage_purchase() {
    let code = buy_storage().push_int(0u64).verify().to_cell().unwrap();
    let (mut registry, id) = registry(code);
    let original = registry.freeze(&id).unwrap();
    let before = registry.actor(&id).unwrap().root.id();
    let target = ActorAddress::Restore {
        addr: id,
        state: original.into(),
    };
    assert!(matches!(
        VM::execute_internal(
            dummy_header(),
            funded_message(target),
            &mut registry,
            &BlockContext { height: 1 }
        )
        .unwrap_err(),
        VMError::VerifyFailed
    ));
    assert!(registry.is_frozen(&id).unwrap());
    assert_eq!(registry.actor(&id).unwrap().root.id(), before);
    assert_eq!(registry.actor(&id).unwrap().capacity, 0);
    assert!(registry.actor_cells(&id).unwrap().is_empty());
}

#[test]
fn partial_restore_can_succeed_without_reading_the_missing_data() {
    let (mut registry, id) = registry(buy_storage().to_cell().unwrap());
    let original = registry.freeze(&id).unwrap();
    let missing = original.refs()[1].id();
    let data = original.refs()[1].as_resident().unwrap().prune(1).unwrap();
    let partial = Cell::new(Vec::new(), vec![original.refs()[0].clone(), data.into()]).unwrap();
    let target = ActorAddress::Restore {
        addr: id,
        state: partial.into(),
    };
    VM::execute_internal(
        dummy_header(),
        funded_message(target),
        &mut registry,
        &BlockContext { height: 1 },
    )
    .unwrap();
    assert!(!registry.is_frozen(&id).unwrap());
    assert_eq!(registry.actor(&id).unwrap().root.id(), original.id());
    assert!(!registry.actor_cells(&id).unwrap().contains(&missing));
    assert!(
        matches!(registry.load_state(&id), Err(VMError::Cell(CellError::MissingCell(value))) if value == missing)
    );
    assert!(!registry.actor(&id).unwrap().is_checked_out());
}

#[test]
fn successful_restore_only_executions_accumulate_data_but_failed_hydration_does_not() {
    let (mut registry, id) = registry(buy_storage().to_cell().unwrap());
    let original = registry.freeze(&id).unwrap();
    let missing = original.refs()[1].id();
    let data = original.refs()[1].as_resident().unwrap().prune(1).unwrap();
    let partial = Cell::new(Vec::new(), vec![original.refs()[0].clone(), data.into()]).unwrap();
    let context = BlockContext { height: 1 };
    VM::execute_internal(
        dummy_header(),
        funded_message(ActorAddress::Restore {
            addr: id,
            state: partial.into(),
        }),
        &mut registry,
        &context,
    )
    .unwrap();
    let before = registry
        .actor_cells(&id)
        .unwrap()
        .iter()
        .map(|(id, _)| *id)
        .collect::<Vec<_>>();
    assert!(!before.contains(&missing));

    let full = ActorAddress::Restore {
        addr: id,
        state: original.clone().into(),
    };
    assert!(VM::execute_internal(
        dummy_header(),
        msg_to(full.clone()),
        &mut registry,
        &context,
    )
    .is_err());
    assert_eq!(registry.actor(&id).unwrap().capacity, 1_024);
    assert_eq!(
        registry
            .actor_cells(&id)
            .unwrap()
            .iter()
            .map(|(id, _)| *id)
            .collect::<Vec<_>>(),
        before
    );

    VM::execute_internal(
        dummy_header(),
        funded_message(full),
        &mut registry,
        &context,
    )
    .unwrap();
    assert_eq!(registry.actor(&id).unwrap().root.id(), original.id());
    assert_eq!(registry.actor(&id).unwrap().capacity, 2_048);
    assert!(registry.actor_cells(&id).unwrap().contains(&missing));
    assert!(
        matches!(registry.load_state(&id).unwrap(), Value::Scalar(value)
        if value == Scalar::from(42u64))
    );
}

#[test]
fn restore_rejects_an_unknown_actor_and_an_unauthenticated_image() {
    let (mut registry, id) = registry(buy_storage().to_cell().unwrap());
    let original = registry.freeze(&id).unwrap();
    let unknown = ActorID::from([0x91; 32]);
    assert!(matches!(
        VM::execute_internal(
            dummy_header(),
            funded_message(ActorAddress::Restore {
                addr: unknown,
                state: original.clone().into(),
            }),
            &mut registry,
            &BlockContext { height: 1 },
        ),
        Err(VMError::ActorNotFound)
    ));
    assert!(!registry.exists(&unknown));

    let forged = Cell::new(
        Vec::new(),
        vec![
            ScriptBuilder::new().nop().to_cell().unwrap().into(),
            original.refs()[1].clone(),
        ],
    )
    .unwrap();
    let before = registry.actor(&id).unwrap().root.id();
    assert!(matches!(
        VM::execute_internal(
            dummy_header(),
            funded_message(ActorAddress::Restore {
                addr: id,
                state: forged.into(),
            }),
            &mut registry,
            &BlockContext { height: 1 },
        ),
        Err(VMError::Cell(CellError::CellCommitmentMismatch(_)))
    ));
    assert!(registry.is_frozen(&id).unwrap());
    assert_eq!(registry.actor(&id).unwrap().root.id(), before);
    assert_eq!(registry.actor(&id).unwrap().capacity, 0);
    assert!(registry.actor_cells(&id).unwrap().is_empty());
}

#[test]
fn accessing_a_missing_branch_fails_and_restores_the_frozen_image() {
    let (mut registry, id) = registry(ScriptBuilder::new().load().save().to_cell().unwrap());
    let original = registry.freeze(&id).unwrap();
    let before = registry.actor(&id).unwrap().root.id();
    let missing = original.refs()[1].id();
    let data = original.refs()[1].as_resident().unwrap().prune(1).unwrap();
    let partial = Cell::new(Vec::new(), vec![original.refs()[0].clone(), data.into()]).unwrap();
    let target = ActorAddress::Restore {
        addr: id,
        state: partial.into(),
    };
    assert!(
        matches!(VM::execute_internal(dummy_header(), msg_to(target), &mut registry,
        &BlockContext { height: 1 }), Err(VMError::Cell(CellError::MissingCell(value))) if value == missing)
    );
    assert_eq!(registry.actor(&id).unwrap().root.id(), before);
    assert!(registry.is_frozen(&id).unwrap());
    assert!(registry.actor_cells(&id).unwrap().is_empty());
}

#[test]
fn init_reuses_upgraded_code_but_cannot_reset_a_frozen_actor() {
    let ctor = ScriptBuilder::new()
        .push_int(0u64)
        .verify()
        .to_cell()
        .unwrap();
    let id = ActorID::from(ctor.id());
    let mut registry = MemRegistry::new();
    let current = ScriptBuilder::new().nop().to_cell().unwrap();
    registry
        .deploy(id, current.clone(), empty_state(), 10_000)
        .unwrap();
    let target = ActorAddress::Init {
        ctor: CellRef::from(ctor).to_unloaded(),
    };
    let result = VM::execute_internal(
        dummy_header(),
        msg_to(target.clone()),
        &mut registry,
        &BlockContext { height: 1 },
    )
    .unwrap();
    assert_eq!(registry.load_code(&id).unwrap().id(), current.id());
    assert!(!result
        .txlog
        .iter()
        .any(|entry| matches!(entry, TxEntry::ActorDeploy { .. })));
    let caller = ActorID::from([0x83; 32]);
    let program = ScriptBuilder::new()
        .push_int(0u64)
        .push_int(10_000u64)
        .push_cell(target.to_cell().unwrap())
        .call()
        .verify()
        .drop_()
        .push_int(0u64)
        .push_literal(CellValue::from_bytes(vec![0; 32]).unwrap())
        .push_int(10_000u64)
        .push_cell(target.to_cell().unwrap())
        .send()
        .to_cell()
        .unwrap();
    registry
        .deploy(caller, program, empty_state(), 10_000)
        .unwrap();
    let result = VM::execute_internal(
        dummy_header(),
        msg_to(caller),
        &mut registry,
        &BlockContext { height: 1 },
    )
    .unwrap();
    let message = result
        .txlog
        .into_iter()
        .find_map(|entry| match entry {
            TxEntry::Send(message) => Some(message),
            _ => None,
        })
        .unwrap();
    VM::execute_internal(
        dummy_header(),
        message,
        &mut registry,
        &BlockContext { height: 1 },
    )
    .unwrap();
    registry.freeze(&id).unwrap();
    for target in [target, ActorAddress::Address { addr: id }] {
        assert!(matches!(
            VM::execute_internal(
                dummy_header(),
                msg_to(target),
                &mut registry,
                &BlockContext { height: 1 }
            )
            .unwrap_err(),
            VMError::ActorFrozen
        ));
    }
}

#[test]
fn synchronous_restore_failure_returns_bearer_arguments_and_discards_restore_effects() {
    let (mut registry, callee) = registry(
        ScriptBuilder::new()
            .push_int(0u64)
            .verify()
            .to_cell()
            .unwrap(),
    );
    let original = registry.freeze(&callee).unwrap();
    let frozen = registry.actor(&callee).unwrap().root.id();
    let caller = ActorID::from([0x82; 32]);
    registry
        .deploy(
            caller,
            ScriptBuilder::new().nop().to_cell().unwrap(),
            empty_state(),
            10_000,
        )
        .unwrap();
    let address = ActorAddress::Restore {
        addr: callee,
        state: original.into(),
    }
    .to_cell()
    .unwrap();
    let program = ScriptBuilder::new()
        .push_int(1u64)
        .push_int(100_000u64)
        .push_cell(address)
        .call();
    let mut vm = fixture_vm(
        program,
        CallKind::InternalRoot {
            actor: caller,
            caller: None,
        },
    );
    vm.last_anchor = Some(Anchor([0x51; 32]));
    vm.push_value(Value::ClearToken(ClearToken::new(
        Scalar::from(7u64),
        FLAME_FLAVOR,
    )));
    let mut entered = false;
    for _ in 0..32 {
        vm.step_internal_with_registry(&mut registry).unwrap();
        entered |= !vm.call_stack.is_empty();
        if entered && vm.call_stack.is_empty() {
            break;
        }
    }
    assert!(entered);
    assert!(vm.call_stack.is_empty());
    assert_eq!(vm.current_call.stack.len(), 3);
    assert!(
        matches!(&vm.current_call.stack[0], Value::ClearToken(value) if value.qty() == Scalar::from(7u64))
    );
    assert_int(&vm.current_call.stack[1], Scalar::ONE);
    assert_int(&vm.current_call.stack[2], Scalar::ZERO);
    assert!(registry.is_frozen(&callee).unwrap());
    assert_eq!(registry.actor(&callee).unwrap().root.id(), frozen);
    assert!(!vm
        .txlog
        .iter()
        .any(|entry| matches!(entry, TxEntry::ActorRestore { .. })));
}

#[test]
fn synchronous_init_without_storage_rolls_back_deployment_and_returns_arguments() {
    let ctor = ScriptBuilder::new().drop_().to_cell().unwrap();
    let target = ActorAddress::Init { ctor: ctor.into() };
    let callee = target.to_actor_id();
    let caller = ActorID::from([0x84; 32]);
    let mut registry = MemRegistry::new();
    registry
        .deploy(
            caller,
            ScriptBuilder::new().nop().to_cell().unwrap(),
            empty_state(),
            10_000,
        )
        .unwrap();
    let mut vm = fixture_vm(
        ScriptBuilder::new()
            .push_int(1u64)
            .push_int(100_000u64)
            .push_cell(target.to_cell().unwrap())
            .call(),
        CallKind::InternalRoot {
            actor: caller,
            caller: None,
        },
    );
    vm.last_anchor = Some(Anchor([0x53; 32]));
    vm.push_value(Value::Scalar(Scalar::from(9u64)));
    let mut entered = false;
    for _ in 0..32 {
        vm.step_internal_with_registry(&mut registry).unwrap();
        entered |= !vm.call_stack.is_empty();
        if entered && vm.call_stack.is_empty() {
            break;
        }
    }
    assert!(entered);
    assert!(!registry.exists(&callee));
    assert_eq!(vm.current_call.stack.len(), 3);
    assert_int(&vm.current_call.stack[0], Scalar::from(9u64));
    assert_int(&vm.current_call.stack[1], Scalar::ONE);
    assert_int(&vm.current_call.stack[2], Scalar::ZERO);
    assert!(!vm
        .txlog
        .iter()
        .any(|entry| matches!(entry, TxEntry::ActorDeploy { .. })));
}

#[test]
fn synchronous_restore_can_buy_storage_and_commit_its_image() {
    let (mut registry, callee) = registry(buy_storage().to_cell().unwrap());
    let original = registry.freeze(&callee).unwrap();
    let caller = ActorID::from([0x85; 32]);
    registry
        .deploy(
            caller,
            ScriptBuilder::new().nop().to_cell().unwrap(),
            empty_state(),
            10_000,
        )
        .unwrap();
    let target = ActorAddress::Restore {
        addr: callee,
        state: original.clone().into(),
    };
    let mut vm = fixture_vm(
        ScriptBuilder::new()
            .push_int(1u64)
            .push_int(100_000u64)
            .push_cell(target.to_cell().unwrap())
            .call(),
        CallKind::InternalRoot {
            actor: caller,
            caller: None,
        },
    );
    vm.last_anchor = Some(Anchor([0x54; 32]));
    vm.push_value(Value::ClearToken(ClearToken::new(
        Scalar::from(77u64),
        FLAME_FLAVOR,
    )));
    let mut entered = false;
    for _ in 0..32 {
        vm.step_internal_with_registry(&mut registry).unwrap();
        entered |= !vm.call_stack.is_empty();
        if entered && vm.call_stack.is_empty() {
            break;
        }
    }
    assert!(entered);
    assert_eq!(vm.current_call.stack.len(), 2);
    assert_int(&vm.current_call.stack[0], Scalar::ZERO);
    assert_int(&vm.current_call.stack[1], Scalar::ONE);
    assert!(!registry.is_frozen(&callee).unwrap());
    assert_eq!(registry.actor(&callee).unwrap().root.id(), original.id());
    assert_eq!(registry.actor(&callee).unwrap().capacity, 1_024);
    assert!(vm
        .txlog
        .iter()
        .any(|entry| matches!(entry, TxEntry::ActorRestore { actor, .. } if actor == &callee)));
}

#[test]
fn synchronous_addresses_resolve_witnesses_owned_by_the_caller() {
    for restore in [false, true] {
        let code = buy_storage().to_cell().unwrap();
        let (mut registry, callee) = registry(code.clone());
        let target = if restore {
            ActorAddress::Restore {
                addr: callee,
                state: registry.freeze(&callee).unwrap().into(),
            }
        } else {
            registry.load_state(&callee).unwrap();
            registry.commit_tx_destructions();
            ActorAddress::Init { ctor: code.into() }
        };
        let caller = ActorID::from([0x86; 32]);
        let program = ScriptBuilder::new()
            .push_int(1u64)
            .push_int(100_000u64)
            .push_cell(target.to_cell().unwrap())
            .call()
            .verify()
            .drop_()
            .to_cell()
            .unwrap();
        registry
            .deploy(caller, program.clone(), empty_state(), 10_000)
            .unwrap();
        let mut vm = VM::new(
            dummy_header(),
            CallFrame::from_cell(
                program.detached(),
                CallKind::InternalRoot {
                    actor: caller,
                    caller: None,
                },
                1_000_000,
            )
            .unwrap()
            .with_anchor(Anchor([0x55; 32])),
        );
        vm.push_value(Value::ClearToken(ClearToken::new(
            Scalar::from(77u64),
            FLAME_FLAVOR,
        )));
        while vm.step_internal_with_registry(&mut registry).unwrap() {}
        assert!(!registry.is_frozen(&callee).unwrap());
        assert!(vm.txlog.iter().any(|entry| matches!(
            entry,
            TxEntry::ActorDeploy { actor, .. } | TxEntry::ActorRestore { actor, .. }
                if actor == &callee
        )));
    }
}

#[test]
fn asynchronous_addresses_carry_witnesses_owned_by_the_sender() {
    for restore in [false, true] {
        let code = buy_storage().to_cell().unwrap();
        let (mut registry, callee) = registry(code.clone());
        let target = if restore {
            ActorAddress::Restore {
                addr: callee,
                state: registry.freeze(&callee).unwrap().into(),
            }
        } else {
            registry.load_state(&callee).unwrap();
            registry.commit_tx_destructions();
            ActorAddress::Init { ctor: code.into() }
        };
        let sender = ActorID::from([0x87; 32]);
        let program = ScriptBuilder::new()
            .push_int(1u64)
            .push_literal(CellValue::from_bytes(vec![0; 32]).unwrap())
            .push_int(100_000u64)
            .push_cell(target.to_cell().unwrap())
            .send()
            .to_cell()
            .unwrap();
        registry
            .deploy(sender, program.clone(), empty_state(), 10_000)
            .unwrap();
        let mut vm = VM::new(
            dummy_header(),
            CallFrame::from_cell(
                program.detached(),
                CallKind::InternalRoot {
                    actor: sender,
                    caller: None,
                },
                1_000_000,
            )
            .unwrap()
            .with_anchor(Anchor([0x56; 32])),
        );
        vm.push_value(Value::ClearToken(ClearToken::new(
            Scalar::from(77u64),
            FLAME_FLAVOR,
        )));
        while vm.step_internal_with_registry(&mut registry).unwrap() {}
        let message = vm
            .txlog
            .into_iter()
            .find_map(|entry| match entry {
                TxEntry::Send(message) => Some(message),
                _ => None,
            })
            .unwrap();
        VM::execute_internal(
            dummy_header(),
            message,
            &mut registry,
            &BlockContext { height: 1 },
        )
        .unwrap();
        assert!(!registry.is_frozen(&callee).unwrap());
    }
}
