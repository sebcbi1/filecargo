#![allow(clippy::unwrap_used, clippy::expect_used)] // test helpers
//! Several stores on one config directory, and unreadable files.

use std::fs;
use std::thread;

use filecargo_config::{ConfigError, ConfigStore, Paths, Protocol, Site, TreeOp};

fn paths(dir: &tempfile::TempDir) -> Paths {
    Paths::from_override(Some(dir.path().join("cfg")))
}

fn site(name: &str) -> Site {
    Site::new(name, Protocol::Sftp, "example.com")
}

fn servers_file(dir: &tempfile::TempDir) -> std::path::PathBuf {
    dir.path().join("cfg/servers.toml")
}

#[test]
fn stale_store_merges_instead_of_overwriting() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = ConfigStore::open(paths(&dir)).unwrap();
    let mut b = ConfigStore::open(paths(&dir)).unwrap();

    let x = site("X");
    let y = site("Y");
    a.apply(TreeOp::AddSite(x.clone())).unwrap();
    b.apply(TreeOp::AddSite(y.clone())).unwrap();

    let reopened = ConfigStore::open(paths(&dir)).unwrap();
    assert!(reopened.tree().site(x.id).is_some());
    assert!(reopened.tree().site(y.id).is_some());
    assert!(b.tree().site(x.id).is_some(), "b must have picked up X");
}

#[test]
fn reload_picks_up_changes_from_another_instance() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = ConfigStore::open(paths(&dir)).unwrap();
    let mut b = ConfigStore::open(paths(&dir)).unwrap();
    let x = site("X");
    a.apply(TreeOp::AddSite(x.clone())).unwrap();

    assert!(b.tree().site(x.id).is_none());
    b.reload().unwrap();
    assert!(b.tree().site(x.id).is_some());
}

#[test]
fn concurrent_writers_lose_no_operation() {
    const PER_THREAD: usize = 50;
    let dir = tempfile::tempdir().unwrap();
    let p = paths(&dir);

    let handles: Vec<_> = (0..2)
        .map(|t| {
            let p = p.clone();
            thread::spawn(move || {
                let mut store = ConfigStore::open(p).unwrap();
                for i in 0..PER_THREAD {
                    store
                        .apply(TreeOp::AddSite(site(&format!("t{t}-{i}"))))
                        .unwrap();
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }

    let store = ConfigStore::open(p).unwrap();
    assert_eq!(store.tree().sites().len(), 2 * PER_THREAD);
}

#[test]
fn corrupt_file_reports_line_and_is_never_modified() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("cfg")).unwrap();
    let bad = "version = 1\nfoo = = 2\n";
    fs::write(servers_file(&dir), bad).unwrap();

    let err = ConfigStore::open(paths(&dir)).unwrap_err();
    match err {
        ConfigError::Parse { line, .. } => assert_eq!(line, 2),
        other => panic!("expected parse error, got {other}"),
    }
    assert_eq!(fs::read_to_string(servers_file(&dir)).unwrap(), bad);
}

#[test]
fn apply_after_external_corruption_fails_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = ConfigStore::open(paths(&dir)).unwrap();
    store.apply(TreeOp::AddSite(site("a"))).unwrap();

    let bad = "version = 1\n[[site]\n";
    fs::write(servers_file(&dir), bad).unwrap();
    let err = store.apply(TreeOp::AddSite(site("b"))).unwrap_err();

    assert!(matches!(err, ConfigError::Parse { .. }), "{err}");
    assert_eq!(fs::read_to_string(servers_file(&dir)).unwrap(), bad);
    assert_eq!(store.tree().sites().len(), 1, "in-memory tree kept");
}

#[test]
fn structurally_invalid_file_is_rejected_on_open() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("cfg")).unwrap();
    fs::write(
        servers_file(&dir),
        "version = 1\n[[site]]\nid = \"7c1e0000-0000-4000-8000-000000000001\"\nname = \"a\"\nprotocol = \"sftp\"\nhost = \"\"\n[site.auth]\nmethod = \"agent\"\n",
    )
    .unwrap();
    let err = ConfigStore::open(paths(&dir)).unwrap_err();
    assert!(matches!(err, ConfigError::Invalid(_)), "{err}");
}

#[test]
fn newer_file_version_is_rejected_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("cfg")).unwrap();
    let newer = "version = 2\n";
    fs::write(servers_file(&dir), newer).unwrap();

    let err = ConfigStore::open(paths(&dir)).unwrap_err();
    assert!(
        matches!(
            err,
            ConfigError::UnsupportedVersion {
                found: 2,
                supported: 1,
                ..
            }
        ),
        "{err}"
    );
    assert_eq!(fs::read_to_string(servers_file(&dir)).unwrap(), newer);
}

#[test]
fn reset_keeps_a_backup_and_a_fresh_store_works() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("cfg")).unwrap();
    let bad = "not toml at all = =\n";
    fs::write(servers_file(&dir), bad).unwrap();
    assert!(ConfigStore::open(paths(&dir)).is_err());

    let backup = ConfigStore::reset(&paths(&dir)).unwrap().unwrap();
    assert_eq!(fs::read_to_string(&backup).unwrap(), bad);
    assert!(
        backup
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("servers.toml.bak-")
    );
    assert!(!servers_file(&dir).exists());

    let mut store = ConfigStore::open(paths(&dir)).unwrap();
    store.apply(TreeOp::AddSite(site("fresh"))).unwrap();
    assert_eq!(store.tree().sites().len(), 1);
}

#[test]
fn reset_without_a_file_does_nothing() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(ConfigStore::reset(&paths(&dir)).unwrap(), None);
}
