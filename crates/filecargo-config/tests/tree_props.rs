#![allow(clippy::unwrap_used, clippy::expect_used)] // test helpers
//! Property test: any sequence of operations keeps the tree valid and durable.

use filecargo_config::{ConfigStore, FolderId, NodeId, Paths, Protocol, ServerTree, Site, TreeOp};
use proptest::prelude::*;

const NAMES: [&str; 5] = ["a", "b", "C", "c", "web"];

#[derive(Debug, Clone)]
enum Step {
    AddFolder(usize, usize),
    AddSite(usize, usize),
    Rename(usize, usize),
    Move(usize, usize),
    Duplicate(usize),
    Delete(usize),
}

fn step() -> impl Strategy<Value = Step> {
    let n = NAMES.len();
    prop_oneof![
        (0..n, any::<usize>()).prop_map(|(a, b)| Step::AddFolder(a, b)),
        (0..n, any::<usize>()).prop_map(|(a, b)| Step::AddSite(a, b)),
        (any::<usize>(), 0..n).prop_map(|(a, b)| Step::Rename(a, b)),
        (any::<usize>(), any::<usize>()).prop_map(|(a, b)| Step::Move(a, b)),
        any::<usize>().prop_map(Step::Duplicate),
        any::<usize>().prop_map(Step::Delete),
    ]
}

fn nodes(tree: &ServerTree) -> Vec<NodeId> {
    tree.folders()
        .iter()
        .map(|f| NodeId::Folder(f.id))
        .chain(tree.sites().iter().map(|s| NodeId::Site(s.id)))
        .collect()
}

fn pick_node(tree: &ServerTree, i: usize) -> Option<NodeId> {
    let all = nodes(tree);
    (!all.is_empty()).then(|| all[i % all.len()])
}

/// `0` means root, otherwise the `(i-1)`th folder.
fn pick_parent(tree: &ServerTree, i: usize) -> Option<FolderId> {
    let folders = tree.folders();
    match i % (folders.len() + 1) {
        0 => None,
        k => Some(folders[k - 1].id),
    }
}

fn to_op(tree: &ServerTree, step: &Step) -> Option<TreeOp> {
    Some(match *step {
        Step::AddFolder(name, parent) => TreeOp::AddFolder {
            name: NAMES[name].into(),
            parent: pick_parent(tree, parent),
        },
        Step::AddSite(name, folder) => {
            let mut s = Site::new(NAMES[name], Protocol::Sftp, "example.com");
            s.folder = pick_parent(tree, folder);
            TreeOp::AddSite(s)
        }
        Step::Rename(node, name) => TreeOp::Rename {
            node: pick_node(tree, node)?,
            name: NAMES[name].into(),
        },
        Step::Move(node, parent) => TreeOp::Move {
            node: pick_node(tree, node)?,
            parent: pick_parent(tree, parent),
        },
        Step::Duplicate(site) => {
            let sites = tree.sites();
            if sites.is_empty() {
                return None;
            }
            TreeOp::Duplicate {
                site: sites[site % sites.len()].id,
            }
        }
        Step::Delete(node) => TreeOp::Delete {
            node: pick_node(tree, node)?,
        },
    })
}

proptest! {
    #![proptest_config(ProptestConfig::default())] // 256 cases; PROPTEST_CASES=1000 for the full run

    #[test]
    fn ops_round_trip_through_disk(steps in proptest::collection::vec(step(), 1..25)) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::from_override(Some(dir.path().join("cfg")));
        let mut store = ConfigStore::open(paths.clone()).unwrap();

        for step in &steps {
            let Some(op) = to_op(store.tree(), step) else { continue };
            let before = store.tree().clone();
            match store.apply(op) {
                // Reopening re-validates every rule, so this also proves the invariants.
                Ok(_) => {
                    let reopened = ConfigStore::open(paths.clone()).unwrap();
                    prop_assert_eq!(reopened.tree(), store.tree());
                }
                Err(_) => prop_assert_eq!(store.tree(), &before),
            }
        }
    }
}
