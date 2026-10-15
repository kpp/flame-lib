//! Feature-gated, read-only snapshots of the real VM dispatcher.
//! Debug output is bounded and never resolves data or clones bearer Values.
use super::*;
use bulletproofs::r1cs::ConstraintSystem;
use serde::Serialize;

#[derive(Serialize)]
pub struct Trace {
    pub steps: Vec<Step>,
    pub cells: Vec<CellView>,
    pub status: &'static str,
    pub error: Option<std::string::String>,
    pub validation: Option<std::string::String>,
    pub txid: Option<std::string::String>,
}

#[derive(Serialize)]
pub struct Step {
    pub operation: std::string::String,
    pub error: Option<std::string::String>,
    pub frames: Vec<Frame>,
    pub effects: Vec<Effect>,
    pub gas: u64,
    pub fee: u64,
    pub anchor: Option<std::string::String>,
    pub multipliers: usize,
    pub deferred_signatures: usize,
}

#[derive(Serialize)]
pub struct Frame {
    pub kind: &'static str,
    pub program: Option<std::string::String>,
    pub byte_cursor: usize,
    pub ref_cursor: usize,
    pub gas_used: u64,
    pub gas_limit: u64,
    pub stack_len: usize,
    pub hidden_values: usize,
    pub stack: Vec<ValueView>,
    pub escrow_len: usize,
    pub escrow: Vec<ValueView>,
}

#[derive(Serialize)]
pub struct ValueView {
    pub kind: &'static str,
    pub summary: std::string::String,
    pub copyable: bool,
    pub droppable: bool,
    pub portable: bool,
    pub cell: Option<std::string::String>,
    pub bytes: Option<std::string::String>,
    pub byte_len: usize,
    pub refs: Vec<std::string::String>,
}

#[derive(Serialize)]
pub struct CellView {
    pub id: std::string::String,
    pub payload: std::string::String,
    pub payload_len: usize,
    pub refs: Vec<std::string::String>,
    pub mask: u16,
    pub hashes: Vec<std::string::String>,
    pub depths: Vec<u16>,
    pub pruned: bool,
}

#[derive(Serialize)]
pub struct Effect {
    pub kind: &'static str,
    pub summary: std::string::String,
    pub input: Option<std::string::String>,
}

/// External execution only; no actor registry or chain membership is simulated.
/// Optional validation generates a proof and runs the public verifier unchanged.
pub fn trace(
    program: ScriptBuilder,
    header: TxHeader,
    gas: u64,
    max_steps: usize,
    validate: bool,
    signature: Option<musig::Signature>,
) -> Result<Trace, VMError> {
    if gas > 1_000_000 || !(1..=512).contains(&max_steps) {
        return Err(VMError::IndexOutOfRange);
    }
    let pc_gens = bulletproofs::PedersenGens::default();
    let mut prover = crate::Prover::new(&pc_gens);
    let mut vm = VM::prepare_external(header, program, gas)?;
    let mut cells = BTreeMap::new();
    remember(
        &vm.current_call.entry_program.as_ref().unwrap().clone(),
        &mut cells,
    );
    for (_, cell) in vm.cells.iter() {
        remember(cell, &mut cells);
    }
    let mut result = Trace {
        steps: vec![snapshot(
            &vm,
            &mut prover,
            "Initial state".into(),
            &mut cells,
        )],
        cells: Vec::new(),
        status: "step_limit",
        error: None,
        validation: None,
        txid: None,
    };
    for _ in 0..max_steps {
        let operation = next_operation(&vm.current_call);
        let state = vm.step_external(&mut prover);
        result
            .steps
            .push(snapshot(&vm, &mut prover, operation, &mut cells));
        match state {
            Ok(true) => {}
            Ok(false) => {
                result.status = "complete";
                break;
            }
            Err(error) => {
                result.status = "failed";
                result.error = Some(error.to_string());
                break;
            }
        }
    }
    if result.status == "complete" {
        let execution = vm.into_result(Vec::new(), None)?;
        result.txid = Some(hex(execution.txid.as_bytes()));
        if validate {
            prover
                .cs()
                .transcript()
                .append_message(b"flamevm.txid", execution.txid.as_bytes());
            result.validation = Some(match prover.into_proof() {
                Ok(proof) => match crate::Verifier::verify_cell_with_cells(
                    &pc_gens,
                    execution.program,
                    &proof,
                    header,
                    gas,
                    signature,
                    &execution.cells,
                ) {
                    Ok(verified) if verified.gas_used == execution.gas_used => {
                        "Proof and public execution verified".into()
                    }
                    Ok(_) => "Prover/verifier gas mismatch".into(),
                    Err(error) => format!("Rejected by public verifier: {error}"),
                },
                Err(error) => format!("Proof construction failed: {error}"),
            });
        }
    }
    result.cells = cells.into_values().collect();
    Ok(result)
}

fn next_operation(frame: &CallFrame) -> std::string::String {
    let Some(program) = &frame.program else {
        return "Initialize frame".into();
    };
    if frame.cursor >= program.len {
        return "End of frame".into();
    }
    Instruction::parse(&mut program.bytes(frame.cursor))
        .map(|instruction| format!("{instruction:?}"))
        .unwrap_or_else(|error| format!("Decode error: {error}"))
}

fn snapshot(
    vm: &VM,
    prover: &mut crate::Prover<'_>,
    operation: std::string::String,
    cells: &mut BTreeMap<CellID, CellView>,
) -> Step {
    let mut budget = 128;
    let mut frames = vec![frame(&vm.current_call, true, cells, &mut budget)];
    frames.extend(
        vm.call_stack
            .iter()
            .rev()
            .map(|parent| frame(parent, false, cells, &mut budget)),
    );
    Step {
        operation,
        error: vm.debug_error.clone(),
        frames,
        effects: vm.txlog.iter().map(effect).collect(),
        gas: vm.call_stack.first().unwrap_or(&vm.current_call).gas_used,
        fee: vm.total_fee.total(),
        anchor: vm.last_anchor.map(|anchor| hex(&anchor.0)),
        multipliers: prover.cs().metrics().multipliers,
        deferred_signatures: vm.deferred_sigs.len(),
    }
}

fn frame(
    frame: &CallFrame,
    active: bool,
    cells: &mut BTreeMap<CellID, CellView>,
    budget: &mut usize,
) -> Frame {
    if let Some(program) = &frame.program {
        remember(&program.root, cells);
    }
    let visible = frame
        .stack
        .len()
        .min(if active { 32 } else { 8 })
        .min(*budget);
    *budget -= visible;
    let escrow_visible = frame.snap_failure_values.len().min(8).min(*budget);
    *budget -= escrow_visible;
    Frame {
        kind: match &frame.kind {
            CallKind::ExternalRoot => "ExternalRoot",
            CallKind::InternalRoot { .. } => "InternalRoot",
            CallKind::QueryRoot { .. } => "QueryRoot",
            CallKind::ContractOpen { .. } => "ContractOpen",
            CallKind::ActorCall { .. } => "ActorCall",
        },
        program: frame
            .program
            .as_ref()
            .map(|program| hex(&program.root.id())),
        byte_cursor: frame.cursor,
        ref_cursor: frame.program.as_ref().map_or(0, Program::reference_cursor),
        gas_used: frame.gas_used,
        gas_limit: frame.gas_limit,
        stack_len: frame.stack.len(),
        hidden_values: frame.stack.len() - visible,
        stack: frame
            .stack
            .iter()
            .rev()
            .take(visible)
            .map(|value| view(value, cells))
            .collect(),
        escrow_len: frame.snap_failure_values.len(),
        escrow: frame
            .snap_failure_values
            .iter()
            .take(escrow_visible)
            .map(|value| view(value, cells))
            .collect(),
    }
}

fn view(value: &Value, cells: &mut BTreeMap<CellID, CellView>) -> ValueView {
    let mut result = ValueView {
        kind: "",
        summary: std::string::String::new(),
        copyable: value.is_copyable(),
        droppable: value.is_droppable(),
        portable: value.is_portable(),
        cell: None,
        bytes: None,
        byte_len: 0,
        refs: Vec::new(),
    };
    match value {
        Value::Scalar(value) => {
            result.kind = "Scalar";
            result.summary = decimal(*value);
        }
        Value::ClearToken(token) => {
            result.kind = "ClearToken";
            result.summary = format!(
                "qty {} · flavor {}",
                decimal(token.qty()),
                decimal(token.flv())
            );
        }
        Value::Token(token) => {
            result.kind = "Token";
            result.summary = format!("qty commitment {}", hex(token.qty().to_point().as_bytes()));
        }
        Value::WideToken(_) => {
            result.kind = "WideToken";
            result.summary = "Encrypted balance / loan".into();
        }
        Value::Cell(cell) => {
            result.kind = "Cell";
            result.summary = hex(&cell.id());
            result.cell = Some(hex(&cell.id()));
            if let Some(body) = cell.as_resident() {
                remember(body, cells);
            }
        }
        Value::Builder(builder) => {
            result.kind = "Builder";
            bytes(&mut result, builder.payload(), builder.refs().iter());
        }
        Value::Slice(slice) => {
            result.kind = "Slice";
            bytes(&mut result, slice.bytes(), slice.refs());
        }
        Value::Point(point) => {
            result.kind = "Point";
            result.summary = hex(&point.to_bytes());
        }
        Value::Contract(contract) => {
            result.kind = "Contract";
            result.summary = hex(&contract.id());
        }
        Value::Dict(dict) => {
            result.kind = "Dict";
            result.summary = format!(
                "{} entries ({} cached)",
                dict.len(),
                dict.cached_values().count()
            );
        }
        Value::Expression(Expression::Constant(value)) => {
            result.kind = "Expression";
            result.summary = format!("constant {}", decimal(*value));
        }
        Value::Expression(Expression::LinearCombination(terms, assignment)) => {
            result.kind = "Expression";
            result.summary = format!(
                "{} terms · assignment {}",
                terms.len(),
                assignment.map(decimal).unwrap_or_else(|| "missing".into())
            );
        }
        Value::Variable(_) => {
            result.kind = "Variable";
            result.summary = "Pedersen-committed variable".into();
        }
        Value::Constraint(constraint) => {
            result.kind = "Constraint";
            result.summary = match constraint {
                Constraint::Cleartext(value) => value.to_string(),
                Constraint::Secret(_) => "secret condition".into(),
            };
        }
        Value::MultiscalarMul(msm) => {
            result.kind = "MultiscalarMul";
            result.summary = format!("{} terms", msm.len());
        }
        Value::Merlin(_) => {
            result.kind = "Merlin";
            result.summary = "Transcript".into();
        }
    }
    result
}

fn bytes<'a>(
    result: &mut ValueView,
    payload: &[u8],
    refs: impl ExactSizeIterator<Item = &'a cells::CellRef>,
) {
    result.byte_len = payload.len();
    result.bytes = Some(hex(&payload[..payload.len().min(64)]));
    result.summary = format!("{} bytes · {} refs", payload.len(), refs.len());
    result.refs = refs.map(|reference| hex(&reference.id())).collect();
}

fn remember(root: &Cell, result: &mut BTreeMap<CellID, CellView>) {
    let mut pending = vec![root.clone()];
    while let Some(cell) = pending.pop() {
        if result.contains_key(&cell.id()) || result.len() >= 1024 {
            continue;
        }
        pending.extend(
            cell.refs()
                .iter()
                .filter_map(|reference| reference.as_resident().cloned()),
        );
        result.insert(
            cell.id(),
            CellView {
                id: hex(&cell.id()),
                payload: hex(&cell.payload()[..cell.payload().len().min(256)]),
                payload_len: cell.payload().len(),
                refs: cell
                    .refs()
                    .iter()
                    .map(|reference| hex(&reference.id()))
                    .collect(),
                mask: cell.level_mask(),
                hashes: cell
                    .commitment()
                    .hashes()
                    .iter()
                    .map(|hash| hex(hash))
                    .collect(),
                depths: cell.commitment().depths().to_vec(),
                pruned: cell.is_pruned(),
            },
        );
    }
}

fn effect(entry: &TxEntry) -> Effect {
    let (kind, summary, input) = match entry {
        TxEntry::Header(header) => (
            "Header",
            format!("version {} · locktime {}", header.version, header.locktime),
            None,
        ),
        TxEntry::Input(id) => ("Input", hex(id), Some(hex(id))),
        TxEntry::Output(contract) => ("Output", hex(&contract.id()), None),
        TxEntry::Data(data) => ("Data", hex(&data[..data.len().min(64)]), None),
        TxEntry::Fee(qty) => ("Fee", qty.to_string(), None),
        TxEntry::Send(message) => ("Send", hex(&message.id().0), None),
        TxEntry::Receive(id) => ("Receive", hex(id), None),
        TxEntry::IssuePub(qty, flavor) => (
            "IssuePub",
            format!("{} · {}", decimal(*qty), decimal(*flavor)),
            None,
        ),
        TxEntry::IssuePriv(_, _) => ("IssuePriv", "Confidential issuance".into(), None),
        TxEntry::Retire(_, _) => ("Retire", "Retired tokens".into(), None),
        TxEntry::ActorDeploy { actor, .. } => ("ActorDeploy", hex(&actor.to_hash()), None),
        TxEntry::ActorSave { actor, .. } => ("ActorSave", hex(&actor.to_hash()), None),
        TxEntry::SetCode { actor, .. } => ("SetCode", hex(&actor.to_hash()), None),
        TxEntry::StoragePurchase { actor, .. } => ("StoragePurchase", hex(&actor.to_hash()), None),
        TxEntry::ActorDestroy { actor } => ("ActorDestroy", hex(&actor.to_hash()), None),
        TxEntry::ActorRestore { actor, state } => (
            "ActorRestore",
            format!("{} → {}", hex(&actor.to_hash()), hex(&state.id())),
            None,
        ),
    };
    Effect {
        kind,
        summary,
        input,
    }
}

pub fn hex(bytes: &[u8]) -> std::string::String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Full-width centered scalar formatting, independent of JavaScript number precision.
pub fn decimal(value: Scalar) -> std::string::String {
    let mut bytes = value.centered_abs().to_bytes();
    let mut digits = Vec::new();
    while bytes.iter().any(|byte| *byte != 0) {
        let mut remainder = 0u16;
        for byte in bytes.iter_mut().rev() {
            let number = remainder * 256 + u16::from(*byte);
            *byte = (number / 10) as u8;
            remainder = number % 10;
        }
        digits.push(char::from(b'0' + remainder as u8));
    }
    if digits.is_empty() {
        return "0".into();
    }
    if value.is_centered_negative() {
        digits.push('-');
    }
    digits.into_iter().rev().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracing_preserves_normal_execution_gas_and_transaction_identity() {
        let program = ScriptBuilder::new()
            .alloc(Some(Scalar::from(7u64)))
            .expr()
            .push_int(7u64)
            .eq()
            .verify();
        let header = TxHeader {
            version: 1,
            locktime: 0,
        };
        let regular = crate::Prover::prove(
            &bulletproofs::PedersenGens::default(),
            program.clone(),
            header,
            100_000,
        )
        .unwrap();
        let recorded = trace(program, header, 100_000, 256, false, None).unwrap();
        assert_eq!(recorded.status, "complete");
        assert_eq!(recorded.txid, Some(hex(regular.txid.as_bytes())));
        assert_eq!(recorded.steps.last().unwrap().gas, regular.gas_used);
    }
}
