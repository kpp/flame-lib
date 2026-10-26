//! Run explicitly with optimizations and no competing tests:
//! cargo test -p flamechain --release --offline benchmark_small_updates -- --ignored --nocapture --test-threads=1

use super::super::collect_owned;
use super::*;
use cells::{CellEncode, CellRef};
use flamevm::Value;
use std::hint::black_box;
use std::time::{Duration, Instant};

const SAMPLES: usize = 11;

fn tree(depth: u32, next_leaf: &mut u64) -> Cell {
    if depth == 0 {
        let mut payload = vec![0; 128];
        payload[..8].copy_from_slice(&next_leaf.to_le_bytes());
        *next_leaf += 1;
        Cell::new(payload, vec![]).unwrap()
    } else {
        Cell::new(
            vec![],
            (0..4).map(|_| tree(depth - 1, next_leaf).into()).collect(),
        )
        .unwrap()
    }
}

// Only the leftmost path is resident in the update, including the old-value
// version. A resident original tree would unfairly charge one side for setup.
fn change_leaf(root: &Cell, value: u8) -> Cell {
    if root.refs().is_empty() {
        let mut payload = root.payload().to_vec();
        payload[8] = value;
        Cell::new(payload, vec![]).unwrap()
    } else {
        let mut refs: Vec<_> = root.refs().iter().map(CellRef::to_unloaded).collect();
        refs[0] = change_leaf(root.refs()[0].as_resident().unwrap(), value).into();
        Cell::new(vec![], refs).unwrap()
    }
}

fn image(code: CellRef, data: Cell) -> Cell {
    Cell::new(
        vec![],
        vec![code, Value::Cell(data.into()).to_cell().unwrap().into()],
    )
    .unwrap()
}

fn recount(root: &Cell, prior: &CellIndex) -> (CellIndex, u64) {
    let cells = collect_owned([Arc::new(root.clone())], prior).unwrap();
    let bytes = cells
        .iter()
        .try_fold(0u64, |sum, (_, c)| sum.checked_add(c.record_size() as u64))
        .unwrap();
    (cells, bytes)
}

fn assert_same(actual: &RetainedCells, expected: &(CellIndex, u64)) {
    assert_eq!(actual.bytes, expected.1);
    assert_eq!(actual.cells.len(), expected.0.len());
    for ((id, body), (expected_id, expected_body)) in actual.cells.iter().zip(expected.0.iter()) {
        assert_eq!(id, expected_id);
        assert_eq!(body.commitment(), expected_body.commitment());
    }
}

fn unique_copy(base: &RetainedCells) -> RetainedCells {
    let mut copy = base.clone();
    Arc::make_mut(&mut copy.references);
    Arc::make_mut(&mut copy.cells);
    copy
}

// Counterfactual, NOT a production alternative: skips the atomic COW wrapper.
// These fixtures always replace the root; failures would not roll back here.
fn delta_only(state: &mut RetainedCells, root: &Cell) {
    assert_eq!(Arc::strong_count(&state.references), 1);
    assert_eq!(Arc::strong_count(&state.cells), 1);
    let supplied = CellIndex::collect(Arc::new(root.clone())).unwrap();
    assert!(supplied.contains(&root.id()) || state.cells.contains(&root.id()));
    state.add_references(vec![root.id()], &supplied).unwrap();
    let mut pending = Vec::new();
    for (id, cell) in supplied.iter() {
        if state.references.contains_key(id) && !state.cells.contains(id) {
            state.add_body(cell, &mut pending).unwrap();
        }
    }
    state.add_references(pending, &supplied).unwrap();
    state.remove_reference(state.root.unwrap()).unwrap();
    state.root = Some(root.id());
}

fn measure(mut operation: impl FnMut()) -> (f64, f64, usize) {
    operation();
    operation();
    let start = Instant::now();
    operation();
    let iterations = (Duration::from_millis(30).as_nanos() / start.elapsed().as_nanos().max(1))
        .clamp(1, 20_000) as usize;
    let mut samples = [0.0; SAMPLES];
    for sample in &mut samples {
        let start = Instant::now();
        for _ in 0..iterations {
            operation();
        }
        *sample = start.elapsed().as_secs_f64() * 1e6 / iterations as f64;
    }
    samples.sort_by(f64::total_cmp);
    (samples[SAMPLES / 2], samples[9], iterations)
}

fn report(base: &RetainedCells, supplied: usize, name: &str, operation: impl FnMut()) {
    let (median, p90, iterations) = measure(operation);
    println!(
        "{},{},{},{},{:.3},{:.3},{},{}",
        base.cells.len(),
        base.bytes,
        supplied,
        name,
        median,
        p90,
        SAMPLES,
        iterations
    );
}

// Instrumented reproduction of atomic replacement, forcing both COW copies
// up front so copying + old-map retirement can be separated from the delta.
fn profile_atomic(state: &mut RetainedCells, root: &Cell) -> (f64, f64) {
    let start = Instant::now();
    let mut next = unique_copy(state);
    let copied = Instant::now();
    delta_only(&mut next, root);
    let updated = Instant::now();
    *state = next;
    let retired = Instant::now();
    (
        ((copied - start) + (retired - updated)).as_secs_f64() * 1e6,
        (updated - copied).as_secs_f64() * 1e6,
    )
}

fn report_profile(base: &RetainedCells, roots: &[Cell; 2], supplied: usize) {
    let mut state = base.clone();
    profile_atomic(&mut state, &roots[0]);
    let warm = profile_atomic(&mut state, &roots[1]);
    let iterations = (30_000.0 / (warm.0 + warm.1)).clamp(1.0, 20_000.0) as usize;
    let mut parts = [[0.0; SAMPLES]; 3];
    let mut step = 0;
    for sample in 0..SAMPLES {
        for _ in 0..iterations {
            let (copy, delta) = profile_atomic(&mut state, &roots[step % 2]);
            step += 1;
            parts[0][sample] += copy / iterations as f64;
            parts[1][sample] += delta / iterations as f64;
            parts[2][sample] += (copy + delta) / iterations as f64;
        }
    }
    for (name, mut samples) in ["profile_copy_and_retire", "profile_delta", "profile_total"]
        .into_iter()
        .zip(parts)
    {
        samples.sort_by(f64::total_cmp);
        println!(
            "{},{},{},{},{:.3},{:.3},{},{}",
            base.cells.len(),
            base.bytes,
            supplied,
            name,
            samples[SAMPLES / 2],
            samples[9],
            SAMPLES,
            iterations
        );
    }
}

#[test]
#[ignore = "release-mode storage accounting benchmark"]
fn benchmark_small_updates() {
    assert!(!cfg!(debug_assertions), "run with --release");
    println!("cells,bytes,supplied,case,median_us,p90_us,samples,iterations_per_sample");
    for depth in [5, 7, 8, 9] {
        let data = tree(depth, &mut 0);
        let code = Cell::new(vec![0], vec![]).unwrap();
        let roots = [
            image(
                CellRef::resident(code.clone()).to_unloaded(),
                change_leaf(&data, 1),
            ),
            image(
                CellRef::resident(code.clone()).to_unloaded(),
                change_leaf(&data, 0),
            ),
        ];
        let initial = image(code.into(), data);
        let initial_cells = CellIndex::collect(Arc::new(initial.clone())).unwrap();
        let mut base = RetainedCells::default();
        base.replace_root(initial.id(), &initial_cells).unwrap();
        assert_eq!(base.cells.len(), ((4usize.pow(depth + 1) - 1) / 3) + 3);
        assert_same(&base, &recount(&initial, &CellIndex::new()));
        drop(initial_cells);
        drop(initial);
        let supplied_count = CellIndex::collect(Arc::new(roots[0].clone()))
            .unwrap()
            .len();
        assert_eq!(supplied_count, depth as usize + 3);

        // Check both directions, including counts, before measuring anything.
        let mut production = base.clone();
        let mut hypothetical = unique_copy(&base);
        let mut full = (base.cells.as_ref().clone(), base.bytes);
        for root in &roots {
            let supplied = CellIndex::collect(Arc::new(root.clone())).unwrap();
            production.replace_root(root.id(), &supplied).unwrap();
            delta_only(&mut hypothetical, root);
            full = recount(root, &full.0);
            assert_same(&production, &full);
            assert_same(&hypothetical, &full);
            assert_eq!(production.references, hypothetical.references);
        }
        drop((production, hypothetical, full));

        let mut state = base.clone();
        let mut step = 0;
        report(&base, supplied_count, "incremental", || {
            let root = &roots[step % 2];
            step += 1;
            let supplied = CellIndex::collect(Arc::new(root.clone())).unwrap();
            state.replace_root(black_box(root.id()), &supplied).unwrap();
            black_box(state.bytes);
        });
        drop(state);

        let mut full = (base.cells.as_ref().clone(), base.bytes);
        let mut step = 0;
        report(&base, supplied_count, "full_recount", || {
            let root = &roots[step % 2];
            step += 1;
            full = recount(black_box(root), &full.0);
            black_box(full.1);
        });
        drop(full);

        // Includes allocation, Arc increments/decrements and map teardown, just
        // like the measured updates. Shared cell payloads are not copied.
        // Copy the evolving maps, not the differently laid-out initial index.
        let mut state = base.clone();
        report(&base, supplied_count, "both_map_clones", || {
            state = black_box(unique_copy(&state));
        });
        drop(state);
        let mut refs = Arc::clone(&base.references);
        report(&base, supplied_count, "reference_map_clone", || {
            let mut next = Arc::clone(&refs);
            black_box(Arc::make_mut(&mut next));
            refs = black_box(next);
        });
        drop(refs);
        let mut cells = Arc::clone(&base.cells);
        report(&base, supplied_count, "cell_index_clone", || {
            let mut next = Arc::clone(&cells);
            black_box(Arc::make_mut(&mut next));
            cells = black_box(next);
        });
        drop(cells);

        let mut state = unique_copy(&base);
        let mut step = 0;
        report(&base, supplied_count, "delta_only_non_atomic", || {
            let root = &roots[step % 2];
            step += 1;
            delta_only(&mut state, black_box(root));
            black_box(state.bytes);
        });
        drop(state);

        let mut state = base.clone();
        let root = roots[1].detached();
        report(&base, 1, "unchanged_root_control", || {
            let supplied = CellIndex::collect(Arc::new(root.clone())).unwrap();
            state.replace_root(black_box(root.id()), &supplied).unwrap();
            black_box(state.bytes);
        });
        drop(state);
        report_profile(&base, &roots, supplied_count);
    }
}
