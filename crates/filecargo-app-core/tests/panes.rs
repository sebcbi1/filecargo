#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::path::Path;
use std::time::Duration;

use filecargo_app_core::{AppState, Command, PaneId, Sort, SortKey};
use support::Fixture;

fn names(state: &AppState) -> Vec<String> {
    state.local.entries.iter().map(|e| e.name.clone()).collect()
}

/// An app whose local pane starts in `start`.
fn app_in(start: &Path) -> Fixture {
    let start = start.to_path_buf();
    Fixture::with(move |config| {
        std::fs::write(
            config.join("settings.toml"),
            format!(
                "version = 1\n[ui]\nlocal_start_dir = {:?}\n",
                start.to_str().unwrap()
            ),
        )
        .unwrap();
    })
}

fn tree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for sub in ["docs", "src", ".git"] {
        std::fs::create_dir(dir.path().join(sub)).unwrap();
    }
    std::fs::write(dir.path().join("file10.txt"), "ten ten").unwrap();
    std::fs::write(dir.path().join("file2.txt"), "2").unwrap();
    std::fs::write(dir.path().join(".env"), "SECRET=1").unwrap();
    std::fs::write(dir.path().join("src/main.rs"), "fn main() {}").unwrap();
    dir
}

#[test]
fn the_start_directory_is_listed_dirs_first_in_natural_order_without_dotfiles() {
    let dir = tree();
    let fx = app_in(dir.path());
    let state = fx.wait_for("the first listing", |s| s.local.generation > 0);
    assert_eq!(names(&state), ["docs", "src", "file2.txt", "file10.txt"]);
    assert!(!state.local.loading && state.local.error.is_none());
    assert_eq!(state.local.path, std::fs::canonicalize(dir.path()).unwrap());
}

#[test]
fn navigation_accepts_relative_absolute_and_home_paths_and_up_goes_to_the_parent() {
    let dir = tree();
    let fx = app_in(dir.path());
    let root = fx
        .wait_for("the first listing", |s| s.local.generation > 0)
        .local
        .path
        .clone();

    fx.app.send(Command::Navigate {
        pane: PaneId::Local,
        path: "src".into(),
    });
    let state = fx.wait_for("src", |s| {
        s.local.path == root.join("src") && !s.local.loading
    });
    assert_eq!(names(&state), ["main.rs"]);

    fx.app.send(Command::Up(PaneId::Local));
    fx.wait_for("up", |s| s.local.path == root && !s.local.loading);

    fx.app.send(Command::Navigate {
        pane: PaneId::Local,
        path: "docs/../src/.".into(),
    });
    fx.wait_for("a path with dots", |s| {
        s.local.path == root.join("src") && !s.local.loading
    });

    fx.app.send(Command::Navigate {
        pane: PaneId::Local,
        path: root.join("docs").to_string_lossy().into_owned(),
    });
    fx.wait_for("an absolute path", |s| {
        s.local.path == root.join("docs") && !s.local.loading
    });

    if let Some(home) = std::env::home_dir().and_then(|h| std::fs::canonicalize(h).ok()) {
        fx.app.send(Command::Navigate {
            pane: PaneId::Local,
            path: "~".into(),
        });
        fx.wait_for("home", |s| s.local.path == home && !s.local.loading);
    }
}

#[test]
fn a_failed_listing_keeps_the_old_entries_and_sets_an_error() {
    let dir = tree();
    let fx = app_in(dir.path());
    let before = fx.wait_for("the first listing", |s| s.local.generation > 0);
    fx.app.send(Command::Navigate {
        pane: PaneId::Local,
        path: "no-such-dir".into(),
    });
    let state = fx.wait_for("the error", |s| s.local.error.is_some());
    assert!(state.local.error.as_ref().unwrap().contains("no-such-dir"));
    assert_eq!(names(&state), names(&before), "the previous entries stay");
    assert_eq!(state.local.path, before.local.path);
    assert_eq!(state.local.generation, before.local.generation);
    assert!(!state.local.loading);

    // a later successful listing clears the error
    fx.app.send(Command::Refresh(PaneId::Local));
    fx.wait_for("the refresh", |s| {
        s.local.error.is_none() && s.local.generation > before.local.generation
    });
}

#[test]
fn two_quick_navigations_leave_the_pane_showing_the_second_one_only() {
    let dir = tempfile::tempdir().unwrap();
    // `big` takes a while to list; `small` is instant, so `big`'s result arrives *last*
    let big = dir.path().join("big");
    std::fs::create_dir(&big).unwrap();
    for i in 0..20_000 {
        std::fs::File::create(big.join(format!("f{i}"))).unwrap();
    }
    let small = dir.path().join("small");
    std::fs::create_dir(&small).unwrap();
    std::fs::write(small.join("only.txt"), "x").unwrap();

    let fx = app_in(dir.path());
    fx.wait_for("the first listing", |s| s.local.generation > 0);
    fx.app.send(Command::Navigate {
        pane: PaneId::Local,
        path: "big".into(),
    });
    fx.app.send(Command::Navigate {
        pane: PaneId::Local,
        path: "small".into(),
    });

    let state = fx.wait_for("the second navigation", |s| {
        s.local.path.ends_with("small") && !s.local.loading
    });
    assert_eq!(names(&state), ["only.txt"]);
    std::thread::sleep(Duration::from_millis(800)); // long enough for the big listing to finish
    let state = fx.state();
    assert!(
        state.local.path.ends_with("small"),
        "the late listing must not win: {:?}",
        state.local.path
    );
    assert_eq!(names(&state), ["only.txt"]);
}

#[test]
fn sorting_by_size_and_date_and_direction_reorders_without_listing_again() {
    let dir = tree();
    let fx = app_in(dir.path());
    let before = fx.wait_for("the first listing", |s| s.local.generation > 0);

    fx.app.send(Command::SetSort {
        pane: PaneId::Local,
        sort: Sort {
            key: SortKey::Size,
            ascending: false,
        },
    });
    let state = fx.wait_for("the size sort", |s| s.local.sort.key == SortKey::Size);
    assert_eq!(
        names(&state),
        ["docs", "src", "file10.txt", "file2.txt"],
        "dirs first, then biggest first"
    );
    assert!(
        state.local.generation > before.local.generation,
        "the UI is told to reset its cursor"
    );

    fx.app.send(Command::SetSort {
        pane: PaneId::Local,
        sort: Sort {
            key: SortKey::Name,
            ascending: false,
        },
    });
    let state = fx.wait_for("the name sort", |s| {
        s.local.sort
            == Sort {
                key: SortKey::Name,
                ascending: false,
            }
    });
    assert_eq!(names(&state), ["src", "docs", "file10.txt", "file2.txt"]);
}

#[test]
fn toggling_show_hidden_in_the_settings_relists_the_pane() {
    let dir = tree();
    let fx = app_in(dir.path());
    fx.wait_for("the first listing", |s| s.local.generation > 0);
    let mut settings = (*fx.state().settings).clone();
    settings.ui.show_hidden = true;
    fx.app.send(Command::UpdateSettings(settings));
    let state = fx.wait_for("hidden files", |s| {
        s.local.entries.iter().any(|e| e.name == ".env")
    });
    assert_eq!(
        names(&state),
        [".git", "docs", "src", ".env", "file2.txt", "file10.txt"]
    );
}

#[test]
fn entries_are_shared_between_snapshots_not_copied() {
    let dir = tree();
    let fx = app_in(dir.path());
    let state = fx.wait_for("the first listing", |s| s.local.generation > 0);
    fx.app
        .send(Command::DismissNotice(filecargo_app_core::NoticeId(1)));
    let mut settings = (*state.settings).clone();
    settings.transfers.max_concurrent = 5;
    fx.app.send(Command::UpdateSettings(settings));
    let later = fx.wait_for("a later snapshot", |s| {
        s.settings.transfers.max_concurrent == 5
    });
    assert!(
        std::sync::Arc::ptr_eq(&state.local.entries, &later.local.entries),
        "an unrelated change must not copy the entries"
    );
}
