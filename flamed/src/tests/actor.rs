//! Actor RPC reads current committed data without changing execution state.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use flamed_rpc::{
    ActorCode, ActorId, ActorResult, ActorStateEnvelope, BlockId, CellId, ClientError,
    FlamedApiClient, HttpClientBuilder, TxValue, ValueCell,
};
use flamevm::{
    Cell, CellDecode, CellEncode, CellEnvelope, CellIndex, CellValue, Dict, ScriptBuilder, Value,
};
use jsonrpsee::{core::client::ClientT, rpc_params};
use tempfile::TempDir;

use super::*;

#[tokio::test]
async fn query_actor_is_bounded_pinned_and_returns_only_data() {
    use flamed_rpc::{codes, ValueEnvelope, MAX_QUERY_BYTES, MAX_QUERY_GAS};

    let dir = TempDir::new().unwrap();
    let (genesis, cfg) = devnet(dir.path(), &account(&SEED_A));
    let mut node = Node::open(&genesis, &cfg).unwrap();
    let code = ScriptBuilder::new().load().save().return_().to_bytecode();
    let (actor, _) = block::deploy_with_code(&mut node, &genesis, code);
    let shared = Arc::new(Mutex::new(node));
    let (addr, handle) = crate::rpc::serve(shared.clone(), cfg.rpc_bind)
        .await
        .unwrap();
    let client = HttpClientBuilder::default()
        .build(format!("http://{addr}"))
        .unwrap();
    let before = client.actor(actor).await.unwrap();
    let tip = client.tip().await.unwrap();

    // DNS records may contain nested Dicts and Cells with descendants.
    let child = Cell::new(vec![42; 256], vec![]).unwrap();
    let graph = Cell::new(vec![17], vec![child.into()]).unwrap();
    let records = Dict::from_values(vec![
        Value::Point(flamevm::Point::from_bytes([7; 32])),
        Value::Dict(Dict::from_values(vec![Value::Cell(graph.clone().into())])),
    ]);
    let values = [
        Value::Scalar(flamevm::Scalar::from(42u64)),
        Value::Point(flamevm::Point::from_bytes([7; 32])),
        Value::Cell(graph.into()),
        Value::Dict(Dict::new()),
        Value::Dict(records),
    ];
    for value in values {
        let argument = ValueEnvelope(value.to_envelope().unwrap().encode());
        let result = client
            .query_actor(actor, argument.clone(), MAX_QUERY_GAS, Some(tip.hash))
            .await
            .unwrap();
        assert_eq!(result.block, tip.hash);
        assert_eq!(result.height, tip.height);
        assert!(result.gas_used > 0 && result.gas_used <= MAX_QUERY_GAS);
        assert_eq!(result.value, argument);
        let mut gas = MAX_QUERY_GAS;
        let mut envelope =
            CellEnvelope::decode(&result.value.0, MAX_QUERY_BYTES, &mut gas).unwrap();
        let root = envelope.cells().get(&envelope.root()).unwrap();
        assert_eq!(
            Value::from_cell(&root, &mut envelope)
                .unwrap()
                .to_cell()
                .unwrap()
                .id(),
            value.to_cell().unwrap().id()
        );
        assert_eq!(client.actor(actor).await.unwrap(), before);
        assert_eq!(client.tip().await.unwrap(), tip);
    }

    let argument = ValueEnvelope(
        Value::Scalar(flamevm::Scalar::ZERO)
            .to_envelope()
            .unwrap()
            .encode(),
    );
    let cases = [
        (actor, argument.clone(), 0, None, codes::LIMIT_EXCEEDED),
        (
            actor,
            argument.clone(),
            MAX_QUERY_GAS + 1,
            None,
            codes::LIMIT_EXCEEDED,
        ),
        (actor, argument.clone(), 1, None, codes::QUERY_FAILED),
        (
            ActorId([0xff; 32]),
            argument.clone(),
            MAX_QUERY_GAS,
            None,
            codes::NOT_FOUND,
        ),
        (
            actor,
            ValueEnvelope(vec![0]),
            MAX_QUERY_GAS,
            None,
            codes::INVALID_BYTES,
        ),
        (
            actor,
            ValueEnvelope(
                Value::Dict(Dict::from_values(vec![Value::Dict(Dict::from_values(
                    vec![Value::ClearToken(flamevm::ClearToken::new(
                        flamevm::Scalar::ONE,
                        flamevm::Scalar::ZERO,
                    ))],
                ))]))
                .to_envelope()
                .unwrap()
                .encode(),
            ),
            MAX_QUERY_GAS,
            None,
            codes::QUERY_FAILED,
        ),
        (
            actor,
            ValueEnvelope(vec![0; MAX_QUERY_BYTES + 1]),
            MAX_QUERY_GAS,
            None,
            codes::LIMIT_EXCEEDED,
        ),
        (
            actor,
            argument.clone(),
            MAX_QUERY_GAS,
            Some(BlockId([0xff; 32])),
            codes::STALE_BLOCK,
        ),
        (
            actor,
            ValueEnvelope(
                Value::ClearToken(flamevm::ClearToken::new(
                    flamevm::Scalar::ONE,
                    flamevm::Scalar::ZERO,
                ))
                .to_envelope()
                .unwrap()
                .encode(),
            ),
            MAX_QUERY_GAS,
            None,
            codes::INVALID_BYTES,
        ),
    ];
    for (id, input, gas, expected, code) in cases {
        let ClientError::Call(error) = client
            .query_actor(id, input, gas, expected)
            .await
            .unwrap_err()
        else {
            panic!("expected query error");
        };
        assert_eq!(error.code(), code, "{error}");
        assert_eq!(client.actor(actor).await.unwrap(), before);
    }
    shared.lock().unwrap().mint_block().unwrap();
    let ClientError::Call(error) = client
        .query_actor(actor, argument.clone(), MAX_QUERY_GAS, Some(tip.hash))
        .await
        .unwrap_err()
    else {
        panic!("expected stale block");
    };
    assert_eq!(error.code(), codes::STALE_BLOCK);
    assert_eq!(
        client
            .query_actor(actor, argument, MAX_QUERY_GAS, None)
            .await
            .unwrap()
            .height,
        tip.height + 1
    );
    handle.stop().unwrap();
    handle.stopped().await;
}

#[test]
fn query_actor_rejects_invalid_and_expanding_dict_envelopes() {
    use flamed_rpc::{ValueEnvelope, MAX_QUERY_BYTES, MAX_QUERY_GAS};
    use flamevm::{CellBuilder, CellError, Scalar, Trie};

    fn dict_cell(values: &[Cell], flags: u8) -> Cell {
        let mut trie = Trie::new(32).unwrap();
        for (index, value) in values.iter().enumerate() {
            let mut key = Scalar::from(index as u64).to_bytes();
            key.reverse();
            trie.insert(&key, value.clone(), &mut ()).unwrap();
        }
        let mut dict = CellBuilder::new();
        dict.store_u64(values.len() as u64).unwrap();
        dict.store_u8(flags).unwrap();
        if let Some(root) = trie.into_root() {
            dict.store_ref(root).unwrap();
        }
        Cell::new(vec![2], vec![dict.build().into()]).unwrap()
    }

    fn envelope(root: Cell) -> ValueEnvelope {
        let root = Arc::new(root);
        ValueEnvelope(
            CellEnvelope::new(root.id(), CellIndex::collect(root).unwrap())
                .unwrap()
                .encode(),
        )
    }

    let dir = TempDir::new().unwrap();
    let (genesis, cfg) = devnet(dir.path(), &account(&SEED_A));
    let mut node = Node::open(&genesis, &cfg).unwrap();
    let code = ScriptBuilder::new().load().save().return_().to_bytecode();
    let (actor, _) = block::deploy_with_code(&mut node, &genesis, code);
    let before = node.actor(actor).unwrap();

    let token = Value::ClearToken(flamevm::ClearToken::new(Scalar::ONE, Scalar::ZERO))
        .to_cell()
        .unwrap();
    let forged = dict_cell(&[dict_cell(&[token], 3)], 3);
    assert!(matches!(
        node.query_actor(actor, envelope(forged), MAX_QUERY_GAS, None),
        Err(crate::node::NodeError::QueryDecode(CellError::InvalidFormat))
    ));

    let root = Arc::new(dict_cell(&[], 3));
    let mut missing = CellIndex::new();
    missing.insert(root.clone()).unwrap();
    let missing = ValueEnvelope(CellEnvelope::new(root.id(), missing).unwrap().encode());
    assert!(matches!(
        node.query_actor(actor, missing, MAX_QUERY_GAS, None),
        Err(crate::node::NodeError::QueryDecode(CellError::MissingCell(_)))
    ));

    let mut shared = Value::Scalar(Scalar::ZERO).to_cell().unwrap();
    for _ in 0..24 {
        shared = dict_cell(&[shared.clone(), shared], 3);
    }
    let shared = envelope(shared);
    assert!(shared.0.len() < MAX_QUERY_BYTES);
    assert!(matches!(
        node.query_actor(actor, shared, MAX_QUERY_GAS, None),
        Err(crate::node::NodeError::QueryDecode(CellError::ResourceExhausted))
    ));
    assert_eq!(node.actor(actor).unwrap(), before);
}

#[tokio::test]
async fn query_actor_rejects_writes_without_changing_state() {
    use flamed_rpc::{codes, ValueEnvelope, MAX_QUERY_GAS};

    let dir = TempDir::new().unwrap();
    let (genesis, cfg) = devnet(dir.path(), &account(&SEED_A));
    let mut node = Node::open(&genesis, &cfg).unwrap();
    let (actor, change) = block::deploy(&mut node, &genesis);
    let before = node.actor(actor).unwrap();
    let input = ValueEnvelope(
        Value::Scalar(flamevm::Scalar::ZERO)
            .to_envelope()
            .unwrap()
            .encode(),
    );
    let shared = Arc::new(Mutex::new(node));
    let (addr, handle) = crate::rpc::serve(shared.clone(), cfg.rpc_bind)
        .await
        .unwrap();
    let client = HttpClientBuilder::default()
        .build(format!("http://{addr}"))
        .unwrap();
    let ClientError::Call(error) = client
        .query_actor(actor, input, MAX_QUERY_GAS, None)
        .await
        .unwrap_err()
    else {
        panic!("expected write rejection");
    };
    assert_eq!(error.code(), codes::QUERY_FAILED);
    let mut node = shared.lock().unwrap();
    assert_eq!(node.actor(actor).unwrap(), before);
    // Normal message delivery still enters with write permission.
    block::send_messages(&mut node, actor, change, &[MAX_QUERY_GAS]);
    node.mint_block().unwrap();
    assert_ne!(node.actor(actor).unwrap().state_hash, before.state_hash);
    drop(node);
    handle.stop().unwrap();
    handle.stopped().await;
}

#[tokio::test]
async fn actor_tracks_updates_expiry_and_replay() {
    let dir = TempDir::new().unwrap();
    let a = account(&SEED_A);
    let (_, cfg) = devnet(dir.path(), &a);
    let mut params = chainparams(&a);
    params.storage.lease_duration_blocks = Some(3);
    let path = cfg.genesis.as_ref().unwrap();
    crate::genesis::write(&params, path).unwrap();
    let genesis = GenesisFile::load(path).unwrap();
    let mut node = Node::open(&genesis, &cfg).unwrap();
    let (actor, change) = block::deploy(&mut node, &genesis);
    let tip = node.tip();
    let shared = Arc::new(Mutex::new(node));
    let (addr, handle) = crate::rpc::serve(shared.clone(), cfg.rpc_bind)
        .await
        .unwrap();
    let client = HttpClientBuilder::default()
        .build(format!("http://{addr}"))
        .unwrap();

    let deployed = client.actor(actor).await.unwrap();
    assert_eq!(deployed.id, actor);
    assert_eq!(deployed.height, 1);
    assert_eq!(deployed.block, BlockId(tip.hash.into_bytes()));
    assert_eq!(deployed.storage_capacity, 1024);
    assert!(deployed.storage_used > 0);
    assert!(deployed.storage_used <= deployed.storage_capacity);
    let code = &deployed.code.as_ref().unwrap().0;
    let mut code_gas = u64::MAX;
    let code_envelope = CellEnvelope::decode(code, code.len(), &mut code_gas).unwrap();
    assert_eq!(code_envelope.root(), deployed.code_hash.unwrap().0);
    assert_eq!(
        code_envelope
            .cells()
            .get(&deployed.code_hash.unwrap().0)
            .unwrap()
            .payload()
            .len() as u64,
        deployed.code_size
    );
    let state = &deployed.state.as_ref().unwrap().0;
    let mut gas = state.len() as u64 * 1024;
    let envelope = CellEnvelope::decode(state, state.len(), &mut gas).unwrap();
    assert_eq!(envelope.root(), deployed.state_hash.unwrap().0);
    let Some(TxValue::Dict { entries }) = &deployed.decoded_state else {
        panic!("expected a decoded dictionary")
    };
    assert_eq!(entries[0].key, "0");
    assert_eq!(entries[0].value, TxValue::Scalar { value: "0".into() });
    assert!(deployed.state_error.is_none() && deployed.code_error.is_none());
    assert_eq!(
        deployed
            .instructions
            .iter()
            .map(|op| op.text.as_str())
            .collect::<Vec<_>>(),
        ["drop", "load", "push 0", "get", "push 1", "add", "put", "save"]
    );
    assert_eq!(deployed.instructions[0].offset, 0);
    assert!(deployed
        .instructions
        .windows(2)
        .all(|pair| pair[0].offset < pair[1].offset));
    assert!(deployed.instructions.last().unwrap().offset < code.len() as u64);
    assert_eq!(client.actor(actor).await.unwrap(), deployed);
    assert_eq!(shared.lock().unwrap().tip(), tip);
    let json: serde_json::Value = client.request("actor", rpc_params![actor]).await.unwrap();
    assert!(json.get("snapshot").is_none());
    assert_eq!(json["id"], actor.to_string());
    let code_json = json["code"].to_string();
    let bytes =
        flamed_rpc::codec::base64::deserialize(&mut serde_json::Deserializer::from_str(&code_json))
            .unwrap();
    assert_eq!(&bytes, code);
    assert_eq!(
        serde_json::from_value::<ActorResult>(json).unwrap(),
        deployed
    );

    {
        let mut node = shared.lock().unwrap();
        block::send_messages(&mut node, actor, change, &[1_000_000]);
        node.mint_block().unwrap();
    }
    let updated = client.actor(actor).await.unwrap();
    assert_eq!(updated.height, 2);
    assert_eq!(updated.code_hash, deployed.code_hash);
    assert_ne!(updated.state_hash, deployed.state_hash);
    let Some(TxValue::Dict { entries }) = &updated.decoded_state else {
        panic!("expected updated state")
    };
    assert_eq!(entries[0].value, TxValue::Scalar { value: "1".into() });
    shared.lock().unwrap().mint_block().unwrap();
    assert!(client.actor(actor).await.unwrap().state.is_some());
    shared.lock().unwrap().mint_block().unwrap();
    let frozen = client.actor(actor).await.unwrap();
    assert_eq!(frozen.height, 4);
    assert!(frozen.frozen);
    assert_ne!(frozen.root_hash, updated.root_hash);
    assert_eq!(frozen.code_hash, None);
    assert_eq!(frozen.state_hash, None);
    assert_eq!(frozen.code_size, updated.code_size);
    assert_eq!(frozen.state_size, updated.state_size);
    assert_eq!(frozen.storage_capacity, 0);
    assert!(frozen.code.is_none() && frozen.state.is_none());
    assert!(frozen.decoded_state.is_none() && frozen.instructions.is_empty());
    assert_eq!(
        frozen.state_error.as_deref(),
        Some("State body is unavailable")
    );
    assert_eq!(
        frozen.code_error.as_deref(),
        Some("Code body is unavailable")
    );

    let ClientError::Call(error) = client.actor(ActorId([0xff; 32])).await.unwrap_err() else {
        panic!("expected a missing actor error")
    };
    assert_eq!(error.code(), flamed_rpc::codes::NOT_FOUND);
    let invalid: Result<ActorResult, _> = client.request("actor", rpc_params!["bad"]).await;
    let ClientError::Call(error) = invalid.unwrap_err() else {
        panic!("expected invalid params")
    };
    assert_eq!(error.code(), flamed_rpc::ErrorCode::InvalidParams.code());
    drop(client);
    handle.stop().unwrap();
    tokio::time::timeout(Duration::from_secs(5), handle.stopped())
        .await
        .unwrap();
    drop(shared);
    let node = Node::open(&genesis, &cfg).unwrap();
    assert_eq!(crate::inspect::actor(node.actor(actor).unwrap()), frozen);
}

#[test]
fn actor_keeps_full_data_and_reports_unavailable_or_malformed_bodies() {
    let literal = CellValue::from_bytes(vec![0xab; 257]).unwrap();
    let state = Value::Dict(Dict::from_values(vec![Value::Cell(literal.clone()); 2_001]));
    let literal_code = flamevm::Cell::new(vec![0xcd; 257], vec![]).unwrap();
    let mut program = ScriptBuilder::new();
    for _ in 0..1_001 {
        program = program.nop();
    }
    let complete = program.push_cell(literal_code.clone()).to_cell().unwrap();
    let error_offset = complete.payload().len();
    let mut payload = complete.payload().to_vec();
    payload.push(0x18); // Truncated full-width scalar literal.
    let code = flamevm::Cell::new(payload, complete.refs().to_vec()).unwrap();
    let code_bytes = CellEnvelope::new(code.id(), CellIndex::collect(code.clone().into()).unwrap())
        .unwrap()
        .encode();
    let state_cell = state.to_cell().unwrap();
    let mut snapshot = ActorResult {
        id: ActorId([1; 32]),
        height: 1,
        block: BlockId([2; 32]),
        root_hash: CellId(
            Cell::new(vec![], vec![code.clone().into(), state_cell.clone().into()])
                .unwrap()
                .id(),
        ),
        frozen: false,
        code_hash: Some(CellId(code.id())),
        state_hash: Some(CellId(state_cell.id())),
        code_size: code.payload().len() as u64,
        state_size: 0,
        storage_used: 0,
        storage_capacity: 0,
        code: Some(ActorCode(code_bytes)),
        state: Some(ActorStateEnvelope(state.to_envelope().unwrap().encode())),
        decoded_state: None,
        state_error: None,
        instructions: Vec::new(),
        code_error: None,
    };
    let detail = crate::inspect::actor(snapshot.clone());
    assert_eq!(detail.code, snapshot.code);
    assert_eq!(detail.state, snapshot.state);
    let Some(TxValue::Dict { entries }) = detail.decoded_state else {
        panic!("expected full state")
    };
    assert_eq!(entries.len(), 2_001);
    assert!(entries.iter().all(|entry| entry.value
        == TxValue::Cells {
            root: CellId(literal.id()),
            cells: vec![ValueCell {
                id: CellId(literal.id()),
                data: vec![0xab; 257],
                refs: vec![],
            }]
        }));
    assert!(detail.state_error.is_none());
    assert_eq!(detail.instructions.len(), 1_002);
    assert_eq!(
        detail.instructions[1_001].text,
        format!("pushcell 0x{}", hex::encode(literal_code.id()))
    );
    assert!(detail
        .code_error
        .unwrap()
        .starts_with(&format!("Byte {error_offset}: ")));

    let mut cells = CellIndex::new();
    cells.insert(state_cell.into()).unwrap();
    snapshot.state = Some(ActorStateEnvelope(
        CellEnvelope::new(snapshot.state_hash.unwrap().0, cells)
            .unwrap()
            .encode(),
    ));
    let partial = crate::inspect::actor(snapshot.clone());
    let Some(TxValue::Cells { root, cells }) = partial.decoded_state else {
        panic!("expected available cells")
    };
    assert_eq!(root, snapshot.state_hash.unwrap());
    assert_eq!(cells.len(), 1);
    assert!(!cells[0].refs.is_empty());
    assert!(partial.state_error.is_none());

    snapshot.state = Some(ActorStateEnvelope(vec![0]));
    snapshot.code = Some(ActorCode(vec![]));
    let malformed = crate::inspect::actor(snapshot.clone());
    assert!(malformed.decoded_state.is_none() && malformed.state_error.is_some());
    assert!(malformed.instructions.is_empty() && malformed.code_error.is_some());
    snapshot.state = None;
    snapshot.code = None;
    let missing = crate::inspect::actor(snapshot);
    assert!(missing.decoded_state.is_none() && missing.state_error.is_some());
    assert!(missing.instructions.is_empty() && missing.code_error.is_some());
}
