#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::path::PathBuf;

use filecargo_app_core::{Command, Level, PromptKind};
use filecargo_config::{
    ExposeSecret, NodeId, Protocol, SecretKey, SecretStore, SecretString, Site, TreeOp,
};
use support::Fixture;

fn site(name: &str) -> Site {
    let mut site = Site::new(name, Protocol::Sftp, "example.org");
    site.user = "me".into();
    site
}

#[test]
fn add_rename_and_delete_show_up_in_the_next_snapshot() {
    let fx = Fixture::new();
    let work = site("work");
    let id = work.id;
    fx.app.send(Command::Tree(TreeOp::AddSite(work)));
    let state = fx.wait_for("the new site", |s| s.servers.sites().len() == 1);
    assert_eq!(state.servers.sites()[0].name, "work");

    fx.app.send(Command::Tree(TreeOp::Rename {
        node: NodeId::Site(id),
        name: "office".into(),
    }));
    fx.wait_for("the rename", |s| {
        s.servers.site(id).is_some_and(|x| x.name == "office")
    });

    fx.app.send(Command::Tree(TreeOp::Delete {
        node: NodeId::Site(id),
    }));
    fx.wait_for("the delete", |s| s.servers.sites().is_empty());
    assert!(
        fx.state().prompt.is_none(),
        "valid operations show no prompt"
    );
}

#[test]
fn an_invalid_operation_leaves_the_tree_unchanged_and_shows_exactly_one_message() {
    let fx = Fixture::new();
    fx.app.send(Command::Tree(TreeOp::AddSite(site("dup"))));
    fx.wait_for("the first site", |s| s.servers.sites().len() == 1);
    let before = fx.state().servers.clone();

    fx.app.send(Command::Tree(TreeOp::AddSite(site("dup")))); // same name in the same folder
    let state = fx.wait_for("the message", |s| s.prompt.is_some());
    assert_eq!(*state.servers, *before, "the tree must not change");
    match &state.prompt.as_ref().unwrap().kind {
        PromptKind::Message { level, body, .. } => {
            assert_eq!(*level, Level::Error);
            assert!(body.contains("dup"), "{body}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn importing_the_filezilla_fixture_reports_six_imported_and_one_skipped() {
    let fx = Fixture::new();
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../filecargo-config/tests/fixtures/filezilla/sitemanager.xml");
    fx.app.send(Command::ImportFileZilla {
        path: fixture,
        import_passwords: false,
    });
    let state = fx.wait_for("the report", |s| s.prompt.is_some());

    match &state.prompt.as_ref().unwrap().kind {
        PromptKind::Message { title, body, level } => {
            assert_eq!(title, "FileZilla import");
            assert_eq!(*level, Level::Warning, "something was skipped");
            assert!(body.contains("Imported 6 sites"), "{body}");
            assert!(body.contains("Skipped 1"), "{body}");
            assert!(
                body.contains("FileZilla import 20"),
                "the folder name carries the date: {body}"
            );
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(state.servers.sites().len(), 6);
    assert!(
        state
            .servers
            .folders()
            .iter()
            .any(|f| f.name.starts_with("FileZilla import "))
    );
}

#[test]
fn importing_an_unreadable_file_shows_an_error_and_changes_nothing() {
    let fx = Fixture::new();
    fx.app.send(Command::ImportFileZilla {
        path: fx.config.path().join("missing.xml"),
        import_passwords: false,
    });
    let state = fx.wait_for("the error", |s| s.prompt.is_some());
    assert!(matches!(
        &state.prompt.as_ref().unwrap().kind,
        PromptKind::Message {
            level: Level::Error,
            ..
        }
    ));
    assert!(state.servers.sites().is_empty());
}

#[test]
fn set_site_password_goes_to_the_keychain_and_nowhere_else() {
    use filecargo_app_core::{App, StartOptions};
    use filecargo_config::{MemoryStore, Paths};
    use std::sync::Arc;

    let keychain = Arc::new(MemoryStore::new());
    let dir = tempfile::tempdir().unwrap();
    let app = App::start(StartOptions {
        paths: Some(Paths::from_override(Some(dir.path().to_path_buf()))),
        secrets: Some(keychain.clone()),
        connector: None,
    })
    .unwrap();
    let work = site("work");
    let id = work.id;
    app.send(Command::Tree(TreeOp::AddSite(work)));
    app.send(Command::SetSitePassword {
        site: id,
        secret: SecretString::from("s3cret-sentinel".to_owned()),
    });
    let mut rx = app.state();
    app.runtime().block_on(async {
        while rx.borrow().servers.sites().is_empty() {
            rx.changed().await.unwrap();
        }
    });
    // the command was processed after the tree op (same queue)
    app.runtime()
        .block_on(async { tokio::time::sleep(std::time::Duration::from_millis(100)).await });

    let stored = keychain.get(&SecretKey::Password(id)).unwrap().unwrap();
    assert_eq!(stored.expose_secret(), "s3cret-sentinel");
    for file in ["servers.toml", "settings.toml"] {
        if let Ok(text) = std::fs::read_to_string(dir.path().join(file)) {
            assert!(
                !text.contains("s3cret-sentinel"),
                "{file} must not contain the secret"
            );
        }
    }
    assert!(!format!("{:?}", *app.state().borrow()).contains("s3cret-sentinel"));
    app.shutdown(std::time::Duration::from_secs(2));
}
