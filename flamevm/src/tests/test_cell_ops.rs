//! Builder/Slice byte and reference operations.
use super::test_helpers::*;

fn bytes(data: &[u8]) -> Cell {
    Cell::new(data.to_vec(), vec![]).unwrap()
}

fn builder_value(data: &[u8], refs: &[CellRef]) -> Value {
    let mut builder = CellBuilder::new();
    builder.store_bytes(data).unwrap();
    for reference in refs {
        builder.store_ref(reference.clone()).unwrap();
    }
    Value::Builder(builder)
}

fn operation(instruction: Instruction, stack: Vec<Value>) -> VM {
    let mut program = ScriptBuilder::new();
    program.push_instr(instruction);
    let mut vm = vm_with_script(program);
    vm.current_call.stack = stack;
    vm
}

fn slice_value(cell: Cell) -> Value {
    Value::Slice(crate::Slice::new(Arc::new(cell)).unwrap())
}

#[test]
fn writers_build_dynamic_cell_graphs_and_roundtrip_public_point_bytes() {
    let point = [0x55; 32]; // Opaque Point bytes remain lazily validated, as before.
    let program = ScriptBuilder::new()
        .builder()
        .builder()
        .push_int(42u64)
        .write_scalar()
        .endcell()
        .write_ref()
        .push_point(point)
        .write_point()
        .endcell()
        .slice()
        .read_point()
        .verify()
        .push_point(point)
        .eq()
        .verify()
        .drop_()
        .drop_()
        .push_int(0u64)
        .load_ref()
        .slice()
        .read_scalar()
        .verify()
        .push_int(42u64)
        .eq()
        .verify()
        .drop_()
        .drop_()
        .drop_()
        .drop_();
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
    for (instruction, byte) in [
        (Instruction::WriteRef, 0xad),
        (Instruction::WritePoint, 0xae),
    ] {
        assert_eq!(instruction.encode_to_vec(), vec![byte]);
        assert_eq!(
            Instruction::parse(&mut &[byte][..])
                .unwrap()
                .encode_to_vec(),
            vec![byte]
        );
    }
}

#[test]
fn writeref_accepts_unloaded_and_pruned_refs_without_reading_and_enforces_limits() {
    let child = bytes(&[9]);
    let refs = [
        CellRef::from(child.clone()).to_unloaded(),
        child.prune(1).unwrap().into(),
    ];
    for reference in refs {
        let mut vm = operation(
            Instruction::WriteRef,
            vec![
                builder_value(&[7], &[]),
                Value::Cell(reference.clone().into()),
            ],
        );
        run_to_end(&mut vm).unwrap();
        let Value::Builder(builder) = &vm.current_call.stack[0] else {
            panic!()
        };
        assert_eq!(builder.payload(), &[7]);
        assert_eq!(builder.refs()[0].id(), reference.id());
        assert_eq!(vm.current_call.gas_used, 2);
    }
    let four = vec![CellRef::from(child.clone()); 4];
    assert!(matches!(
        run_to_end(&mut operation(
            Instruction::WriteRef,
            vec![builder_value(&[], &four), Value::Cell(child.clone().into())]
        )),
        Err(VMError::Cell(CellError::ReferenceCapacity))
    ));
    let depth = cells::CellCommitment::new(0, vec![[9; 32]], vec![u16::MAX]).unwrap();
    assert!(matches!(
        run_to_end(&mut operation(
            Instruction::WriteRef,
            vec![
                builder_value(&[], &[]),
                Value::Cell(CellRef::Unloaded(Arc::new(depth)).into())
            ]
        )),
        Err(VMError::Cell(CellError::DepthOverflow))
    ));
    for bad in [
        Value::Scalar(Scalar::ONE),
        Value::ClearToken(ClearToken::new(Scalar::ONE, FLAME_FLAVOR)),
    ] {
        assert!(matches!(
            run_to_end(&mut operation(
                Instruction::WriteRef,
                vec![builder_value(&[], &[]), bad]
            )),
            Err(VMError::TypeNotCell)
        ));
    }
    let mut vm = operation(
        Instruction::WriteRef,
        vec![builder_value(&[], &[]), Value::Cell(child.into())],
    );
    vm.current_call.gas_limit = 1;
    assert!(matches!(run_to_end(&mut vm), Err(VMError::OutOfGas)));
}

#[test]
fn point_reads_advance_only_bytes_and_writes_preserve_refs_and_check_capacity() {
    let reference: CellRef = bytes(&[9]).into();
    for count in [0, 31, 32, 33] {
        let cell = Cell::new(vec![0x55; count], vec![reference.clone()]).unwrap();
        let mut vm = operation(Instruction::ReadPoint, vec![slice_value(cell)]);
        run_to_end(&mut vm).unwrap();
        let Value::Slice(slice) = &vm.current_call.stack[0] else {
            panic!()
        };
        let success = count >= 32;
        assert_eq!(
            slice.remaining_bytes(),
            count - if success { 32 } else { 0 }
        );
        assert_eq!(slice.remaining_refs(), 1);
        assert_int(
            vm.current_call.stack.last().unwrap(),
            Scalar::from(success as u64),
        );
        assert_eq!(vm.current_call.gas_used, 1);
    }
    let mut vm = operation(
        Instruction::WritePoint,
        vec![
            builder_value(&[9], &[reference]),
            Value::Point(Point::from_bytes([0x55; 32])),
        ],
    );
    run_to_end(&mut vm).unwrap();
    let Value::Builder(builder) = &vm.current_call.stack[0] else {
        panic!()
    };
    assert_eq!(builder.used_refs(), 1);
    assert_eq!(builder.payload(), &[vec![9], vec![0x55; 32]].concat());
    assert_eq!(vm.current_call.gas_used, 33);
    let full = vec![0; cells::MAX_CELL_PAYLOAD - 31];
    assert!(matches!(
        run_to_end(&mut operation(
            Instruction::WritePoint,
            vec![
                builder_value(&full, &[]),
                Value::Point(Point::from_bytes([0; 32]))
            ]
        )),
        Err(VMError::Cell(CellError::PayloadCapacity))
    ));
    assert!(matches!(
        run_to_end(&mut operation(
            Instruction::ReadPoint,
            vec![Value::Cell(bytes(&[0; 32]).into())]
        )),
        Err(VMError::TypeNotSlice)
    ));
}

#[test]
fn transcripts_accept_byte_containers_and_append_challenges_without_losing_refs() {
    let reference: CellRef = bytes(&[9]).into();
    for count in [0, 32, cells::MAX_CELL_PAYLOAD - 1] {
        let mut expected = Merlin::new(b"protocol");
        expected.write_bytes(b"data", b"message");
        let challenge = expected.read_bytes(b"challenge", count);
        let mut vm = vm_with_script(ScriptBuilder::new().transcript().twrite().tread());
        vm.current_call.stack = vec![Value::Cell(bytes(b"protocol").into())];
        vm.step_internal().unwrap();
        vm.current_call.stack.push(slice_value(bytes(b"data")));
        vm.current_call.stack.push(builder_value(b"message", &[]));
        vm.step_internal().unwrap();
        vm.current_call.stack.push(builder_value(b"challenge", &[]));
        vm.current_call
            .stack
            .push(builder_value(&[7], &[reference.clone()]));
        vm.current_call
            .stack
            .push(Value::Scalar(Scalar::from(count as u64)));
        vm.step_internal().unwrap();
        let Value::Builder(builder) = vm.current_call.stack.pop().unwrap() else {
            panic!()
        };
        assert_eq!(&builder.payload()[1..], challenge);
        assert_eq!(builder.payload()[0], 7);
        assert_eq!(builder.refs()[0].id(), reference.id());
        let Value::Merlin(mut actual) = vm.current_call.stack.pop().unwrap() else {
            panic!()
        };
        assert_eq!(
            actual.read_bytes(b"next", 32),
            expected.read_bytes(b"next", 32)
        );
    }
    let label = builder_value(b"challenge", &[]);
    assert!(matches!(
        run_to_end(&mut operation(
            Instruction::TRead,
            vec![
                Value::Merlin(Merlin::new(b"protocol")),
                label,
                builder_value(&vec![0; cells::MAX_CELL_PAYLOAD], &[]),
                Value::Scalar(Scalar::ONE)
            ]
        )),
        Err(VMError::Cell(CellError::PayloadCapacity))
    ));
    let mut vm = operation(
        Instruction::TRead,
        vec![
            Value::Merlin(Merlin::new(b"protocol")),
            builder_value(b"challenge", &[]),
            builder_value(&[], &[]),
            Value::Scalar(Scalar::from(32u64)),
        ],
    );
    vm.current_call.gas_limit = 32;
    assert!(matches!(run_to_end(&mut vm), Err(VMError::OutOfGas)));
}

#[test]
fn remaining_plain_byte_consumers_reject_graphs_and_non_byte_values() {
    let graph = Cell::new(vec![0; 32], vec![bytes(&[9]).into()]).unwrap();
    for instruction in [
        Instruction::Transcript,
        Instruction::Mod252,
        Instruction::Log,
    ] {
        for source in [
            Value::Cell(graph.clone().into()),
            slice_value(graph.clone()),
            builder_value(graph.payload(), graph.refs()),
        ] {
            assert!(matches!(
                run_to_end(&mut operation(instruction.clone(), vec![source])),
                Err(VMError::Cell(CellError::InvalidFormat))
            ));
        }
        assert!(matches!(
            run_to_end(&mut operation(
                instruction,
                vec![Value::Scalar(Scalar::ZERO)]
            )),
            Err(VMError::TypeNotByteSource)
        ));
    }
    let mut vm = operation(
        Instruction::WriteZeros,
        vec![
            builder_value(&vec![9; cells::MAX_CELL_PAYLOAD - 1], &[]),
            Value::Scalar(Scalar::ONE),
        ],
    );
    run_to_end(&mut vm).unwrap();
    let Value::Builder(builder) = &vm.current_call.stack[0] else {
        panic!()
    };
    assert_eq!(builder.payload().last(), Some(&0));
    assert!(matches!(
        run_to_end(&mut operation(
            Instruction::WriteZeros,
            vec![
                builder_value(&vec![0; cells::MAX_CELL_PAYLOAD], &[]),
                Value::Scalar(Scalar::ONE)
            ]
        )),
        Err(VMError::Cell(CellError::PayloadCapacity))
    ));
}

#[test]
fn forged_private_commitment_annotations_cannot_replace_public_bytes() {
    let literal = CellValue::commitment(Commitment::unblinded(Scalar::from(7u64)));
    let forged = crate::CellValue {
        reference: bytes(&[9; 32]).into(),
        witness: literal.witness.clone(),
    };
    let mut vm = vm_external_with_script(ScriptBuilder::new().commit());
    vm.current_call.stack = vec![Value::Cell(forged)];
    assert!(matches!(
        vm.step_external(&mut StubDelegate::new()),
        Err(VMError::Cell(CellError::CellCommitmentMismatch(_)))
    ));
}

#[test]
fn unrelated_annotations_do_not_change_public_commitment_parsing_or_gas() {
    let mut results = Vec::new();
    for literal in [
        CellValue::scalar(Scalar::ZERO),
        CellValue::point(Point::from_bytes([0; 32])),
        CellValue::predicate(Predicate::opaque(CompressedRistretto([0; 32]))),
    ] {
        let tx = ScriptBuilder::new()
            .push_literal(literal)
            .commit()
            .drop_()
            .build_tx(dummy_header(), crate::Limits { gas: 1_000_000 })
            .unwrap();
        results.push((tx.txid(), tx.metrics()));
        tx.without_signature()
            .unwrap()
            .verify(crate::Limits { gas: 1_000_000 })
            .unwrap();
    }
    assert!(results.windows(2).all(|pair| pair[0] == pair[1]));
}

#[test]
fn runslice_snapshots_after_instruction_with_independent_byte_and_ref_cursors() {
    let children = [bytes(&[1]), bytes(&[2]), bytes(&[3])];
    let program = ScriptBuilder::new()
        .push_int(300u64)
        .drop_()
        .push_cell(children[0].clone())
        .drop_()
        .run_slice()
        .dup_k(0)
        .push_int(1u64)
        .load_ref()
        .push_cell(children[1].clone())
        .push_cell(children[2].clone());
    let mut vm = vm_with_script(program);
    for _ in 0..5 {
        vm.step_internal().unwrap();
    }
    let snapshot_cursor = vm.current_call.cursor;
    let Value::Slice(snapshot) = &vm.current_call.stack[0] else {
        panic!()
    };
    let root = &vm.current_call.program.as_ref().unwrap().root;
    assert_eq!(
        snapshot.bytes().as_ptr(),
        root.payload()[snapshot_cursor..].as_ptr()
    );
    assert_eq!(
        snapshot.refs().map(CellRef::id).collect::<Vec<_>>(),
        children[1..].iter().map(Cell::id).collect::<Vec<_>>()
    );
    assert!(!Value::Slice(snapshot.clone()).is_portable());
    run_to_end(&mut vm).unwrap();
    let Value::Slice(original) = &vm.current_call.stack[0] else {
        panic!()
    };
    let Value::Slice(copy) = &vm.current_call.stack[1] else {
        panic!()
    };
    assert_eq!(original.bytes(), copy.bytes());
    assert_eq!(original.remaining_refs(), 2);
    assert_eq!(copy.remaining_refs(), 1);
    assert_eq!(copy.refs().next().unwrap().id(), children[1].id());
    // loadref on the copy did not consume the actual program's refs.
    for (actual, expected) in vm.current_call.stack[2..].iter().zip([2, 1, 2]) {
        let Value::Cell(cell) = actual else { panic!() };
        assert_eq!(cell.id(), children[expected].id());
    }
    assert_eq!(
        original.bytes().as_ptr(),
        vm.current_call.program.as_ref().unwrap().root.payload()[snapshot_cursor..].as_ptr()
    );
}

#[test]
fn runslice_uses_current_cell_after_exec_and_supports_empty_tail() {
    let next = ScriptBuilder::new().run_slice().to_cell().unwrap();
    let mut vm = vm_with_script(
        ScriptBuilder::new()
            .run_slice()
            .push_cell(next.clone())
            .exec(),
    );
    run_to_end(&mut vm).unwrap();
    let Value::Slice(before) = &vm.current_call.stack[0] else {
        panic!()
    };
    let Value::Slice(after) = &vm.current_call.stack[1] else {
        panic!()
    };
    assert_eq!(before.refs().next().unwrap().id(), next.id());
    assert_eq!(before.bytes(), &[0xa6, 0xa7]);
    assert_eq!(after.remaining_bytes(), 0);
    assert_eq!(after.remaining_refs(), 0);
    let mut encoded = Vec::new();
    Instruction::RunSlice.encode(&mut encoded);
    assert_eq!(encoded, &[0xac]);
    assert!(matches!(
        Instruction::parse(&mut encoded.as_slice()),
        Ok(Instruction::RunSlice)
    ));
}

#[test]
fn runslice_after_forward_jump_retains_skipped_references_at_empty_byte_tail() {
    let child = bytes(&[9]);
    let mut vm = vm_with_script(
        ScriptBuilder::new()
            .jump(0)
            .push_cell(child.clone())
            .drop_()
            .label(0)
            .run_slice(),
    );
    run_to_end(&mut vm).unwrap();
    let Value::Slice(slice) = &vm.current_call.stack[0] else {
        panic!()
    };
    assert_eq!(slice.remaining_bytes(), 0);
    assert_eq!(slice.remaining_refs(), 1);
    assert_eq!(slice.refs().next().unwrap().id(), child.id());
    assert_eq!(vm.current_call.gas_used, 5); // jump + three scanned instructions + runslice
}

#[test]
fn cellinfo_peeks_factual_record_stats_for_ordinary_and_pruned_cells() {
    let leaf = bytes(&[9]);
    let branch = Cell::new(vec![], vec![leaf.clone().into()]).unwrap();
    let partial = Cell::new(vec![1, 2], vec![branch.prune(1).unwrap().into()]).unwrap();
    assert_ne!(
        partial.depth(0).unwrap(),
        partial.depth(partial.level()).unwrap()
    );
    let sparse = partial.prune(4).unwrap();
    assert_eq!(sparse.level_mask().count_ones(), 2);
    for (cell, expected) in [
        (bytes(&[]), [0, 0, 0, 0, 0]),
        (
            Cell::new(vec![0; cells::MAX_CELL_PAYLOAD], vec![branch.into(); 4]).unwrap(),
            [4095, 4, 2, 0, 0],
        ),
        (partial, [2, 1, 1, 1, 0]),
        (sparse, [68, 0, 0, 4, 1]),
        (leaf.prune(15).unwrap(), [34, 0, 0, 15, 1]),
    ] {
        let id = cell.id();
        let mut vm = operation(Instruction::CellInfo, vec![Value::Cell(cell.into())]);
        run_to_end(&mut vm).unwrap();
        assert_eq!(vm.current_call.stack.len(), 6);
        let Value::Cell(original) = &vm.current_call.stack[0] else {
            panic!()
        };
        assert_eq!(original.id(), id);
        for (actual, expected) in vm.current_call.stack[1..].iter().zip(expected) {
            assert_int(actual, Scalar::from(expected as u64));
        }
    }
}

#[test]
fn cellinfo_resolves_only_its_own_record_with_equal_resident_and_unloaded_gas() {
    let child: CellRef = bytes(&[9; 100]).into();
    let root = Cell::new(vec![1, 2], vec![child.to_unloaded()]).unwrap();
    let index = CellIndex::collect(Arc::new(root.clone())).unwrap();
    assert!(index.get(&child.id()).is_none());
    let reference: CellRef = root.clone().into();
    for reference in [reference.clone(), reference.to_unloaded()] {
        let unloaded = reference.is_unloaded();
        let mut vm = operation(Instruction::CellInfo, vec![Value::Cell(reference.into())])
            .with_cells(Arc::new(index.clone()))
            .unwrap();
        run_to_end(&mut vm).unwrap();
        assert_eq!(vm.current_call.gas_used, 2 + root.record_size() as u64 + 1);
        assert_int(&vm.current_call.stack[1], Scalar::from(2u64));
        assert_int(&vm.current_call.stack[2], Scalar::ONE);
        assert_int(&vm.current_call.stack[3], Scalar::ONE);
        let Value::Cell(original) = &vm.current_call.stack[0] else {
            panic!()
        };
        assert_eq!(original.is_unloaded(), unloaded);
    }
    let mut missing = operation(
        Instruction::CellInfo,
        vec![Value::Cell(reference.to_unloaded().into())],
    );
    assert!(matches!(
        run_to_end(&mut missing),
        Err(VMError::Cell(CellError::MissingCell(_)))
    ));
    assert_eq!(missing.current_call.stack.len(), 1);

    let forged = cells::CellCommitment::new(0, vec![root.id()], vec![2]).unwrap();
    let mut mismatch = operation(
        Instruction::CellInfo,
        vec![Value::Cell(CellRef::Unloaded(Arc::new(forged)).into())],
    )
    .with_cells(Arc::new(index))
    .unwrap();
    assert!(matches!(
        run_to_end(&mut mismatch),
        Err(VMError::Cell(CellError::CellCommitmentMismatch(_)))
    ));
    assert_eq!(mismatch.current_call.stack.len(), 1);

    let mut exhausted = operation(Instruction::CellInfo, vec![Value::Cell(root.into())]);
    exhausted.current_call.gas_limit = 2;
    assert!(matches!(run_to_end(&mut exhausted), Err(VMError::OutOfGas)));
    assert_eq!(exhausted.current_call.stack.len(), 1);
}

#[test]
fn cellinfo_rejects_other_types_and_matches_public_verifier_stats_and_gas() {
    assert!(matches!(
        run_to_end(&mut operation(Instruction::CellInfo, vec![])),
        Err(VMError::StackUnderflow)
    ));
    for value in [
        Value::Scalar(Scalar::ZERO),
        slice_value(bytes(&[])),
        builder_value(&[], &[]),
    ] {
        let mut vm = operation(Instruction::CellInfo, vec![value]);
        assert!(matches!(run_to_end(&mut vm), Err(VMError::TypeNotCell)));
        assert_eq!(vm.current_call.stack.len(), 1);
    }
    let mut program = ScriptBuilder::new();
    for (cell, expected) in [
        (bytes(&[1, 2]), [2, 0, 0, 0, 0]),
        (bytes(&[9]).prune(4).unwrap(), [34, 0, 0, 4, 1]),
    ] {
        program = program.push_cell(cell).cell_info();
        for value in expected.iter().rev() {
            program = program
                .push_int(*value as u64)
                .eq()
                .verify()
                .drop_()
                .drop_();
        }
        program = program.drop_();
    }
    let unsigned = program
        .build_tx(dummy_header(), crate::Limits { gas: 1_000_000 })
        .unwrap();
    let gas = unsigned.metrics().gas_used;
    let tx = unsigned.without_signature().unwrap();
    let decoded =
        crate::ExternalTx::from_bytes_bounded(&tx.to_bytes().unwrap(), 1, 4095, 2497).unwrap();
    let (_, metrics) = decoded
        .verify_with_metrics(crate::Limits { gas: 1_000_000 })
        .unwrap();
    assert_eq!(metrics.gas_used, gas);
}

#[test]
fn loadref_removes_any_remaining_ref_without_loading_or_advancing_bytes() {
    let child = bytes(&[9]);
    let unloaded = CellRef::from(child.clone()).to_unloaded();
    let pruned: CellRef = child.prune(1).unwrap().into();
    let source = Cell::new(
        vec![1, 2],
        vec![unloaded.clone(), pruned.clone(), child.clone().into()],
    )
    .unwrap();
    let mut vm = operation(
        Instruction::LoadRef,
        vec![slice_value(source), Value::Scalar(Scalar::ONE)],
    );
    run_to_end(&mut vm).unwrap();
    let Value::Slice(slice) = &vm.current_call.stack[0] else {
        panic!()
    };
    assert_eq!(slice.bytes(), &[1, 2]);
    assert_eq!(
        slice.refs().map(CellRef::id).collect::<Vec<_>>(),
        vec![unloaded.id(), child.id()]
    );
    let Value::Cell(loaded) = &vm.current_call.stack[1] else {
        panic!()
    };
    assert_eq!(loaded.id(), pruned.id());
    assert!(loaded.as_resident().unwrap().is_pruned());
    assert!(matches!(
        run_to_end(&mut operation(
            Instruction::Slice,
            vec![Value::Cell(loaded.clone())]
        )),
        Err(VMError::Cell(CellError::PrunedCell))
    ));

    let mut vm = operation(
        Instruction::LoadRef,
        vec![
            slice_value(Cell::new(vec![], vec![unloaded.clone()]).unwrap()),
            Value::Scalar(Scalar::ZERO),
        ],
    );
    run_to_end(&mut vm).unwrap();
    let Value::Cell(loaded) = &vm.current_call.stack[1] else {
        panic!()
    };
    assert!(loaded.reference.is_unloaded());
    assert_eq!(loaded.id(), unloaded.id());
}

#[test]
fn loadref_checks_types_indexes_and_instruction_gas() {
    for (count, index) in [
        (0, Scalar::ZERO),
        (1, Scalar::ONE),
        (4, Scalar::from(4u64)),
        (1, Scalar::from(-1i64)),
        (1, Scalar::from(u64::MAX)),
    ] {
        let refs = vec![bytes(&[]).into(); count];
        let mut vm = operation(
            Instruction::LoadRef,
            vec![
                slice_value(Cell::new(vec![], refs).unwrap()),
                Value::Scalar(index),
            ],
        );
        assert!(matches!(run_to_end(&mut vm), Err(VMError::IndexOutOfRange)));
    }
    assert!(matches!(
        run_to_end(&mut operation(
            Instruction::LoadRef,
            vec![Value::Scalar(Scalar::ZERO), Value::Scalar(Scalar::ZERO)]
        )),
        Err(VMError::TypeNotSlice)
    ));
    assert!(matches!(
        run_to_end(&mut operation(
            Instruction::LoadRef,
            vec![slice_value(bytes(&[])), Value::Cell(bytes(&[]).into())]
        )),
        Err(VMError::TypeNotScalar)
    ));
    assert!(matches!(
        run_to_end(&mut operation(Instruction::LoadRef, vec![])),
        Err(VMError::StackUnderflow)
    ));
    let mut vm = operation(
        Instruction::LoadRef,
        vec![
            slice_value(Cell::new(vec![1], vec![bytes(&[9]).into()]).unwrap()),
            Value::Scalar(Scalar::ZERO),
        ],
    );
    vm.current_call.gas_limit = 0;
    assert!(matches!(run_to_end(&mut vm), Err(VMError::OutOfGas)));
    assert_eq!(vm.current_call.stack.len(), 2);
    let Value::Slice(slice) = &vm.current_call.stack[0] else {
        panic!()
    };
    assert_eq!(slice.remaining_refs(), 1);
}

#[test]
fn loadref_composes_with_reads_appends_and_public_verification() {
    let a: CellRef = bytes(&[7]).into();
    let b: CellRef = bytes(&[8]).into();
    let c: CellRef = bytes(&[9]).into();
    let root = Cell::new(vec![5, 6], vec![a.clone(), b, c.clone()]).unwrap();
    let expected = Cell::new(vec![6], vec![a, c]).unwrap();
    let program = ScriptBuilder::new()
        .push_cell(root)
        .slice()
        .push_int(1u64)
        .read_uint()
        .verify()
        .drop_()
        .push_int(1u64)
        .load_ref()
        .slice()
        .push_int(1u64)
        .read_uint()
        .verify()
        .push_int(8u64)
        .eq()
        .verify()
        .drop_()
        .drop_()
        .drop_()
        .builder()
        .append_refs()
        .append_bytes()
        .endcell()
        .push_cell(expected)
        .eq()
        .verify()
        .drop_()
        .drop_()
        .drop_();
    let unsigned = program
        .build_tx(dummy_header(), crate::Limits { gas: 1_000_000 })
        .unwrap();
    let gas = unsigned.metrics().gas_used;
    let tx = unsigned.without_signature().unwrap();
    let decoded =
        crate::ExternalTx::from_bytes_bounded(&tx.to_bytes().unwrap(), 1, 4095, 2497).unwrap();
    let (_, metrics) = decoded
        .verify_with_metrics(crate::Limits { gas: 1_000_000 })
        .unwrap();
    assert_eq!(metrics.gas_used, gas);
}

#[test]
fn unsigned_reads_and_writes_use_byte_widths_including_zero_and_32() {
    for count in 0..=32 {
        let value = Scalar::from(0x12345678u64);
        let program = ScriptBuilder::new()
            .builder()
            .push_int(value)
            .push_int(count as u64)
            .write_uint()
            .endcell()
            .slice()
            .push_int(count as u64)
            .read_uint();
        let mut vm = vm_with_script(program);
        run_to_end(&mut vm).unwrap();
        assert_bytes(&vm.current_call.stack[0], &[]);
        let mut expected = [0; 32];
        expected[..count].copy_from_slice(&value.to_bytes()[..count]);
        assert_int(
            &vm.current_call.stack[1],
            Scalar::from_bytes(expected).unwrap(),
        );
        assert_int(&vm.current_call.stack[2], Scalar::ONE);
    }
    for value in [Scalar::ZERO, Scalar::ONE, Scalar::from(-1i64)] {
        let mut vm = vm_with_script(
            ScriptBuilder::new()
                .builder()
                .push_int(value)
                .write_scalar()
                .endcell()
                .slice()
                .read_scalar(),
        );
        run_to_end(&mut vm).unwrap();
        assert_int(&vm.current_call.stack[1], value);
    }
}

#[test]
fn scalar_reads_soft_fail_without_advancing_on_short_or_noncanonical_input() {
    let mut modulus = Scalar::from(-1i64).to_bytes();
    modulus[0] += 1;
    for data in [vec![0; 31], modulus.to_vec(), vec![255; 32]] {
        let mut vm = operation(Instruction::ReadScalar, vec![slice_value(bytes(&data))]);
        run_to_end(&mut vm).unwrap();
        assert_eq!(vm.current_call.stack.len(), 2);
        assert_bytes(&vm.current_call.stack[0], &data);
        assert_int(&vm.current_call.stack[1], Scalar::ZERO);
    }
    for instruction in [Instruction::ReadUint, Instruction::WriteUint] {
        for count in [Scalar::from(33u64), Scalar::from(-1i64)] {
            let mut vm = operation(instruction.clone(), vec![Value::Scalar(count)]);
            assert!(matches!(run_to_end(&mut vm), Err(VMError::IndexOutOfRange)));
        }
    }
}

#[test]
fn readbytes_copies_requested_bytes_and_preserves_refs_and_failure_operands() {
    let child = bytes(&[9]);
    let source = Cell::new(vec![1, 2, 3], vec![child.into()]).unwrap();
    let mut vm = operation(
        Instruction::ReadBytes,
        vec![
            slice_value(source.clone()),
            builder_value(&[8], &[]),
            Value::Scalar(Scalar::from(2u64)),
        ],
    );
    run_to_end(&mut vm).unwrap();
    assert_bytes(&vm.current_call.stack[0], &[3]);
    let Value::Slice(slice) = &vm.current_call.stack[0] else {
        panic!()
    };
    assert_eq!(slice.remaining_refs(), 1);
    let Value::Builder(builder) = &vm.current_call.stack[1] else {
        panic!()
    };
    assert_eq!(builder.clone().build().payload(), &[8, 1, 2]);
    assert_int(&vm.current_call.stack[2], Scalar::ONE);

    let mut vm = operation(
        Instruction::ReadBytes,
        vec![
            slice_value(source),
            builder_value(&[8], &[]),
            Value::Scalar(Scalar::from(4u64)),
        ],
    );
    run_to_end(&mut vm).unwrap();
    assert_bytes(&vm.current_call.stack[0], &[1, 2, 3]);
    let Value::Builder(builder) = &vm.current_call.stack[1] else {
        panic!()
    };
    assert_eq!(builder.clone().build().payload(), &[8]);
    assert_int(&vm.current_call.stack[2], Scalar::ZERO);
}

#[test]
fn append_streams_are_independent_and_refs_do_not_resolve_children() {
    let target = bytes(&[9]);
    let pruned = target.prune(1).unwrap();
    let refs = vec![CellRef::from(target).to_unloaded(), pruned.into()];
    let source = Cell::new(vec![1, 2], refs.clone()).unwrap();
    for refs_first in [false, true] {
        let mut program = ScriptBuilder::new()
            .push_cell(source.clone())
            .slice()
            .builder();
        program = if refs_first {
            program.append_refs().append_bytes()
        } else {
            program.append_bytes().append_refs()
        };
        let mut vm = vm_with_script(program.endcell());
        run_to_end(&mut vm).unwrap();
        let Value::Slice(slice) = &vm.current_call.stack[0] else {
            panic!()
        };
        assert_eq!((slice.remaining_bytes(), slice.remaining_refs()), (0, 0));
        let Value::Cell(result) = &vm.current_call.stack[1] else {
            panic!()
        };
        assert_eq!(result.id(), source.id());
        assert!(result.as_resident().unwrap().refs()[0].is_unloaded());
    }
}

#[test]
fn byte_and_reference_overflow_hard_fail_without_implicit_spill() {
    let full = vec![0; cells::MAX_CELL_PAYLOAD];
    for instruction in [Instruction::AppendBytes, Instruction::ReadBytes] {
        let mut stack = vec![slice_value(bytes(&[1])), builder_value(&full, &[])];
        if matches!(instruction, Instruction::ReadBytes) {
            stack.push(Value::Scalar(Scalar::ONE));
        }
        assert!(matches!(
            run_to_end(&mut operation(instruction, stack)),
            Err(VMError::Cell(CellError::PayloadCapacity))
        ));
    }
    assert!(matches!(
        run_to_end(&mut operation(
            Instruction::WriteScalar,
            vec![builder_value(&full, &[]), Value::Scalar(Scalar::ZERO)]
        )),
        Err(VMError::Cell(CellError::PayloadCapacity))
    ));
    let reference: CellRef = bytes(&[]).into();
    let four = vec![reference.clone(); 4];
    let source = Cell::new(vec![], vec![reference]).unwrap();
    assert!(matches!(
        run_to_end(&mut operation(
            Instruction::AppendRefs,
            vec![slice_value(source), builder_value(&[], &four)]
        )),
        Err(VMError::Cell(CellError::ReferenceCapacity))
    ));
}

#[test]
fn cells_and_slices_dup_but_builders_are_transient_and_not_copyable() {
    let mut vm = vm_with_script(
        ScriptBuilder::new()
            .push_cell(bytes(&[7, 8]))
            .dup_k(0)
            .slice()
            .dup_k(0)
            .push_int(1u64)
            .read_uint(),
    );
    run_to_end(&mut vm).unwrap();
    assert_bytes(&vm.current_call.stack[0], &[7, 8]);
    assert_bytes(&vm.current_call.stack[1], &[7, 8]);
    assert_bytes(&vm.current_call.stack[2], &[8]);
    assert!(vm.current_call.stack[0].is_copyable());
    assert!(!vm.current_call.stack[1].is_portable());
    let builder = builder_value(&[], &[]);
    assert!(builder.is_droppable());
    assert!(!builder.is_copyable());
    assert!(!builder.is_portable());
    let mut vm = vm_with_script(ScriptBuilder::new().builder().dup_k(0));
    assert!(matches!(run_to_end(&mut vm), Err(VMError::TypeNotCopyable)));
}

#[test]
fn slice_access_rejects_missing_or_pruned_bodies_and_old_pushstr_is_not_an_opcode() {
    let cell = bytes(&[1]);
    for reference in [
        CellRef::from(cell.clone()).to_unloaded(),
        cell.prune(1).unwrap().into(),
    ] {
        let mut vm = vm_with_script(ScriptBuilder::new().push_cell(reference).slice());
        assert!(matches!(
            run_to_end(&mut vm),
            Err(VMError::Cell(
                CellError::MissingCell(_) | CellError::PrunedCell
            ))
        ));
    }
    let mut vm = VM::new(
        dummy_header(),
        CallFrame::from_cell(
            bytes(&[0x19]),
            CallKind::InternalRoot {
                actor: ActorID::Hash([0; 32]),
                caller: None,
            },
            1_000_000,
        )
        .unwrap(),
    );
    assert!(matches!(
        run_to_end(&mut vm),
        Err(VMError::UnknownOpcode(0x19))
    ));
    let program = ScriptBuilder::new()
        .push_literal(CellValue::from_bytes(vec![0; cells::MAX_CELL_PAYLOAD]).unwrap());
    let root = program.to_cell().unwrap();
    assert_eq!(root.payload(), &[0xa6]);
    assert_eq!(
        root.refs()[0].as_resident().unwrap().payload().len(),
        cells::MAX_CELL_PAYLOAD
    );
}

#[test]
fn public_cell_operation_execution_has_identical_prover_and_verifier_gas() {
    let program = ScriptBuilder::new()
        .builder()
        .push_int(513u64)
        .push_int(2u64)
        .write_uint()
        .endcell()
        .slice()
        .builder()
        .push_int(2u64)
        .read_bytes()
        .verify()
        .endcell()
        .slice()
        .push_int(2u64)
        .read_uint()
        .verify()
        .push_int(513u64)
        .eq()
        .verify()
        .drop_()
        .drop_()
        .drop_()
        .drop_();
    let unsigned = program
        .build_tx(dummy_header(), crate::Limits { gas: 1_000_000 })
        .unwrap();
    let gas = unsigned.metrics().gas_used;
    let tx = unsigned.without_signature().unwrap();
    let decoded =
        crate::ExternalTx::from_bytes_bounded(&tx.to_bytes().unwrap(), 1, 4095, 2497).unwrap();
    let (_, metrics) = decoded
        .verify_with_metrics(crate::Limits { gas: 1_000_000 })
        .unwrap();
    assert_eq!(metrics.gas_used, gas);
}

#[test]
fn cursor_and_builder_values_can_return_upward_but_cannot_be_call_arguments() {
    for value in [slice_value(bytes(&[7])), builder_value(&[7], &[])] {
        assert!(matches!(
            VM::require_portable_call_args(&[value]),
            Err(VMError::NonPortableInCall)
        ));
    }
    let branch = ScriptBuilder::new()
        .builder()
        .push_int(7u64)
        .push_int(1u64)
        .write_uint()
        .push_int(1u64)
        .return_();
    let mut registry = StubRegistry {
        script: branch.to_bytecode(),
    };
    let mut vm = vm_with_script(call_to(&ActorID::Hash([9; 32])).verify().drop_());
    vm.last_anchor = Some(Anchor([3; 32]));
    while !vm.current_call.is_finished() || !vm.call_stack.is_empty() {
        vm.step_internal_with_registry(&mut registry).unwrap();
    }
    let Value::Builder(builder) = &vm.current_call.stack[0] else {
        panic!()
    };
    assert_eq!(builder.clone().build().payload(), &[7]);
}

#[test]
fn byte_copy_is_charged_before_mutation_and_zero_length_read_preserves_streams() {
    let source = Cell::new(vec![7; 100], vec![bytes(&[]).into()]).unwrap();
    let mut vm = operation(
        Instruction::ReadBytes,
        vec![
            slice_value(source),
            builder_value(&[9], &[]),
            Value::Scalar(Scalar::ZERO),
        ],
    );
    run_to_end(&mut vm).unwrap();
    let Value::Slice(slice) = &vm.current_call.stack[0] else {
        panic!()
    };
    assert_eq!((slice.remaining_bytes(), slice.remaining_refs()), (100, 1));
    let Value::Builder(builder) = &vm.current_call.stack[1] else {
        panic!()
    };
    assert_eq!(builder.clone().build().payload(), &[9]);

    let mut vm = operation(
        Instruction::AppendBytes,
        vec![slice_value(bytes(&[7; 100])), builder_value(&[], &[])],
    );
    vm.current_call.gas_limit = 50;
    assert!(matches!(run_to_end(&mut vm), Err(VMError::OutOfGas)));
}

#[test]
fn equal_cell_bytes_do_not_overwrite_per_value_private_annotations() {
    let program = ScriptBuilder::new()
        .push_literal(CellValue::scalar(Scalar::ZERO))
        .push_literal(CellValue::point(Point::from_bytes([0; 32])))
        .drop_()
        .scalar()
        .push_int(0u64)
        .eq()
        .verify();
    let tx = program
        .build_tx(dummy_header(), crate::Limits { gas: 1_000_000 })
        .unwrap()
        .without_signature()
        .unwrap();
    tx.verify(crate::Limits { gas: 1_000_000 }).unwrap();
}
