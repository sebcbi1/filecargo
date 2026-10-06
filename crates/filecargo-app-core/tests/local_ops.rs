#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::path::Path;
use std::time::Duration;

use filecargo_app_core::{AppState, Command, Level, PaneId, PromptAnswer, PromptKind};
use support::Fixture;

fn names(state: &AppState) -> Vec<String> {
    state.local.entries.iter().map(|e| e.name.clone()).collect()
}

/// An app whose local pane starts in `start`, listed.
fn app_in(start: &Path, confirm_delete: bool) -> Fixture {
    let start = start.to_path_buf();
    let fx = Fixture::with(move |config| {
        std::fs::write(
            config.join("settings.toml"),
            format!(
                "version = 1\n[ui]\nlocal_start_dir = {:?}\nconfirm_delete = {confirm_delete}\n",
                start.to_str().unwrap()
            ),
        )
        .unwrap();
    });
    fx.wait_for("the first listing", |s| s.local.generation > 0);
    fx
}

fn tree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("sub/deep")).unwrap();
    std::fs::write(dir.path().join("sub/deep/x.txt"), "x").unwrap();
    std::fs::write(dir.path().join("a.txt"), "aaa").unwrap();
    std::fs::write(dir.path().join("b.txt"), "bbb").unwrap();
    dir
}

#[test]
fn mkdir_and_rename_change_the_disk_and_refresh_the_local_pane() {
    let dir = tree();
    let fx = app_in(dir.path(), true);
    fx.app.send(Command::Mkdir {
        pane: PaneId::Local,
        name: "newdir".into(),
    });
    let state = fx.wait_for("the new folder", |s| {
        names(s).contains(&"newdir".to_owned())
    });
    assert!(dir.path().join("newdir").is_dir());
    assert!(state.notices.is_empty(), "{:?}", state.notices);

    fx.app.send(Command::Rename {
        pane: PaneId::Local,
        from: "a.txt".into(),
        to: "renamed.txt".into(),
    });
    let state = fx.wait_for("the rename", |s| {
        names(s).contains(&"renamed.txt".to_owned())
    });
    assert!(!names(&state).contains(&"a.txt".to_owned()));
    assert_eq!(
        std::fs::read(dir.path().join("renamed.txt")).unwrap(),
        b"aaa"
    );
}

#[test]
fn delete_asks_first_and_declining_deletes_nothing() {
    let dir = tree();
    let fx = app_in(dir.path(), true);
    fx.app.send(Command::Delete {
        pane: PaneId::Local,
        names: vec!["a.txt".into(), "sub".into()],
    });
    let state = fx.wait_for("the confirmation", |s| s.prompt.is_some());
    match &state.prompt.as_ref().unwrap().kind {
        PromptKind::ConfirmDelete {
            pane,
            names,
            recursive,
        } => {
            assert_eq!(*pane, PaneId::Local);
            assert_eq!(names, &["a.txt", "sub"]);
            assert!(recursive, "a directory makes it recursive");
        }
        other => panic!("{other:?}"),
    }
    fx.app.send(Command::Answer {
        id: state.prompt.as_ref().unwrap().id,
        answer: PromptAnswer::Confirm(false),
    });
    fx.wait_for("the prompt to go", |s| s.prompt.is_none());
    std::thread::sleep(Duration::from_millis(200));
    assert!(dir.path().join("a.txt").exists());
    assert!(dir.path().join("sub/deep/x.txt").exists());
}

#[test]
fn confirming_deletes_files_and_whole_directories_and_refreshes() {
    let dir = tree();
    let fx = app_in(dir.path(), true);
    fx.app.send(Command::Delete {
        pane: PaneId::Local,
        names: vec!["sub".into(), "b.txt".into()],
    });
    let state = fx.wait_for("the confirmation", |s| s.prompt.is_some());
    fx.app.send(Command::Answer {
        id: state.prompt.as_ref().unwrap().id,
        answer: PromptAnswer::Confirm(true),
    });
    let state = fx.wait_for("the deletion", |s| names(s) == ["a.txt"]);
    assert!(!dir.path().join("sub").exists(), "recursive");
    assert!(!dir.path().join("b.txt").exists());
    assert!(state.notices.is_empty(), "{:?}", state.notices);
}

#[test]
fn without_confirm_delete_the_deletion_happens_at_once() {
    let dir = tree();
    let fx = app_in(dir.path(), false);
    fx.app.send(Command::Delete {
        pane: PaneId::Local,
        names: vec!["a.txt".into()],
    });
    fx.wait_for("the deletion", |s| !names(s).contains(&"a.txt".to_owned()));
    assert!(!dir.path().join("a.txt").exists());
    assert!(fx.state().prompt.is_none());
}

#[test]
fn invalid_names_and_unknown_entries_are_rejected() {
    let dir = tree();
    let fx = app_in(dir.path(), false);
    fx.app.send(Command::Mkdir {
        pane: PaneId::Local,
        name: "a/b".into(),
    });
    let state = fx.wait_for("a notice", |s| !s.notices.is_empty());
    assert_eq!(state.notices[0].level, Level::Error);
    assert!(state.notices[0].text.contains("not a valid file name"));
    assert!(!dir.path().join("a").exists());

    fx.app.send(Command::Rename {
        pane: PaneId::Local,
        from: "a.txt".into(),
        to: "../escape".into(),
    });
    fx.app.send(Command::Delete {
        pane: PaneId::Local,
        names: vec!["not-in-this-pane".into()],
    });
    std::thread::sleep(Duration::from_millis(200));
    assert!(dir.path().join("a.txt").exists());
    assert!(fx.state().prompt.is_none());
}

#[test]
fn failures_become_a_notice() {
    let dir = tree();
    let fx = app_in(dir.path(), false);
    fx.app.send(Command::Mkdir {
        pane: PaneId::Local,
        name: "sub".into(), // exists
    });
    let state = fx.wait_for("a notice", |s| !s.notices.is_empty());
    assert!(state.notices[0].text.contains("create the folder \"sub\""));
}

#[cfg(unix)]
#[test]
fn chmod_sets_the_mode_of_local_entries() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tree();
    let fx = app_in(dir.path(), true);
    fx.app.send(Command::Chmod {
        pane: PaneId::Local,
        names: vec!["a.txt".into(), "b.txt".into()],
        mode: 0o600,
    });
    fx.wait_for("the chmod to show", |s| {
        s.local
            .entries
            .iter()
            .filter(|e| e.name.ends_with(".txt"))
            .all(|e| e.permissions.map(|p| p & 0o777) == Some(0o600))
    });
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&dir.path().join("a.txt")), 0o600);
    assert_eq!(mode(&dir.path().join("b.txt")), 0o600);
}

#[cfg(windows)]
#[test]
fn chmod_on_the_local_pane_is_refused_on_windows() {
    let dir = tree();
    let fx = app_in(dir.path(), true);
    fx.app.send(Command::Chmod {
        pane: PaneId::Local,
        names: vec!["a.txt".into()],
        mode: 0o600,
    });
    let state = fx.wait_for("a notice", |s| !s.notices.is_empty());
    assert_eq!(state.notices[0].level, Level::Warning);
}
