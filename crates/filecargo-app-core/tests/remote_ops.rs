#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::time::Duration;

use filecargo_app_core::{AppState, Command, Level, PaneId, PromptAnswer, PromptKind};
use support::{Fixture, TestFactory, server_tree, site_for};

fn names(state: &AppState) -> Vec<String> {
    state
        .remote
        .as_ref()
        .unwrap()
        .entries
        .iter()
        .map(|e| e.name.clone())
        .collect()
}

/// A connected app on the server tree, remote pane in `/projects`.
fn connected() -> (Fixture, tempfile::TempDir) {
    let server = server_tree();
    let factory = TestFactory::new();
    factory.serve("host", server.path().to_path_buf(), None);
    let fx = Fixture::with_factory(factory);
    let mut site = site_for("s", "host");
    site.remote_dir = Some("/projects".into());
    let id = fx.add_site(site);
    fx.app.send(Command::Connect(id));
    fx.wait_for("connected", |s| s.remote.is_some());
    (fx, server)
}

fn set_confirm_delete(fx: &Fixture, on: bool) {
    let mut settings = (*fx.state().settings).clone();
    settings.ui.confirm_delete = on;
    fx.app.send(Command::UpdateSettings(settings));
    fx.wait_for("the setting", |s| s.settings.ui.confirm_delete == on);
}

#[test]
fn mkdir_and_rename_change_the_server_and_refresh_the_pane() {
    let (fx, server) = connected();
    fx.app.send(Command::Mkdir {
        pane: PaneId::Remote,
        name: "newdir".into(),
    });
    let state = fx.wait_for("the new folder", |s| {
        s.remote
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .any(|e| e.name == "newdir")
    });
    assert!(server.path().join("projects/newdir").is_dir());
    assert_eq!(names(&state)[0..2], ["newdir", "sub"], "directories first");

    fx.app.send(Command::Rename {
        pane: PaneId::Remote,
        from: "a.txt".into(),
        to: "renamed.txt".into(),
    });
    let state = fx.wait_for("the rename", |s| {
        s.remote
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .any(|e| e.name == "renamed.txt")
    });
    assert!(!names(&state).contains(&"a.txt".to_owned()));
    assert_eq!(
        std::fs::read(server.path().join("projects/renamed.txt")).unwrap(),
        b"aaa"
    );
}

#[test]
fn delete_asks_first_and_declining_deletes_nothing() {
    let (fx, server) = connected();
    fx.app.send(Command::Delete {
        pane: PaneId::Remote,
        names: vec!["a.txt".into()],
    });
    let state = fx.wait_for("the confirmation", |s| s.prompt.is_some());
    match &state.prompt.as_ref().unwrap().kind {
        PromptKind::ConfirmDelete {
            pane,
            names,
            recursive,
        } => {
            assert_eq!(*pane, PaneId::Remote);
            assert_eq!(names, &["a.txt"]);
            assert!(!recursive, "a plain file is not recursive");
        }
        other => panic!("{other:?}"),
    }
    fx.app.send(Command::Answer {
        id: state.prompt.as_ref().unwrap().id,
        answer: PromptAnswer::Confirm(false),
    });
    fx.wait_for("the prompt to go", |s| s.prompt.is_none());
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        server.path().join("projects/a.txt").exists(),
        "declined: nothing deleted"
    );
    assert!(names(&fx.state()).contains(&"a.txt".to_owned()));
}

#[test]
fn confirming_deletes_files_and_whole_directories_and_refreshes() {
    let (fx, server) = connected();
    fx.app.send(Command::Delete {
        pane: PaneId::Remote,
        names: vec!["sub".into(), "b.txt".into()],
    });
    let state = fx.wait_for("the confirmation", |s| s.prompt.is_some());
    match &state.prompt.as_ref().unwrap().kind {
        PromptKind::ConfirmDelete {
            names, recursive, ..
        } => {
            assert_eq!(names.len(), 2);
            assert!(recursive, "a directory makes it recursive");
        }
        other => panic!("{other:?}"),
    }
    fx.app.send(Command::Answer {
        id: state.prompt.as_ref().unwrap().id,
        answer: PromptAnswer::Confirm(true),
    });
    let state = fx.wait_for("the deletion", |s| names(s) == ["a.txt"]);
    assert!(
        !server.path().join("projects/sub").exists(),
        "the directory went with its contents"
    );
    assert!(!server.path().join("projects/b.txt").exists());
    assert!(state.notices.is_empty(), "{:?}", state.notices);
}

#[test]
fn without_confirm_delete_the_deletion_happens_at_once() {
    let (fx, server) = connected();
    set_confirm_delete(&fx, false);
    fx.app.send(Command::Delete {
        pane: PaneId::Remote,
        names: vec!["a.txt".into()],
    });
    fx.wait_for("the deletion", |s| !names(s).contains(&"a.txt".to_owned()));
    assert!(!server.path().join("projects/a.txt").exists());
    assert!(fx.state().prompt.is_none());
}

#[test]
fn unknown_names_are_ignored_and_an_empty_selection_asks_nothing() {
    let (fx, server) = connected();
    fx.app.send(Command::Delete {
        pane: PaneId::Remote,
        names: vec!["not-in-this-pane".into()],
    });
    fx.app.send(Command::Delete {
        pane: PaneId::Remote,
        names: vec![],
    });
    std::thread::sleep(Duration::from_millis(200));
    assert!(fx.state().prompt.is_none());
    assert!(server.path().join("projects/a.txt").exists());
}

#[cfg(unix)]
#[test]
fn chmod_changes_the_permissions_of_the_selection() {
    use std::os::unix::fs::PermissionsExt;
    let (fx, server) = connected();
    fx.app.send(Command::Chmod {
        pane: PaneId::Remote,
        names: vec!["a.txt".into(), "b.txt".into()],
        mode: 0o600,
    });
    fx.wait_for("the chmod to show", |s| {
        s.remote
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .filter(|e| e.name.ends_with(".txt"))
            .all(|e| e.permissions.map(|p| p & 0o777) == Some(0o600))
    });
    let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&server.path().join("projects/a.txt")), 0o600);
    assert_eq!(mode(&server.path().join("projects/b.txt")), 0o600);
}

#[test]
fn failures_become_a_notice_without_changing_the_pane() {
    let (fx, _server) = connected();
    let before = fx.state();
    fx.app.send(Command::Mkdir {
        pane: PaneId::Remote,
        name: "sub".into(),
    }); // already exists
    let state = fx.wait_for("a notice", |s| !s.notices.is_empty());
    assert_eq!(state.notices[0].level, Level::Error);
    assert!(
        state.notices[0].text.contains("create the folder \"sub\""),
        "{}",
        state.notices[0].text
    );
    assert_eq!(names(&state), names(&before));

    fx.app.send(Command::Mkdir {
        pane: PaneId::Remote,
        name: "a/b".into(),
    });
    let state = fx.wait_for("a notice for the bad name", |s| s.notices.len() >= 2);
    assert!(state.notices[1].text.contains("not a valid file name"));
    fx.app.send(Command::Rename {
        pane: PaneId::Remote,
        from: "a.txt".into(),
        to: "..".into(),
    });
    let state = fx.wait_for("another notice", |s| s.notices.len() >= 3);
    assert!(state.notices[2].text.contains("not a valid file name"));
}
