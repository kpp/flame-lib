//! Read-only actor calls share the actor ownership rules without exporting assets.

use super::test_helpers::*;
use crate::{query_actor, QueryResult};

const QUERY_GAS: u64 = 100_000;

fn deploy_query(registry: &mut MemRegistry, program: ScriptBuilder, state: Value) -> ActorID {
    let code = program.to_cell().unwrap();
    let actor = ActorID::from(code.id());
    registry.deploy(actor, code, state, 1_000_000).unwrap();
    actor
}

fn query(registry: &mut MemRegistry, actor: ActorID, arg: Value) -> Result<QueryResult, VMError> {
    query_actor(
        actor,
        arg,
        QUERY_GAS,
        registry,
        &BlockContext { height: 123 },
        Arc::new(CellIndex::new()),
    )
}

fn unit() -> Value {
    Value::Scalar(Scalar::ZERO)
}

fn run_actor_call(registry: &mut MemRegistry, target: ActorID, arg: Value, readonly: bool) -> VM {
    let caller = ActorID::from([0x77; 32]);
    if registry.actor(&caller).is_none() {
        registry
            .deploy(
                caller,
                ScriptBuilder::new().nop().to_cell().unwrap(),
                unit(),
                1_000_000,
            )
            .unwrap();
    }
    let mut program = ScriptBuilder::new()
        .push_int(QUERY_GAS)
        .push_cell(actor_address(&target));
    program.push_instr(if readonly {
        Instruction::Query
    } else {
        Instruction::Call
    });
    let frame = CallFrame::new(
        program.into_instructions(),
        CallKind::InternalRoot {
            actor: caller,
            caller: None,
        },
        1_000_000,
    )
    .with_readwrite(true)
    .with_anchor(Anchor([0x55; 32]));
    let mut vm = VM::new(dummy_header(), frame);
    vm.push_value(arg);
    while !vm.call_stack.is_empty() || !vm.current_call.is_finished() {
        vm.step_internal_with_registry(registry).unwrap();
    }
    vm
}

#[test]
fn call_frames_default_to_readonly() {
    let frame = CallFrame::new(vec![], CallKind::ExternalRoot, 100);
    assert!(!frame.readwrite);
    assert!(frame.with_readwrite(true).readwrite);
}

#[test]
fn offchain_getter_restores_state_and_reports_gas() {
    let mut registry = MemRegistry::new();
    let actor = deploy_query(
        &mut registry,
        ScriptBuilder::new()
            .drop_()
            .load()
            .dup_k(0)
            .save()
            .return_(),
        Value::Scalar(Scalar::from(42u64)),
    );
    let root = registry.actor(&actor).unwrap().root.id();
    let result = query(&mut registry, actor, unit()).unwrap();
    assert_int(&result.value, Scalar::from(42u64));
    assert!(result.gas_used > 0 && result.gas_used < QUERY_GAS);
    assert_eq!(registry.actor(&actor).unwrap().root.id(), root);
    assert!(!registry.actor(&actor).unwrap().is_checked_out());
}

#[test]
fn getter_can_inspect_and_restore_a_noncopyable_token() {
    let mut registry = MemRegistry::new();
    let actor = deploy_query(
        &mut registry,
        ScriptBuilder::new()
            .drop_()
            .load()
            .amount()
            .drop_()
            .roll_k(1)
            .save()
            .return_(),
        Value::ClearToken(ClearToken::new(Scalar::from(42u64), FLAME_FLAVOR)),
    );
    let root = registry.actor(&actor).unwrap().root.id();
    assert_int(
        &query(&mut registry, actor, unit()).unwrap().value,
        Scalar::from(42u64),
    );
    assert_eq!(registry.actor(&actor).unwrap().root.id(), root);
    let restored = registry.load_state(&actor).unwrap();
    let Value::ClearToken(token) = restored else {
        panic!("token lost")
    };
    assert_eq!(token.qty, Scalar::from(42u64));
}

#[test]
fn query_rejects_nondata_arguments() {
    let mut registry = MemRegistry::new();
    let actor = deploy_query(&mut registry, ScriptBuilder::new().return_(), unit());
    for arg in [
        Value::ClearToken(ClearToken::new(Scalar::ZERO, FLAME_FLAVOR)),
        Value::ClearToken(ClearToken::new(Scalar::ONE, FLAME_FLAVOR)),
        Value::Slice(crate::Slice::new(Arc::new(Cell::new(vec![], vec![]).unwrap())).unwrap()),
        Value::Dict(Dict::from_values(vec![Value::Dict(Dict::from_values(
            vec![Value::Builder(CellBuilder::new())],
        ))])),
    ] {
        assert!(matches!(
            query(&mut registry, actor, arg),
            Err(VMError::NonDataInQuery)
        ));
        assert!(!registry.actor(&actor).unwrap().is_checked_out());
    }
}

#[test]
fn query_rejects_nondata_results() {
    for program in [
        ScriptBuilder::new().drop_().builder().return_(),
        ScriptBuilder::new()
            .drop_()
            .push_int(FLAME_FLAVOR)
            .pushtoken()
            .push_int(0u64)
            .push_int(1u64)
            .dict()
            .push_int(0u64)
            .push_int(1u64)
            .dict()
            .return_(),
        ScriptBuilder::new()
            .drop_()
            .builder()
            .push_int(0u64)
            .push_int(1u64)
            .dict()
            .return_(),
        ScriptBuilder::new()
            .drop_()
            .push_cell(CellValue::from_bytes(vec![]).unwrap())
            .slice()
            .return_(),
    ] {
        let mut registry = MemRegistry::new();
        let actor = deploy_query(&mut registry, program, unit());
        assert!(matches!(
            query(&mut registry, actor, unit()),
            Err(VMError::NonDataInQuery)
        ));
        let vm = run_actor_call(
            &mut registry,
            actor,
            Value::Scalar(Scalar::from(17u64)),
            true,
        );
        assert_eq!(vm.current_call.stack.len(), 2);
        assert_int(&vm.current_call.stack[0], Scalar::from(17u64));
        assert_int(&vm.current_call.stack[1], Scalar::ZERO);
    }
}

#[test]
fn queries_accept_empty_and_nested_dns_dict_arguments_and_results() {
    let mut records = Dict::new();
    records.insert(
        Scalar::ONE,
        Value::Point(Point::commitment(Commitment::unblinded(Scalar::ONE))),
    );
    records.insert(
        Scalar::from(2u64),
        Value::Cell(CellValue::from_bytes(b"https://alice.example".to_vec()).unwrap()),
    );
    records.insert(
        Scalar::from(3u64),
        Value::Dict(Dict::from_values(vec![Value::Scalar(Scalar::from(
            3600u64,
        ))])),
    );
    let mut registry = MemRegistry::new();
    let actor = deploy_query(&mut registry, ScriptBuilder::new().return_(), unit());
    for arg in [Value::Dict(Dict::new()), Value::Dict(records)] {
        let expected = arg.to_cell().unwrap().id();
        let result = query(&mut registry, actor, arg.clone()).unwrap();
        assert_eq!(result.value.to_cell().unwrap().id(), expected);
        assert!(!result.value.is_copyable());
        assert!(matches!(
            result.value.try_clone(),
            Err(VMError::TypeNotCopyable)
        ));
        let vm = run_actor_call(&mut registry, actor, arg, true);
        assert_eq!(vm.current_call.stack[0].to_cell().unwrap().id(), expected);
        assert_int(&vm.current_call.stack[1], Scalar::ONE);
        for value in [&result.value, &vm.current_call.stack[0]] {
            let Value::Dict(dict) = value else {
                panic!("expected Dict")
            };
            if !dict.is_empty() {
                assert!(matches!(
                    dict.get(&Scalar::ONE),
                    Some(Value::Point(Point::Opaque(_)))
                ));
            }
        }
    }
    let actor = deploy_query(
        &mut registry,
        ScriptBuilder::new().drop_().push_int(0u64).dict().return_(),
        unit(),
    );
    assert!(matches!(
        query(&mut registry, actor, unit()).unwrap().value,
        Value::Dict(dict) if dict.is_empty()
    ));
    let vm = run_actor_call(&mut registry, actor, unit(), true);
    match &vm.current_call.stack[0] {
        Value::Dict(dict) => assert!(dict.is_empty()),
        _ => panic!("expected empty Dict"),
    }
    assert_int(&vm.current_call.stack[1], Scalar::ONE);
}

#[test]
fn query_dict_normalization_preserves_sticky_flags() {
    let mut dict = Dict::new();
    dict.insert(
        Scalar::ZERO,
        Value::ClearToken(ClearToken::new(Scalar::ONE, FLAME_FLAVOR)),
    );
    dict.remove(&Scalar::ZERO).unwrap();
    dict.insert(Scalar::ONE, unit());
    assert!(!dict.is_droppable());
    let expected = dict.to_cell().unwrap().id();
    let mut registry = MemRegistry::new();
    let actor = deploy_query(&mut registry, ScriptBuilder::new().return_(), unit());
    let result = query(&mut registry, actor, Value::Dict(dict)).unwrap();
    let Value::Dict(dict) = result.value else {
        panic!("expected Dict")
    };
    assert!(!dict.is_droppable());
    assert_eq!(dict.to_cell().unwrap().id(), expected);
}

#[test]
fn queries_reject_bearers_hidden_in_lazy_nested_dicts() {
    let mut registry = MemRegistry::new();
    let actor = deploy_query(&mut registry, ScriptBuilder::new().return_(), unit());
    for quantity in [Scalar::ZERO, Scalar::ONE] {
        let dict = Dict::from_values(vec![Value::Dict(Dict::from_values(vec![
            Value::ClearToken(ClearToken::new(quantity, FLAME_FLAVOR)),
        ]))]);
        let full = dict.to_cell().unwrap();
        let cells = Arc::new(CellIndex::collect(Arc::new(full.clone())).unwrap());
        let unloaded = Cell::decode_record_exact(&full.encode_record()).unwrap();
        let lazy = Dict::from_trusted_cell(&unloaded, &mut ()).unwrap();
        assert!(matches!(
            query_actor(
                actor,
                Value::Dict(lazy),
                QUERY_GAS,
                &mut registry,
                &BlockContext { height: 0 },
                cells,
            ),
            Err(VMError::NonDataInQuery)
        ));
        assert!(!registry.actor(&actor).unwrap().is_checked_out());
    }
}

#[test]
fn query_dict_validation_requires_all_branches() {
    let dict = Dict::from_values(vec![unit(), Value::Scalar(Scalar::ONE)]);
    let full = dict.to_cell().unwrap();
    let unloaded = Cell::decode_record_exact(&full.encode_record()).unwrap();
    let pruned = Cell::new(
        full.payload().to_vec(),
        vec![full.refs()[0]
            .as_resident()
            .unwrap()
            .prune(1)
            .unwrap()
            .into()],
    )
    .unwrap();
    let mut registry = MemRegistry::new();
    let actor = deploy_query(&mut registry, ScriptBuilder::new().return_(), unit());
    for cell in [unloaded, pruned] {
        let lazy = Dict::from_trusted_cell(&cell, &mut ()).unwrap();
        assert!(matches!(
            query(&mut registry, actor, Value::Dict(lazy)),
            Err(VMError::Cell(_))
        ));
    }
}

#[test]
fn query_dict_validation_is_metered_for_native_and_lazy_inputs() {
    let dict = Dict::from_values((0..8u64).map(|n| Value::Scalar(Scalar::from(n))).collect());
    let full = dict.to_cell().unwrap();
    let unloaded = Cell::decode_record_exact(&full.encode_record()).unwrap();
    let lazy = Dict::from_trusted_cell(&unloaded, &mut ()).unwrap();
    let cells = Arc::new(CellIndex::collect(Arc::new(full)).unwrap());
    let mut registry = MemRegistry::new();
    let actor = deploy_query(&mut registry, ScriptBuilder::new().return_(), unit());
    let empty_gas = query(&mut registry, actor, Value::Dict(Dict::new()))
        .unwrap()
        .gas_used;
    for dict in [dict, lazy] {
        let value = Value::Dict(dict);
        let run = |registry: &mut MemRegistry, gas_limit| {
            query_actor(
                actor,
                value.clone(),
                gas_limit,
                registry,
                &BlockContext { height: 0 },
                Arc::clone(&cells),
            )
        };
        let gas_used = run(&mut registry, QUERY_GAS).unwrap().gas_used;
        assert!(gas_used > empty_gas);
        assert!(matches!(
            run(&mut registry, gas_used - 1),
            Err(VMError::OutOfGas)
        ));
    }
}

#[test]
fn nested_dict_results_keep_cell_graphs_after_callee_exit() {
    let child = Cell::new(vec![42], vec![]).unwrap();
    let root = Cell::new(vec![7], vec![child.clone().into()]).unwrap();
    let mut registry = MemRegistry::new();
    let actor = deploy_query(
        &mut registry,
        ScriptBuilder::new()
            .drop_()
            .load()
            .dup_k(0)
            .save()
            .push_int(2u64)
            .push_int(1u64)
            .dict()
            .push_int(3u64)
            .push_int(1u64)
            .dict()
            .return_(),
        Value::Cell(root.clone().into()),
    );
    let offchain = query(&mut registry, actor, unit()).unwrap().value;
    let onchain = run_actor_call(&mut registry, actor, unit(), true);
    assert_int(&onchain.current_call.stack[1], Scalar::ONE);
    for value in [&offchain, &onchain.current_call.stack[0]] {
        let Value::Dict(outer) = value else {
            panic!("expected outer Dict")
        };
        let Value::Dict(inner) = outer.get(&Scalar::from(3u64)).unwrap() else {
            panic!("expected inner Dict")
        };
        let Value::Cell(cell) = inner.get(&Scalar::from(2u64)).unwrap() else {
            panic!("expected Cell")
        };
        assert_eq!(cell.id(), root.id());
        assert!(cell.witness().is_none());
        assert_eq!(
            cell.to_cell().unwrap().refs()[0]
                .as_resident()
                .unwrap()
                .id(),
            child.id()
        );
    }
}

#[test]
fn query_rejects_changed_state_and_rolls_back() {
    let mut registry = MemRegistry::new();
    let actor = deploy_query(
        &mut registry,
        ScriptBuilder::new()
            .drop_()
            .load()
            .push_int(99u64)
            .save()
            .save()
            .push_int(1u64)
            .return_(),
        Value::Scalar(Scalar::from(42u64)),
    );
    let original = registry.actor(&actor).unwrap().root.id();
    assert!(matches!(
        query(&mut registry, actor, unit()),
        Err(VMError::QueryStateChanged)
    ));
    assert_eq!(registry.actor(&actor).unwrap().root.id(), original);
    assert!(!registry.actor(&actor).unwrap().is_checked_out());
}

#[test]
fn query_cannot_destroy_an_actor_by_omitting_save() {
    for explicit_return in [false, true] {
        let mut program = ScriptBuilder::new().drop_().load().drop_();
        if explicit_return {
            program = program.push_int(1u64).return_();
        }
        let mut registry = MemRegistry::new();
        let actor = deploy_query(&mut registry, program, unit());
        let original = registry.actor(&actor).unwrap().root.id();
        assert!(matches!(
            query(&mut registry, actor, unit()),
            Err(VMError::QueryStateNotRestored)
        ));
        assert_eq!(registry.actor(&actor).unwrap().root.id(), original);
        assert!(!registry.actor(&actor).unwrap().is_checked_out());
    }
}

#[test]
fn all_effectful_instructions_are_rejected_in_queries() {
    for instruction in [
        Instruction::Send,
        Instruction::Output,
        Instruction::Contract,
        Instruction::Input,
        Instruction::Open,
        Instruction::Signcall,
        Instruction::Signtx,
        Instruction::Fee,
        Instruction::Log,
        Instruction::IssuePub,
        Instruction::IssuePriv,
        Instruction::Retire,
        Instruction::Setcode,
        Instruction::AddStorage,
        Instruction::Anchor,
        Instruction::Timelock,
        Instruction::Version,
    ] {
        let mut program = ScriptBuilder::new().drop_();
        program.push_instr(instruction.clone());
        let mut registry = MemRegistry::new();
        let actor = deploy_query(&mut registry, program, unit());
        assert!(
            matches!(
                query(&mut registry, actor, unit()),
                Err(VMError::ReadOnlyViolation)
            ),
            "effect {:?} was not rejected as read-only",
            instruction,
        );
    }
}

#[test]
fn exec_cannot_escape_readonly_mode() {
    let mut registry = MemRegistry::new();
    let actor = deploy_query(
        &mut registry,
        ScriptBuilder::new()
            .drop_()
            .push_script(ScriptBuilder::new().retire())
            .unwrap()
            .exec(),
        unit(),
    );
    assert!(matches!(
        query(&mut registry, actor, unit()),
        Err(VMError::ReadOnlyViolation)
    ));
}

#[test]
fn ordinary_nested_call_inherits_readonly_permission() {
    let mut registry = MemRegistry::new();
    let target = deploy_query(
        &mut registry,
        ScriptBuilder::new()
            .drop_()
            .load()
            .drop_()
            .push_int(99u64)
            .save()
            .push_int(1u64)
            .return_(),
        Value::Scalar(Scalar::from(42u64)),
    );
    let caller = deploy_query(
        &mut registry,
        ScriptBuilder::new()
            .push_int(50_000u64)
            .push_cell(actor_address(&target))
            .call()
            .roll_k(1)
            .drop_()
            .return_(),
        unit(),
    );
    let root = registry.actor(&target).unwrap().root.id();
    assert_int(
        &query(&mut registry, caller, unit()).unwrap().value,
        Scalar::ZERO,
    );
    assert_eq!(registry.actor(&target).unwrap().root.id(), root);
    assert!(!registry.actor(&target).unwrap().is_checked_out());
}

#[test]
fn writable_call_can_update_state_but_query_cannot() {
    let mut registry = MemRegistry::new();
    let actor = deploy_query(
        &mut registry,
        ScriptBuilder::new()
            .load()
            .drop_()
            .save()
            .push_int(1u64)
            .return_(),
        Value::Scalar(Scalar::from(42u64)),
    );
    let arg = Value::Scalar(Scalar::from(99u64));
    let readonly = run_actor_call(&mut registry, actor, arg.clone(), true);
    assert_int(&readonly.current_call.stack[1], Scalar::ZERO);
    assert_int(
        registry.actor(&actor).unwrap().state.as_ref().unwrap(),
        Scalar::from(42u64),
    );
    assert!(!readonly
        .txlog
        .iter()
        .any(|entry| matches!(entry, TxEntry::ActorSave { .. })));

    let writable = run_actor_call(&mut registry, actor, arg, false);
    assert_int(&writable.current_call.stack[1], Scalar::ONE);
    assert_int(
        registry.actor(&actor).unwrap().state.as_ref().unwrap(),
        Scalar::from(99u64),
    );
    assert!(writable
        .txlog
        .iter()
        .any(|entry| matches!(entry, TxEntry::ActorSave { .. })));
}

#[test]
fn query_returns_data_without_emitting_saves_or_consuming_anchor() {
    let mut registry = MemRegistry::new();
    let actor = deploy_query(
        &mut registry,
        ScriptBuilder::new()
            .drop_()
            .load()
            .dup_k(0)
            .save()
            .return_(),
        Value::Scalar(Scalar::from(42u64)),
    );
    let vm = run_actor_call(&mut registry, actor, unit(), true);
    assert_int(&vm.current_call.stack[0], Scalar::from(42u64));
    assert_int(&vm.current_call.stack[1], Scalar::ONE);
    assert!(vm.current_call.readwrite);
    assert_eq!(vm.current_call.anchor, Some(Anchor([0x55; 32])));
    assert!(!vm
        .txlog
        .iter()
        .any(|entry| matches!(entry, TxEntry::ActorSave { .. })));
}

#[test]
fn offchain_context_has_actor_identity_height_and_no_caller_or_anchor() {
    for (program, expected) in [
        (ScriptBuilder::new().drop_().height().return_(), None),
        (
            ScriptBuilder::new().drop_().callerid().return_(),
            Some([0; 32]),
        ),
    ] {
        let mut registry = MemRegistry::new();
        let actor = deploy_query(&mut registry, program, unit());
        let result = query(&mut registry, actor, unit()).unwrap();
        if let Some(bytes) = expected {
            assert_bytes(&result.value, &bytes);
        } else {
            assert_int(&result.value, Scalar::from(123u64));
        }
    }
    let mut registry = MemRegistry::new();
    let actor = deploy_query(
        &mut registry,
        ScriptBuilder::new().drop_().selfid().return_(),
        unit(),
    );
    assert_bytes(
        &query(&mut registry, actor, unit()).unwrap().value,
        &actor.to_hash(),
    );
    let actor = deploy_query(
        &mut registry,
        ScriptBuilder::new().drop_().anchor().return_(),
        unit(),
    );
    assert!(matches!(
        query(&mut registry, actor, unit()),
        Err(VMError::ReadOnlyViolation)
    ));
}

#[test]
fn onchain_query_exposes_actual_caller_identity() {
    let mut registry = MemRegistry::new();
    let actor = deploy_query(
        &mut registry,
        ScriptBuilder::new().drop_().callerid().return_(),
        unit(),
    );
    let vm = run_actor_call(&mut registry, actor, unit(), true);
    assert_bytes(&vm.current_call.stack[0], &[0x77; 32]);
    assert_int(&vm.current_call.stack[1], Scalar::ONE);
}

#[test]
fn query_cannot_read_a_checked_out_actor() {
    let mut registry = MemRegistry::new();
    let actor = deploy_query(
        &mut registry,
        ScriptBuilder::new()
            .drop_()
            .load()
            .dup_k(0)
            .save()
            .return_(),
        unit(),
    );
    let state = registry.load_state(&actor).unwrap();
    assert!(matches!(
        query(&mut registry, actor, unit()),
        Err(VMError::ActorEmpty)
    ));
    assert!(registry.actor(&actor).unwrap().is_checked_out());
    registry.save_state(&actor, state).unwrap();
}

#[test]
fn exhausted_query_restores_loaded_state() {
    let mut registry = MemRegistry::new();
    let actor = deploy_query(
        &mut registry,
        ScriptBuilder::new().drop_().load().build_loop(|s| s.nop()),
        unit(),
    );
    let root = registry.actor(&actor).unwrap().root.id();
    assert!(matches!(
        query(&mut registry, actor, unit()),
        Err(VMError::OutOfGas)
    ));
    assert_eq!(registry.actor(&actor).unwrap().root.id(), root);
    assert!(!registry.actor(&actor).unwrap().is_checked_out());
    assert!(matches!(
        query_actor(
            actor,
            unit(),
            0,
            &mut registry,
            &BlockContext { height: 0 },
            Arc::new(CellIndex::new())
        ),
        Err(VMError::OutOfGas)
    ));
}

#[test]
fn query_cell_results_keep_their_graph_after_callee_exit() {
    let child = Cell::new(vec![42], vec![]).unwrap();
    let root = Cell::new(vec![7], vec![child.clone().into()]).unwrap();
    let mut registry = MemRegistry::new();
    let actor = deploy_query(
        &mut registry,
        ScriptBuilder::new()
            .drop_()
            .load()
            .dup_k(0)
            .save()
            .return_(),
        Value::Cell(root.clone().into()),
    );
    let value = query(&mut registry, actor, unit()).unwrap().value;
    let Value::Cell(value) = value else {
        panic!("expected Cell")
    };
    assert_eq!(value.id(), root.id());
    assert!(value.witness().is_none());
    let resident = value.to_cell().unwrap();
    assert_eq!(resident.refs()[0].as_resident().unwrap().payload(), &[42]);
    let vm = run_actor_call(&mut registry, actor, unit(), true);
    let Value::Cell(value) = &vm.current_call.stack[0] else {
        panic!("expected Cell")
    };
    assert_eq!(
        value.to_cell().unwrap().refs()[0]
            .as_resident()
            .unwrap()
            .id(),
        child.id()
    );
}

#[test]
fn query_resolves_unloaded_argument_from_explicit_cells() {
    let cell = Cell::new(vec![42], vec![]).unwrap();
    let arg = Value::Cell(CellRef::from(cell.clone()).to_unloaded().into());
    let mut registry = MemRegistry::new();
    let actor = deploy_query(&mut registry, ScriptBuilder::new().return_(), unit());
    assert!(matches!(
        query(&mut registry, actor, arg.clone()),
        Err(VMError::Cell(CellError::MissingCell(_)))
    ));
    let mut cells = CellIndex::new();
    cells.insert(Arc::new(cell)).unwrap();
    let result = query_actor(
        actor,
        arg,
        QUERY_GAS,
        &mut registry,
        &BlockContext { height: 0 },
        Arc::new(cells),
    )
    .unwrap();
    assert_bytes(&result.value, &[42]);
}

#[test]
fn query_failure_preserves_the_surrounding_transaction_checkpoint() {
    let mut registry = MemRegistry::new();
    let actor = deploy_query(
        &mut registry,
        ScriptBuilder::new()
            .drop_()
            .load()
            .drop_()
            .push_int(99u64)
            .save(),
        unit(),
    );
    let original = registry.actor(&actor).unwrap().root.id();
    registry.push_checkpoint();
    registry.load_state(&actor).unwrap();
    registry
        .save_state(&actor, Value::Scalar(Scalar::from(17u64)))
        .unwrap();
    assert!(matches!(
        query(&mut registry, actor, unit()),
        Err(VMError::QueryStateChanged)
    ));
    assert_int(
        registry.actor(&actor).unwrap().state.as_ref().unwrap(),
        Scalar::from(17u64),
    );
    registry.pop_checkpoint_rollback();
    assert_eq!(registry.actor(&actor).unwrap().root.id(), original);
}

#[test]
fn query_success_sees_and_preserves_prior_transaction_writes() {
    let mut registry = MemRegistry::new();
    let actor = deploy_query(
        &mut registry,
        ScriptBuilder::new()
            .drop_()
            .load()
            .dup_k(0)
            .save()
            .return_(),
        unit(),
    );
    let original = registry.actor(&actor).unwrap().root.id();
    registry.push_checkpoint();
    registry.load_state(&actor).unwrap();
    registry
        .save_state(&actor, Value::Scalar(Scalar::from(17u64)))
        .unwrap();
    let vm = run_actor_call(&mut registry, actor, unit(), true);
    assert_int(&vm.current_call.stack[0], Scalar::from(17u64));
    assert_int(&vm.current_call.stack[1], Scalar::ONE);
    assert_int(
        registry.actor(&actor).unwrap().state.as_ref().unwrap(),
        Scalar::from(17u64),
    );
    registry.pop_checkpoint_rollback();
    assert_eq!(registry.actor(&actor).unwrap().root.id(), original);
}

#[test]
fn nested_query_preserves_the_loaded_parents_reentrancy_lock() {
    let a = ActorID::from([0xa; 32]);
    let b = ActorID::from([0xb; 32]);
    let mut registry = MemRegistry::new();
    let b_code = ScriptBuilder::new()
        .push_int(20_000u64)
        .push_cell(actor_address(&a))
        .query()
        .roll_k(1)
        .drop_()
        .return_()
        .to_cell()
        .unwrap();
    let a_code = ScriptBuilder::new()
        .drop_()
        .load()
        .push_int(7u64)
        .push_int(50_000u64)
        .push_cell(actor_address(&b))
        .query()
        .verify()
        .roll_k(1)
        .save()
        .return_()
        .to_cell()
        .unwrap();
    registry.deploy(a, a_code, unit(), 1_000_000).unwrap();
    registry.deploy(b, b_code, unit(), 1_000_000).unwrap();
    assert_int(
        &query(&mut registry, a, unit()).unwrap().value,
        Scalar::ZERO,
    );
    assert!(!registry.actor(&a).unwrap().is_checked_out());
    assert!(!registry.actor(&b).unwrap().is_checked_out());
}

#[test]
fn query_does_not_initialize_or_restore_addresses() {
    let mut registry = MemRegistry::new();
    let caller = ActorID::from([0x77; 32]);
    registry
        .deploy(
            caller,
            ScriptBuilder::new().nop().to_cell().unwrap(),
            unit(),
            1_000_000,
        )
        .unwrap();
    let code = ScriptBuilder::new().return_().to_cell().unwrap();
    let target = ActorID::from(code.id());
    let root = Cell::new(
        vec![],
        vec![code.clone().into(), unit().to_cell().unwrap().into()],
    )
    .unwrap();
    for address in [
        ActorAddress::Init { ctor: code.into() },
        ActorAddress::Restore {
            addr: target,
            state: root.into(),
        },
    ] {
        let mut vm = VM::new(
            dummy_header(),
            CallFrame::new(
                ScriptBuilder::new()
                    .push_int(QUERY_GAS)
                    .push_cell(address.to_cell().unwrap())
                    .query()
                    .into_instructions(),
                CallKind::InternalRoot {
                    actor: caller,
                    caller: None,
                },
                1_000_000,
            )
            .with_readwrite(true),
        );
        vm.push_value(Value::Scalar(Scalar::from(17u64)));
        while !vm.current_call.is_finished() {
            vm.step_internal_with_registry(&mut registry).unwrap();
        }
        assert_int(&vm.current_call.stack[0], Scalar::from(17u64));
        assert_int(&vm.current_call.stack[1], Scalar::ZERO);
        assert!(registry.actor(&target).is_none());
    }
}

#[test]
fn failed_query_preserves_parent_effects_and_anchor() {
    let mut registry = MemRegistry::new();
    let target = deploy_query(
        &mut registry,
        ScriptBuilder::new().drop_().load().drop_(),
        unit(),
    );
    let caller = ActorID::from([0x77; 32]);
    registry
        .deploy(
            caller,
            ScriptBuilder::new().nop().to_cell().unwrap(),
            unit(),
            1_000_000,
        )
        .unwrap();
    let program = ScriptBuilder::new()
        .push_int(17u64)
        .push_int(QUERY_GAS)
        .push_cell(actor_address(&target))
        .query();
    let mut vm = VM::new(
        dummy_header(),
        CallFrame::new(
            program.into_instructions(),
            CallKind::InternalRoot {
                actor: caller,
                caller: None,
            },
            1_000_000,
        )
        .with_readwrite(true)
        .with_anchor(Anchor([0x55; 32])),
    );
    vm.txlog.push(TxEntry::Data(vec![1, 2, 3]));
    let effects = TxID::from_log(&vm.txlog);
    let initial_anchor = vm.last_anchor;
    while !vm.call_stack.is_empty() || !vm.current_call.is_finished() {
        vm.step_internal_with_registry(&mut registry).unwrap();
    }
    assert_eq!(TxID::from_log(&vm.txlog), effects);
    assert_eq!(vm.last_anchor, initial_anchor);
    assert_eq!(vm.current_call.anchor, Some(Anchor([0x55; 32])));
    assert_int(&vm.current_call.stack[0], Scalar::from(17u64));
    assert_int(&vm.current_call.stack[1], Scalar::ZERO);
    assert!(!registry.actor(&target).unwrap().is_checked_out());
}
