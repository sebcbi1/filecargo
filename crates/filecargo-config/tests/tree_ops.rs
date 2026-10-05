#![allow(clippy::unwrap_used, clippy::expect_used)] // test helpers
use std::fs;

mod common;

use filecargo_config::{
    ConfigError, ConfigStore, Node, NodeId, Paths, Protocol, Site, TreeOp, ValidationError,
};

struct Fixture {
    dir: tempfile::TempDir,
    store: ConfigStore,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = common::open(Paths::from_override(Some(dir.path().join("cfg")))).unwrap();
        Self { dir, store }
    }

    fn file(&self) -> Vec<u8> {
        fs::read(self.dir.path().join("cfg/servers.toml")).unwrap()
    }

    fn folder(
        &mut self,
        name: &str,
        parent: Option<filecargo_config::FolderId>,
    ) -> filecargo_config::FolderId {
        match self
            .store
            .apply(TreeOp::AddFolder {
                name: name.into(),
                parent,
            })
            .unwrap()
        {
            NodeId::Folder(id) => id,
            NodeId::Site(_) => unreachable!(),
        }
    }

    fn site(
        &mut self,
        name: &str,
        folder: Option<filecargo_config::FolderId>,
    ) -> filecargo_config::SiteId {
        let mut s = Site::new(name, Protocol::Sftp, "example.com");
        s.folder = folder;
        let id = s.id;
        self.store.apply(TreeOp::AddSite(s)).unwrap();
        id
    }

    /// Asserts `op` is rejected with a validation error and the file bytes did not change.
    fn rejected(&mut self, op: TreeOp) -> ValidationError {
        let before = self.file();
        let err = self.store.apply(op).unwrap_err();
        assert_eq!(self.file(), before, "file must be unchanged");
        match err {
            ConfigError::Invalid(v) => v,
            other => panic!("expected validation error, got {other}"),
        }
    }
}

#[test]
fn children_list_folders_first_then_sites_alphabetically() {
    let mut f = Fixture::new();
    f.site("beta", None);
    f.site("Alpha", None);
    f.folder("zeta", None);
    f.folder("Eta", None);

    let names: Vec<String> = f
        .store
        .tree()
        .children(None)
        .iter()
        .map(|n| match n {
            Node::Folder(x) => format!("dir:{}", x.name),
            Node::Site(x) => format!("site:{}", x.name),
        })
        .collect();
    assert_eq!(names, ["dir:Eta", "dir:zeta", "site:Alpha", "site:beta"]);
}

#[test]
fn sites_in_folders_survive_reopen() {
    let mut f = Fixture::new();
    let work = f.folder("Work", None);
    let inner = f.folder("Inner", Some(work));
    let s = f.site("prod", Some(inner));

    let reopened = common::open(Paths::from_override(Some(f.dir.path().join("cfg")))).unwrap();
    assert_eq!(reopened.tree(), f.store.tree());
    assert_eq!(reopened.tree().site(s).unwrap().folder, Some(inner));
}

#[test]
fn unknown_folder_is_rejected_for_new_site_and_folder() {
    let mut f = Fixture::new();
    f.site("seed", None);
    let ghost = f.folder("tmp", None);
    f.store
        .apply(TreeOp::Delete {
            node: NodeId::Folder(ghost),
        })
        .unwrap();

    let mut s = Site::new("a", Protocol::Sftp, "h");
    s.folder = Some(ghost);
    assert_eq!(
        f.rejected(TreeOp::AddSite(s)),
        ValidationError::UnknownFolder(ghost)
    );
    assert_eq!(
        f.rejected(TreeOp::AddFolder {
            name: "x".into(),
            parent: Some(ghost)
        }),
        ValidationError::UnknownFolder(ghost)
    );
}

#[test]
fn empty_and_duplicate_folder_names_are_rejected() {
    let mut f = Fixture::new();
    f.folder("Work", None);
    assert_eq!(
        f.rejected(TreeOp::AddFolder {
            name: "  ".into(),
            parent: None
        }),
        ValidationError::EmptyName
    );
    assert!(matches!(
        f.rejected(TreeOp::AddFolder {
            name: "work".into(),
            parent: None
        }),
        ValidationError::DuplicateName { .. }
    ));
}

#[test]
fn site_and_folder_share_one_name_space_per_parent() {
    let mut f = Fixture::new();
    f.folder("prod", None);
    let mut s = Site::new("PROD", Protocol::Ftp, "h");
    s.auth = filecargo_config::Auth::Anonymous;
    assert!(matches!(
        f.rejected(TreeOp::AddSite(s)),
        ValidationError::DuplicateName { .. }
    ));
}

#[test]
fn same_name_is_fine_in_different_folders() {
    let mut f = Fixture::new();
    let a = f.folder("a", None);
    let b = f.folder("b", None);
    f.site("web", Some(a));
    f.site("web", Some(b));
}

#[test]
fn rename_validates_and_persists() {
    let mut f = Fixture::new();
    let a = f.site("a", None);
    f.site("b", None);

    assert!(matches!(
        f.rejected(TreeOp::Rename {
            node: NodeId::Site(a),
            name: "B".into()
        }),
        ValidationError::DuplicateName { .. }
    ));
    f.store
        .apply(TreeOp::Rename {
            node: NodeId::Site(a),
            name: "c".into(),
        })
        .unwrap();
    assert_eq!(f.store.tree().site(a).unwrap().name, "c");
}

#[test]
fn move_into_self_or_descendant_is_rejected() {
    let mut f = Fixture::new();
    let a = f.folder("a", None);
    let b = f.folder("b", Some(a));
    let c = f.folder("c", Some(b));

    for target in [a, b, c] {
        assert_eq!(
            f.rejected(TreeOp::Move {
                node: NodeId::Folder(a),
                parent: Some(target)
            }),
            ValidationError::MoveIntoDescendant,
            "moving a into {target}"
        );
    }
    f.store
        .apply(TreeOp::Move {
            node: NodeId::Folder(c),
            parent: None,
        })
        .unwrap();
    assert_eq!(f.store.tree().folder(c).unwrap().parent, None);
}

#[test]
fn move_site_between_folders_and_collision_is_rejected() {
    let mut f = Fixture::new();
    let a = f.folder("a", None);
    let b = f.folder("b", None);
    let s = f.site("web", Some(a));
    f.site("web", Some(b));

    assert!(matches!(
        f.rejected(TreeOp::Move {
            node: NodeId::Site(s),
            parent: Some(b)
        }),
        ValidationError::DuplicateName { .. }
    ));
    f.store
        .apply(TreeOp::Move {
            node: NodeId::Site(s),
            parent: None,
        })
        .unwrap();
    assert_eq!(f.store.tree().site(s).unwrap().folder, None);
}

#[test]
fn delete_folder_removes_descendants_and_sites() {
    let mut f = Fixture::new();
    let a = f.folder("a", None);
    let b = f.folder("b", Some(a));
    f.site("one", Some(a));
    f.site("two", Some(b));
    let keep = f.site("keep", None);

    f.store
        .apply(TreeOp::Delete {
            node: NodeId::Folder(a),
        })
        .unwrap();

    assert!(f.store.tree().folders().is_empty());
    let left: Vec<_> = f.store.tree().sites().iter().map(|s| s.id).collect();
    assert_eq!(left, [keep]);
}

#[test]
fn delete_unknown_node_is_rejected() {
    let mut f = Fixture::new();
    let gone = f.site("x", None);
    f.store
        .apply(TreeOp::Delete {
            node: NodeId::Site(gone),
        })
        .unwrap();
    assert_eq!(
        f.rejected(TreeOp::Delete {
            node: NodeId::Site(gone)
        }),
        ValidationError::UnknownNode
    );
}

#[test]
fn duplicate_gets_new_id_and_numbered_copy_names() {
    let mut f = Fixture::new();
    let s = f.site("web", None);

    let mut names = Vec::new();
    for _ in 0..3 {
        let NodeId::Site(copy) = f.store.apply(TreeOp::Duplicate { site: s }).unwrap() else {
            unreachable!()
        };
        assert_ne!(copy, s);
        names.push(f.store.tree().site(copy).unwrap().name.clone());
    }
    assert_eq!(names, ["web (copy)", "web (copy 2)", "web (copy 3)"]);
}
