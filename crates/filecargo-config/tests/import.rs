#![allow(clippy::unwrap_used, clippy::expect_used)] // test helpers

mod common;

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use filecargo_config::{
    Auth, ConfigError, ExposeSecret, FolderId, FtpMode, ImportError, ImportOptions, MemoryStore,
    Node, Paths, Protocol, SecretError, SecretKey, SecretStore, SecretString, ServerTree, Site,
    SkippedSite, default_filezilla_path,
};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/filezilla/sitemanager.xml")
}

fn paths(dir: &tempfile::TempDir) -> Paths {
    Paths::from_override(Some(dir.path().join("cfg")))
}

fn opts(import_passwords: bool) -> ImportOptions {
    ImportOptions {
        import_passwords,
        folder_name: "Imported from FileZilla (2026-10-05)".into(),
    }
}

/// `path` like `["Imported", "Work", "prod-web"]` -> the site or folder at the end.
fn find_site<'a>(tree: &'a ServerTree, path: &[&str]) -> &'a Site {
    let (name, folders) = path.split_last().unwrap();
    let mut parent: Option<FolderId> = None;
    for folder in folders {
        parent = tree
            .children(parent)
            .into_iter()
            .find_map(|n| match n {
                Node::Folder(f) if f.name == *folder => Some(f.id),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no folder {folder} in {path:?}"))
            .into();
    }
    tree.children(parent)
        .into_iter()
        .find_map(|n| match n {
            Node::Site(s) if s.name == *name => Some(s),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no site {path:?}"))
}

fn tree_shape(tree: &ServerTree) -> Vec<String> {
    fn walk(tree: &ServerTree, parent: Option<FolderId>, depth: usize, out: &mut Vec<String>) {
        for node in tree.children(parent) {
            match node {
                Node::Folder(f) => {
                    out.push(format!("{}{}/", "  ".repeat(depth), f.name));
                    walk(tree, Some(f.id), depth + 1, out);
                }
                Node::Site(s) => out.push(format!("{}{}", "  ".repeat(depth), s.name)),
            }
        }
    }
    let mut out = Vec::new();
    walk(tree, None, 0, &mut out);
    out
}

#[test]
fn fixture_imports_to_the_exact_expected_tree_and_report() {
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemoryStore::new());
    let mut store = common::open_with(paths(&dir), secrets.clone()).unwrap();

    let report = store.import_filezilla(&fixture(), opts(true)).unwrap();

    assert_eq!(
        tree_shape(store.tree()),
        [
            "Imported from FileZilla (2026-10-05)/",
            "  Work/",
            "    Internal/",
            "      intranet-ftps",
            "    prod-web",
            "  ask-me",
            "  implicit-ftps",
            "  legacy-ftp",
            "  public-mirror",
        ]
    );

    let root = "Imported from FileZilla (2026-10-05)";
    assert_eq!(report.root_folder.as_deref(), Some(root));
    let mut imported = report.imported.clone();
    imported.sort();
    assert_eq!(
        imported,
        [
            "Work/Internal/intranet-ftps",
            "Work/prod-web",
            "ask-me",
            "implicit-ftps",
            "legacy-ftp",
            "public-mirror",
        ]
    );
    assert_eq!(
        report.skipped,
        [SkippedSite {
            site: "backup-bucket".into(),
            reason: "S3 is not supported".into()
        }]
    );
    assert_eq!(report.passwords_skipped.len(), 1);
    assert_eq!(report.passwords_skipped[0].site, "implicit-ftps");
    assert!(
        report.passwords_skipped[0]
            .reason
            .contains("master password")
    );

    // Field mapping.
    let prod = find_site(store.tree(), &[root, "Work", "prod-web"]);
    assert_eq!(prod.protocol, Protocol::Sftp);
    assert_eq!(prod.port, Some(2222));
    assert_eq!(prod.user, "deploy");
    assert_eq!(
        prod.auth,
        Auth::KeyFile {
            path: "/home/me/.ssh/id_ed25519".into(),
            remember_passphrase: false
        }
    );
    assert_eq!(prod.remote_dir.as_deref(), Some("/var/www"));
    assert_eq!(prod.local_dir, Some(PathBuf::from("/home/me/projects")));
    assert_eq!(prod.notes, "production & staging gateway");

    let intranet = find_site(store.tree(), &[root, "Work", "Internal", "intranet-ftps"]);
    assert_eq!(intranet.protocol, Protocol::FtpsExplicit);
    assert_eq!(intranet.port, None, "default port is not stored");
    assert_eq!(intranet.auth, Auth::Password { remember: true });
    assert_eq!(intranet.ftp_mode, FtpMode::Passive);

    let public = find_site(store.tree(), &[root, "public-mirror"]);
    assert_eq!(public.protocol, Protocol::Ftp);
    assert_eq!(public.auth, Auth::Anonymous);

    let implicit = find_site(store.tree(), &[root, "implicit-ftps"]);
    assert_eq!(implicit.protocol, Protocol::FtpsImplicit);
    assert_eq!(implicit.port, None);

    let legacy = find_site(store.tree(), &[root, "legacy-ftp"]);
    assert_eq!(legacy.protocol, Protocol::Ftp);
    assert_eq!(legacy.ftp_mode, FtpMode::Active);
    assert_eq!(legacy.remote_dir.as_deref(), Some("/home/user"));

    let ask = find_site(store.tree(), &[root, "ask-me"]);
    assert_eq!(ask.auth, Auth::Password { remember: false });

    // Passwords: readable ones in the keychain, crypt one not.
    assert_eq!(secrets.len(), 2);
    let secret = |id| secrets.get(&SecretKey::Password(id)).unwrap();
    assert_eq!(
        secret(intranet.id).unwrap().expose_secret(),
        "fixture-password"
    );
    assert_eq!(secret(legacy.id).unwrap().expose_secret(), "legacy-pw");
    assert!(secret(implicit.id).is_none());

    // Durable.
    let reopened = common::open(paths(&dir)).unwrap();
    assert_eq!(reopened.tree(), store.tree());
}

#[test]
fn passwords_are_skipped_unless_requested_and_never_written_to_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemoryStore::new());
    let mut store = common::open_with(paths(&dir), secrets.clone()).unwrap();

    let report = store.import_filezilla(&fixture(), opts(false)).unwrap();

    assert!(secrets.is_empty());
    let mut skipped: Vec<_> = report
        .passwords_skipped
        .iter()
        .map(|p| p.site.as_str())
        .collect();
    skipped.sort();
    assert_eq!(
        skipped,
        ["Work/Internal/intranet-ftps", "implicit-ftps", "legacy-ftp"]
    );
    let file = fs::read_to_string(dir.path().join("cfg/servers.toml")).unwrap();
    for leaked in ["fixture-password", "legacy-pw", "Zml4dHVyZS1wYXNzd29yZA"] {
        assert!(!file.contains(leaked), "{leaked} in servers.toml");
    }
}

#[test]
fn key_site_with_a_saved_passphrase_stores_it_as_a_passphrase() {
    let dir = tempfile::tempdir().unwrap();
    let xml = dir.path().join("sm.xml");
    fs::write(
        &xml,
        "<FileZilla3><Servers><Server><Host>h.example.com</Host><Protocol>1</Protocol><User>u</User>\
         <Pass encoding=\"base64\">cHA=</Pass><Logontype>5</Logontype><Keyfile>/k</Keyfile>\
         <Name>keyed</Name></Server></Servers></FileZilla3>",
    )
    .unwrap();
    let secrets = Arc::new(MemoryStore::new());
    let mut store = common::open_with(paths(&dir), secrets.clone()).unwrap();

    store.import_filezilla(&xml, opts(true)).unwrap();

    let site = store.tree().sites()[0].clone();
    assert_eq!(
        site.auth,
        Auth::KeyFile {
            path: "/k".into(),
            remember_passphrase: true
        }
    );
    let got = secrets
        .get(&SecretKey::Passphrase(site.id))
        .unwrap()
        .unwrap();
    assert_eq!(got.expose_secret(), "pp");
}

#[test]
fn bad_input_writes_nothing_and_sets_no_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemoryStore::new());
    let mut store = common::open_with(paths(&dir), secrets.clone()).unwrap();

    let bad = dir.path().join("bad.xml");
    fs::write(&bad, "<FileZilla3><Servers><Server></Folder></Servers>").unwrap();
    let err = store.import_filezilla(&bad, opts(true)).unwrap_err();
    assert!(matches!(err, ImportError::Parse { .. }), "{err}");

    let err = store
        .import_filezilla(&dir.path().join("missing.xml"), opts(true))
        .unwrap_err();
    assert!(matches!(err, ImportError::Io { .. }), "{err}");

    // servers.toml corrupted behind the store's back: the import must fail, not overwrite.
    fs::create_dir_all(dir.path().join("cfg")).unwrap();
    fs::write(
        dir.path().join("cfg/servers.toml"),
        "version = 1\n[[site]\n",
    )
    .unwrap();
    let err = store.import_filezilla(&fixture(), opts(true)).unwrap_err();
    assert!(
        matches!(err, ImportError::Config(ConfigError::Parse { .. })),
        "{err}"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("cfg/servers.toml")).unwrap(),
        "version = 1\n[[site]\n"
    );
    assert!(secrets.is_empty());
}

#[test]
fn nothing_importable_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let xml = dir.path().join("empty.xml");
    fs::write(&xml, "<FileZilla3><Servers></Servers></FileZilla3>").unwrap();
    let mut store = common::open(paths(&dir)).unwrap();

    let report = store.import_filezilla(&xml, opts(true)).unwrap();

    assert_eq!(report.root_folder, None);
    assert!(report.imported.is_empty());
    assert!(!dir.path().join("cfg/servers.toml").exists());
}

#[test]
fn importing_twice_numbers_the_root_folder_and_keeps_both() {
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemoryStore::new());
    let mut store = common::open_with(paths(&dir), secrets.clone()).unwrap();

    let first = store.import_filezilla(&fixture(), opts(true)).unwrap();
    let second = store.import_filezilla(&fixture(), opts(true)).unwrap();

    assert_eq!(
        first.root_folder.as_deref(),
        Some("Imported from FileZilla (2026-10-05)")
    );
    assert_eq!(
        second.root_folder.as_deref(),
        Some("Imported from FileZilla (2026-10-05) (2)")
    );
    assert_eq!(store.tree().sites().len(), 12);
    assert_eq!(secrets.len(), 4, "each import stores its own secrets");
    let reopened = common::open(paths(&dir)).unwrap();
    assert_eq!(reopened.tree(), store.tree());
}

#[test]
fn duplicate_names_inside_the_file_are_numbered() {
    let dir = tempfile::tempdir().unwrap();
    let xml = dir.path().join("dup.xml");
    let server = |host: &str| {
        format!(
            "<Server><Host>{host}</Host><Protocol>0</Protocol><Logontype>0</Logontype><Name>same</Name></Server>"
        )
    };
    fs::write(
        &xml,
        format!(
            "<FileZilla3><Servers>{}{}<Folder>Same</Folder></Servers></FileZilla3>",
            server("a.example.com"),
            server("b.example.com")
        ),
    )
    .unwrap();
    let mut store = common::open(paths(&dir)).unwrap();

    let report = store.import_filezilla(&xml, opts(false)).unwrap();

    let mut imported = report.imported.clone();
    imported.sort();
    assert_eq!(imported, ["same", "same (2)"]);
    assert_eq!(
        tree_shape(store.tree()),
        [
            "Imported from FileZilla (2026-10-05)/",
            "  Same (3)/",
            "  same",
            "  same (2)"
        ]
    );
}

#[derive(Debug)]
struct RejectingStore;

impl SecretStore for RejectingStore {
    fn get(&self, _: &SecretKey) -> Result<Option<SecretString>, SecretError> {
        Ok(None)
    }
    fn set(&self, _: &SecretKey, _: &SecretString) -> Result<(), SecretError> {
        Err(SecretError::Unavailable("no keychain".into()))
    }
    fn delete(&self, _: &SecretKey) -> Result<(), SecretError> {
        Ok(())
    }
}

#[test]
fn keychain_failure_is_reported_but_the_import_still_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = common::open_with(paths(&dir), Arc::new(RejectingStore)).unwrap();

    let report = store.import_filezilla(&fixture(), opts(true)).unwrap();

    assert_eq!(report.imported.len(), 6);
    let keychain: Vec<_> = report
        .passwords_skipped
        .iter()
        .filter(|p| p.reason.starts_with("keychain error"))
        .map(|p| p.site.as_str())
        .collect();
    assert_eq!(keychain.len(), 2, "{keychain:?}");
    assert!(common::open(paths(&dir)).unwrap().tree().sites().len() == 6);
}

#[test]
fn default_path_points_at_filezillas_site_manager_without_reading_it() {
    let path = default_filezilla_path().unwrap();
    assert!(path.ends_with("sitemanager.xml"), "{}", path.display());
    let dir = path
        .parent()
        .unwrap()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_lowercase();
    assert_eq!(dir, "filezilla");
}
