use super::*;
use cells::CellRef;

use super::super::collect_owned;

fn cell(payload: u8, children: Vec<CellRef>) -> Cell {
    Cell::new(vec![payload], children).unwrap()
}

fn supplied(root: &Cell) -> CellIndex {
    CellIndex::collect(Arc::new(root.clone())).unwrap()
}

fn assert_matches_fresh_collection(accounting: &RetainedCells, expected: &CellIndex) {
    assert_eq!(accounting.cells.id().unwrap(), expected.id().unwrap());
    let expected_bytes: u64 = expected
        .iter()
        .map(|(_, body)| body.record_size() as u64)
        .sum();
    assert_eq!(accounting.bytes, expected_bytes);

    let mut references = BTreeMap::new();
    if let Some(root) = accounting.root {
        references.insert(root, 1u64);
    }
    for (_, body) in expected.iter() {
        for child in body.refs() {
            *references.entry(child.id()).or_default() += 1;
        }
    }
    assert_eq!(*accounting.references, references);
}

fn update_and_check(accounting: &mut RetainedCells, root: &Cell, supplied: &CellIndex) {
    let mut available = accounting.cells.as_ref().clone();
    available.extend(supplied).unwrap();
    let expected = collect_owned([Arc::new(root.detached())], &available).unwrap();
    accounting.replace_root(root.id(), supplied).unwrap();
    assert_matches_fresh_collection(accounting, &expected);
}

#[test]
fn shared_and_duplicate_edges_are_counted_without_multiplying_descendants() {
    let grandchild = cell(1, vec![]);
    let shared = cell(2, vec![grandchild.clone().into()]);
    let left = cell(3, vec![shared.clone().into(), shared.clone().into()]);
    let right = cell(4, vec![shared.clone().into()]);
    let root = cell(5, vec![left.clone().into(), right.clone().into()]);
    let mut accounting = RetainedCells::default();
    update_and_check(&mut accounting, &root, &supplied(&root));
    assert_eq!(accounting.references[&shared.id()], 3);
    assert_eq!(accounting.references[&grandchild.id()], 1);
    assert_eq!(accounting.cells.len(), 5);

    // Keeping one branch removes only the other branch's incoming edge.
    update_and_check(&mut accounting, &left, &CellIndex::new());
    assert_eq!(accounting.references[&shared.id()], 2);
    assert_eq!(accounting.references[&grandchild.id()], 1);
    assert_eq!(accounting.cells.len(), 3);

    // The final incoming edge owns the full remaining descendant graph.
    let replacement = cell(6, vec![]);
    update_and_check(&mut accounting, &replacement, &supplied(&replacement));
    assert_eq!(accounting.cells.len(), 1);
    assert_eq!(accounting.references.len(), 1);
}

#[test]
fn replacement_adds_new_root_before_releasing_shared_bodies() {
    let descendant = cell(1, vec![]);
    let shared = cell(2, vec![descendant.into()]);
    let old = cell(3, vec![shared.clone().into()]);
    let new = cell(4, vec![shared.clone().into()]);
    let mut accounting = RetainedCells::default();
    update_and_check(&mut accounting, &old, &supplied(&old));
    let retained_shared = accounting.cells.get(&shared.id()).unwrap();

    update_and_check(&mut accounting, &new, &supplied(&new));
    assert!(Arc::ptr_eq(
        &retained_shared,
        &accounting.cells.get(&shared.id()).unwrap()
    ));
    assert_eq!(accounting.references[&shared.id()], 1);
}

#[test]
fn detached_new_root_keeps_previously_owned_descendants() {
    let descendant = cell(1, vec![]);
    let shared = cell(2, vec![descendant.clone().into()]);
    let old = cell(3, vec![shared.clone().into()]);
    let new = cell(4, vec![CellRef::resident(shared.clone()).to_unloaded()]);
    let mut accounting = RetainedCells::default();
    update_and_check(&mut accounting, &old, &supplied(&old));
    let bodies = supplied(&new);
    assert_eq!(bodies.len(), 1);
    update_and_check(&mut accounting, &new, &bodies);
    assert!(accounting.cells.contains(&shared.id()));
    assert!(accounting.cells.contains(&descendant.id()));
}

#[test]
fn same_hash_root_can_supply_a_richer_frontier() {
    let left = cell(1, vec![]);
    let right = cell(2, vec![]);
    let initial = cell(
        3,
        vec![
            left.clone().into(),
            CellRef::resident(right.clone()).to_unloaded(),
        ],
    );
    let richer = cell(
        3,
        vec![
            CellRef::resident(left.clone()).to_unloaded(),
            right.clone().into(),
        ],
    );
    assert_eq!(initial.id(), richer.id());
    let mut accounting = RetainedCells::default();
    update_and_check(&mut accounting, &initial, &supplied(&initial));
    let before = accounting.bytes;
    assert!(!accounting.cells.contains(&right.id()));
    assert_eq!(accounting.references[&right.id()], 1);

    update_and_check(&mut accounting, &richer, &supplied(&richer));
    assert_eq!(accounting.bytes, before + right.record_size() as u64);
    assert!(accounting.cells.contains(&left.id()));
    assert_eq!(accounting.references[&initial.id()], 1);
    let after = accounting.bytes;
    update_and_check(&mut accounting, &initial, &supplied(&initial));
    assert_eq!(accounting.bytes, after);
}

#[test]
fn late_missing_body_activates_edges_once_for_all_waiting_parents() {
    let descendant = cell(1, vec![]);
    let missing = cell(2, vec![descendant.clone().into()]);
    let unloaded = CellRef::resident(missing.clone()).to_unloaded();
    let left = cell(3, vec![unloaded.clone(), unloaded.clone()]);
    let right = cell(4, vec![unloaded]);
    let root = cell(5, vec![left.into(), right.into()]);
    let mut accounting = RetainedCells::default();
    update_and_check(&mut accounting, &root, &supplied(&root));
    assert_eq!(accounting.references[&missing.id()], 3);
    assert!(!accounting.references.contains_key(&descendant.id()));

    // A restore may supply just the missing frontier, not the unchanged root.
    update_and_check(&mut accounting, &root, &supplied(&missing));
    assert_eq!(accounting.references[&missing.id()], 3);
    assert_eq!(accounting.references[&descendant.id()], 1);
    assert!(accounting.cells.contains(&descendant.id()));
}

#[test]
fn same_id_resident_variants_merge_and_unreachable_supplies_are_ignored() {
    let left = cell(1, vec![]);
    let right = cell(2, vec![]);
    let first = cell(
        3,
        vec![
            left.clone().into(),
            CellRef::resident(right.clone()).to_unloaded(),
        ],
    );
    let second = cell(
        3,
        vec![
            CellRef::resident(left.clone()).to_unloaded(),
            right.clone().into(),
        ],
    );
    assert_eq!(first.id(), second.id());
    let root = cell(4, vec![first.clone().into(), second.into()]);
    let unrelated = cell(9, vec![]);
    let mut bodies = supplied(&root);
    bodies.insert(Arc::new(unrelated.clone())).unwrap();
    let mut accounting = RetainedCells::default();
    update_and_check(&mut accounting, &root, &bodies);
    assert_eq!(accounting.references[&first.id()], 2);
    assert_eq!(accounting.references[&left.id()], 1);
    assert_eq!(accounting.references[&right.id()], 1);
    assert!(!accounting.cells.contains(&unrelated.id()));
}

#[test]
fn missing_root_leaves_accounting_and_snapshots_unchanged() {
    let root = cell(1, vec![cell(2, vec![]).into()]);
    let mut accounting = RetainedCells::default();
    update_and_check(&mut accounting, &root, &supplied(&root));
    let before = accounting.clone();
    let missing = cell(3, vec![]).id();
    assert_eq!(
        accounting.replace_root(missing, &CellIndex::new()),
        Err(CellError::MissingCell(missing))
    );
    assert_eq!(accounting.root, before.root);
    assert_eq!(accounting.bytes, before.bytes);
    assert!(Arc::ptr_eq(&accounting.cells, &before.cells));
    assert!(Arc::ptr_eq(&accounting.references, &before.references));
}

#[test]
fn deterministic_updates_match_full_collection_and_preserve_snapshots() {
    let mut nodes: Vec<Cell> = (0..8).map(|n| cell(n, vec![])).collect();
    let mut seed = 0x7d41_92e5_u64;
    let mut next = || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        (seed >> 32) as usize
    };
    let mut accounting = RetainedCells::default();
    for step in 0..160 {
        let count = next() % 5;
        let children = (0..count)
            .map(|_| {
                let child = CellRef::resident(nodes[next() % nodes.len()].clone());
                if next() % 2 == 0 {
                    child
                } else {
                    child.to_unloaded()
                }
            })
            .collect();
        let root = cell((step + 8) as u8, children);
        let mut bodies = supplied(&root);
        // Occasionally reveal an earlier body. It can be reachable only through
        // old unloaded edges, or entirely unrelated to the new root.
        if step % 3 == 0 {
            bodies
                .extend(&supplied(&nodes[next() % nodes.len()]))
                .unwrap();
        }
        let snapshot = accounting.clone();
        let snapshot_id = snapshot.cells.id().unwrap();
        let snapshot_bytes = snapshot.bytes;
        update_and_check(&mut accounting, &root, &bodies);
        assert_eq!(snapshot.cells.id().unwrap(), snapshot_id);
        assert_eq!(snapshot.bytes, snapshot_bytes);
        nodes.push(root);
    }
}
