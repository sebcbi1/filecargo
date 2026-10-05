//! The server tree as a flat list of rows, and the cursor / expansion state over it.

use std::collections::HashSet;

use filecargo_app_core::prelude::*;

/// What one row stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowKind {
    Folder { id: FolderId, expanded: bool },
    Site { id: SiteId, protocol: Protocol },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRow {
    pub depth: usize,
    pub name: String,
    pub kind: RowKind,
}

impl TreeRow {
    pub fn node(&self) -> NodeId {
        match self.kind {
            RowKind::Folder { id, .. } => NodeId::Folder(id),
            RowKind::Site { id, .. } => NodeId::Site(id),
        }
    }
}

/// Cursor and open folders of the tree pane.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TreeUi {
    pub cursor: usize,
    pub offset: usize,
    pub expanded: HashSet<FolderId>,
}

fn walk(
    tree: &ServerTree,
    parent: Option<FolderId>,
    depth: usize,
    expanded: &HashSet<FolderId>,
    out: &mut Vec<TreeRow>,
) {
    for node in tree.children(parent) {
        match node {
            Node::Folder(folder) => {
                let open = expanded.contains(&folder.id);
                out.push(TreeRow {
                    depth,
                    name: folder.name.clone(),
                    kind: RowKind::Folder {
                        id: folder.id,
                        expanded: open,
                    },
                });
                if open {
                    walk(tree, Some(folder.id), depth + 1, expanded, out);
                }
            }
            Node::Site(site) => out.push(TreeRow {
                depth,
                name: site.name.clone(),
                kind: RowKind::Site {
                    id: site.id,
                    protocol: site.protocol,
                },
            }),
        }
    }
}

/// The visible rows: folders first, then sites, each alphabetical (the order `ServerTree`
/// gives), children only under open folders.
pub fn rows(tree: &ServerTree, expanded: &HashSet<FolderId>) -> Vec<TreeRow> {
    let mut out = Vec::new();
    walk(tree, None, 0, expanded, &mut out);
    out
}

/// The parent folder row of `index`, for `←` on a collapsed node.
pub fn parent_row(rows: &[TreeRow], index: usize) -> Option<usize> {
    let depth = rows.get(index)?.depth;
    (depth > 0)
        .then(|| rows[..index].iter().rposition(|r| r.depth == depth - 1))
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::sample_tree;

    fn names(rows: &[TreeRow]) -> Vec<String> {
        rows.iter()
            .map(|r| format!("{}{}", "  ".repeat(r.depth), r.name))
            .collect()
    }

    #[test]
    fn collapsed_folders_hide_their_children_and_folders_sort_first() {
        let tree = sample_tree();
        assert_eq!(
            names(&rows(&tree, &HashSet::new())),
            ["Personal", "Work", "home-nas"]
        );
    }

    #[test]
    fn open_folders_show_their_children_indented() {
        let tree = sample_tree();
        let work = tree.folders().iter().find(|f| f.name == "Work").unwrap().id;
        let open: HashSet<_> = [work].into();
        assert_eq!(
            names(&rows(&tree, &open)),
            ["Personal", "Work", "  prod-web", "  staging", "home-nas"]
        );
    }

    #[test]
    fn the_parent_row_is_the_nearest_shallower_row_above() {
        let tree = sample_tree();
        let all: HashSet<_> = tree.folders().iter().map(|f| f.id).collect();
        let r = rows(&tree, &all);
        // Personal, blog, Work, prod-web, staging, home-nas
        assert_eq!(parent_row(&r, 1), Some(0));
        assert_eq!(parent_row(&r, 4), Some(2));
        assert_eq!(parent_row(&r, 0), None);
        assert_eq!(parent_row(&r, 5), None);
    }
}
