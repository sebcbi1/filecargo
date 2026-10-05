#![allow(clippy::unwrap_used, clippy::expect_used)] // test helpers

mod common;

use std::fmt;
use std::fs;
use std::io::Write;
use std::sync::{Arc, Mutex};

use filecargo_config::{
    Auth, ExposeSecret, FolderId, KeyringStore, MemoryStore, NodeId, Paths, Protocol, SecretError,
    SecretKey, SecretStore, SecretString, Site, SiteId, TreeOp,
};

fn paths(dir: &tempfile::TempDir) -> Paths {
    Paths::from_override(Some(dir.path().join("cfg")))
}

fn secret(s: &str) -> SecretString {
    SecretString::from(s.to_owned())
}

fn remembered_site(name: &str, folder: Option<FolderId>) -> Site {
    let mut s = Site::new(name, Protocol::Sftp, "example.com");
    s.folder = folder;
    s.auth = Auth::Password { remember: true };
    s
}

fn put(secrets: &MemoryStore, key: SecretKey, value: &str) {
    secrets.set(&key, &secret(value)).unwrap();
}

fn has(secrets: &MemoryStore, key: SecretKey) -> bool {
    secrets.get(&key).unwrap().is_some()
}

#[test]
fn memory_store_round_trip_and_idempotent_delete() {
    let store = MemoryStore::new();
    let key = SecretKey::Password(SiteId::new());
    assert!(store.get(&key).unwrap().is_none());
    put(&store, key, "hunter2");
    assert_eq!(store.get(&key).unwrap().unwrap().expose_secret(), "hunter2");
    store.delete(&key).unwrap();
    store.delete(&key).unwrap();
    assert!(store.is_empty());
}

#[test]
fn secret_keys_are_distinct_per_kind_and_site() {
    let (a, b) = (SiteId::new(), SiteId::new());
    let keys = [
        SecretKey::Password(a),
        SecretKey::Passphrase(a),
        SecretKey::Password(b),
    ];
    let accounts: std::collections::HashSet<_> = keys.iter().map(|k| k.account()).collect();
    assert_eq!(accounts.len(), 3);
    assert_eq!(
        SecretKey::Password(a).account(),
        format!("site:{a}:password")
    );
}

#[test]
fn keyring_store_round_trips_through_a_keyring_core_store() {
    let backend = keyring_core::mock::Store::new().unwrap();
    let store = KeyringStore::from_store(backend, "filecargo-test");
    let key = SecretKey::Passphrase(SiteId::new());

    assert!(store.get(&key).unwrap().is_none());
    store.set(&key, &secret("correct horse")).unwrap();
    assert_eq!(
        store.get(&key).unwrap().unwrap().expose_secret(),
        "correct horse"
    );
    store.set(&key, &secret("battery staple")).unwrap();
    assert_eq!(
        store.get(&key).unwrap().unwrap().expose_secret(),
        "battery staple"
    );
    store.delete(&key).unwrap();
    store.delete(&key).unwrap();
    assert!(store.get(&key).unwrap().is_none());
}

#[test]
fn deleting_a_folder_with_three_sites_removes_their_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemoryStore::new());
    let mut store = common::open_with(paths(&dir), secrets.clone()).unwrap();

    let NodeId::Folder(folder) = store
        .apply(TreeOp::AddFolder {
            name: "Work".into(),
            parent: None,
        })
        .unwrap()
    else {
        unreachable!()
    };
    let mut ids = Vec::new();
    for name in ["a", "b", "c"] {
        let site = remembered_site(name, Some(folder));
        ids.push(site.id);
        store.apply(TreeOp::AddSite(site)).unwrap();
    }
    let outside = remembered_site("outside", None);
    let outside_id = outside.id;
    store.apply(TreeOp::AddSite(outside)).unwrap();
    for id in ids.iter().chain([&outside_id]) {
        put(&secrets, SecretKey::Password(*id), "pw");
        put(&secrets, SecretKey::Passphrase(*id), "pp");
    }
    assert_eq!(secrets.len(), 8);

    store
        .apply(TreeOp::Delete {
            node: NodeId::Folder(folder),
        })
        .unwrap();

    assert_eq!(store.tree().sites().len(), 1);
    for id in &ids {
        assert!(!has(&secrets, SecretKey::Password(*id)));
        assert!(!has(&secrets, SecretKey::Passphrase(*id)));
    }
    assert!(
        has(&secrets, SecretKey::Password(outside_id)),
        "others stay"
    );
    assert_eq!(secrets.len(), 2);
}

#[test]
fn deleting_a_single_site_removes_its_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemoryStore::new());
    let mut store = common::open_with(paths(&dir), secrets.clone()).unwrap();
    let site = remembered_site("a", None);
    let id = site.id;
    store.apply(TreeOp::AddSite(site)).unwrap();
    put(&secrets, SecretKey::Password(id), "pw");

    store
        .apply(TreeOp::Delete {
            node: NodeId::Site(id),
        })
        .unwrap();
    assert!(secrets.is_empty());
}

#[test]
fn turning_remember_off_deletes_the_secret_but_other_edits_keep_it() {
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemoryStore::new());
    let mut store = common::open_with(paths(&dir), secrets.clone()).unwrap();
    let mut site = remembered_site("a", None);
    store.apply(TreeOp::AddSite(site.clone())).unwrap();
    put(&secrets, SecretKey::Password(site.id), "pw");

    site.host = "other.example.com".into();
    store.apply(TreeOp::UpdateSite(site.clone())).unwrap();
    assert!(
        has(&secrets, SecretKey::Password(site.id)),
        "unrelated edit"
    );

    site.auth = Auth::Password { remember: false };
    store.apply(TreeOp::UpdateSite(site.clone())).unwrap();
    assert!(!has(&secrets, SecretKey::Password(site.id)));
}

#[test]
fn switching_a_key_site_off_remembering_deletes_only_the_passphrase() {
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemoryStore::new());
    let mut store = common::open_with(paths(&dir), secrets.clone()).unwrap();
    let mut site = remembered_site("a", None);
    site.auth = Auth::KeyFile {
        path: "k".into(),
        remember_passphrase: true,
    };
    store.apply(TreeOp::AddSite(site.clone())).unwrap();
    put(&secrets, SecretKey::Passphrase(site.id), "pp");
    put(&secrets, SecretKey::Password(site.id), "stale-pw");

    site.auth = Auth::KeyFile {
        path: "k".into(),
        remember_passphrase: false,
    };
    store.apply(TreeOp::UpdateSite(site.clone())).unwrap();
    assert!(!has(&secrets, SecretKey::Passphrase(site.id)));
    assert!(has(&secrets, SecretKey::Password(site.id)), "not this kind");
}

#[test]
fn duplicate_copies_secrets_to_the_new_site() {
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemoryStore::new());
    let mut store = common::open_with(paths(&dir), secrets.clone()).unwrap();
    let site = remembered_site("a", None);
    store.apply(TreeOp::AddSite(site.clone())).unwrap();
    put(&secrets, SecretKey::Password(site.id), "pw");

    let NodeId::Site(copy) = store.apply(TreeOp::Duplicate { site: site.id }).unwrap() else {
        unreachable!()
    };
    assert_eq!(
        secrets
            .get(&SecretKey::Password(copy))
            .unwrap()
            .unwrap()
            .expose_secret(),
        "pw"
    );
    assert!(has(&secrets, SecretKey::Password(site.id)), "original kept");
}

/// A keychain whose every operation fails.
#[derive(Debug)]
struct BrokenStore;

impl SecretStore for BrokenStore {
    fn get(&self, _: &SecretKey) -> Result<Option<SecretString>, SecretError> {
        Err(SecretError::Backend("locked".into()))
    }
    fn set(&self, _: &SecretKey, _: &SecretString) -> Result<(), SecretError> {
        Err(SecretError::Backend("locked".into()))
    }
    fn delete(&self, _: &SecretKey) -> Result<(), SecretError> {
        Err(SecretError::Backend("locked".into()))
    }
}

#[test]
fn keychain_failures_are_logged_and_never_fail_the_operation() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = common::open_with(paths(&dir), Arc::new(BrokenStore)).unwrap();
    let site = remembered_site("a", None);
    let id = site.id;

    let logs = capture_logs(|| {
        store.apply(TreeOp::AddSite(site)).unwrap();
        store.apply(TreeOp::Duplicate { site: id }).unwrap();
        store
            .apply(TreeOp::Delete {
                node: NodeId::Site(id),
            })
            .unwrap();
    });

    assert_eq!(store.tree().sites().len(), 1, "duplicate remains");
    assert!(logs.contains("could not delete keychain entry"), "{logs}");
    assert!(logs.contains("WARN"), "{logs}");
}

#[derive(Clone, Default)]
struct LogBuffer(Arc<Mutex<Vec<u8>>>);

impl Write for LogBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuffer {
    type Writer = LogBuffer;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

fn capture_logs(f: impl FnOnce()) -> String {
    let buffer = LogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(buffer.clone())
        .with_ansi(false)
        .finish();
    tracing::subscriber::with_default(subscriber, f);
    String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap()
}

fn assert_clean(label: &str, text: &str, sentinel: &str) {
    assert!(
        !text.contains(sentinel),
        "{label} leaks the secret:\n{text}"
    );
}

fn debug(v: &dyn fmt::Debug) -> String {
    format!("{v:?} {v:#?}")
}

#[test]
fn sentinel_secret_never_reaches_files_debug_output_or_logs() {
    const SENTINEL: &str = "S3NT1NEL-pw-9f2c41";
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemoryStore::new());
    let mut store = common::open_with(paths(&dir), secrets.clone()).unwrap();

    let logs = capture_logs(|| {
        let mut site = remembered_site("prod", None);
        site.user = "deploy".into();
        store.apply(TreeOp::AddSite(site.clone())).unwrap();
        let key = SecretKey::Password(site.id);
        secrets.set(&key, &secret(SENTINEL)).unwrap();
        store.update_settings(|s| s.ui.show_hidden = true).unwrap();
        store.apply(TreeOp::Duplicate { site: site.id }).unwrap();
        site.auth = Auth::Password { remember: false };
        store.apply(TreeOp::UpdateSite(site)).unwrap();
        // Exercise the secret itself through every formatting path.
        let fetched = secrets.get(&key).unwrap();
        assert_clean("fetched Debug", &debug(&fetched), SENTINEL);
        assert_clean("SecretString Debug", &debug(&secret(SENTINEL)), SENTINEL);
    });

    assert_clean("tracing output", &logs, SENTINEL);
    for file in ["servers.toml", "settings.toml"] {
        let text = fs::read_to_string(dir.path().join("cfg").join(file)).unwrap();
        assert_clean(file, &text, SENTINEL);
    }
    assert_clean("ConfigStore Debug", &debug(&store), SENTINEL);
    assert_clean("MemoryStore Debug", &debug(&*secrets), SENTINEL);
    assert_clean("tree Debug", &debug(store.tree()), SENTINEL);
    assert_clean("settings Debug", &debug(store.settings()), SENTINEL);
    let keyring = KeyringStore::from_store(keyring_core::mock::Store::new().unwrap(), "t");
    keyring
        .set(&SecretKey::Password(SiteId::new()), &secret(SENTINEL))
        .unwrap();
    assert_clean("KeyringStore Debug", &debug(&keyring), SENTINEL);
    assert_clean(
        "SecretError Display",
        &SecretError::Backend("locked".into()).to_string(),
        SENTINEL,
    );
}
