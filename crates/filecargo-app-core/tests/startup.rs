#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::time::{Duration, Instant};

use filecargo_app_core::{Command, SessionState};
use support::Fixture;

#[test]
fn a_fresh_start_has_an_empty_tree_no_session_and_a_local_pane_at_the_start_dir() {
    let fx = Fixture::new();
    let state = fx.state();
    assert!(state.startup_error.is_none());
    assert!(state.servers.sites().is_empty() && state.servers.folders().is_empty());
    assert_eq!(state.session, SessionState::Disconnected);
    assert!(state.remote.is_none());
    assert!(state.local.path.is_dir(), "{:?}", state.local.path);
    assert!(state.prompt.is_none() && state.notices.is_empty());
}

#[test]
fn the_configured_start_directory_is_used_and_a_bad_one_falls_back_to_home() {
    let dir = tempfile::tempdir().unwrap();
    let wanted = dir.path().to_path_buf();
    let fx = Fixture::with(|config| {
        std::fs::write(
            config.join("settings.toml"),
            format!(
                "version = 1\n[ui]\nlocal_start_dir = {:?}\n",
                wanted.to_str().unwrap()
            ),
        )
        .unwrap();
    });
    assert_eq!(fx.state().local.path, wanted);

    let fx = Fixture::with(|config| {
        std::fs::write(
            config.join("settings.toml"),
            "version = 1\n[ui]\nlocal_start_dir = \"/definitely/not/a/directory\"\n",
        )
        .unwrap();
    });
    assert!(fx.state().local.path.is_dir());
}

#[test]
fn a_corrupt_servers_file_sets_startup_error_and_reset_config_recovers() {
    let fx = Fixture::with(|config| {
        std::fs::write(config.join("servers.toml"), "this is = not [valid").unwrap()
    });
    let state = fx.state();
    let error = state.startup_error.as_ref().expect("a startup error");
    assert!(error.contains("servers.toml"), "{error}");
    assert!(
        state.servers.sites().is_empty(),
        "the UI gets an empty tree"
    );

    fx.app.send(Command::ResetConfig);
    let state = fx.wait_for("the reset", |s| s.startup_error.is_none());
    assert_eq!(state.notices.len(), 1, "{:?}", state.notices);
    let backups: Vec<_> = std::fs::read_dir(fx.config.path())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().contains(".bak-"))
        .collect();
    assert_eq!(backups.len(), 1);
    assert_eq!(
        std::fs::read_to_string(backups[0].path()).unwrap(),
        "this is = not [valid"
    );
}

#[test]
fn a_corrupt_settings_file_is_reset_too() {
    let fx =
        Fixture::with(|config| std::fs::write(config.join("settings.toml"), "= broken").unwrap());
    assert!(fx.state().startup_error.is_some());
    fx.app.send(Command::ResetConfig);
    fx.wait_for("the reset", |s| s.startup_error.is_none());
    assert!(
        fx.config
            .path()
            .read_dir()
            .unwrap()
            .filter_map(Result::ok)
            .any(|e| e
                .file_name()
                .to_string_lossy()
                .starts_with("settings.toml.bak-"))
    );
}

#[test]
fn a_config_from_a_newer_version_is_reported_not_overwritten() {
    let fx = Fixture::with(|config| {
        std::fs::write(config.join("servers.toml"), "version = 9\n").unwrap()
    });
    assert!(
        fx.state()
            .startup_error
            .as_deref()
            .unwrap()
            .contains("newer")
    );
    assert_eq!(
        std::fs::read_to_string(fx.config.path().join("servers.toml")).unwrap(),
        "version = 9\n"
    );
}

#[test]
fn shutdown_is_idempotent_and_returns_within_its_timeout_even_if_tasks_hang() {
    let fx = Fixture::new();
    // an async task that never ends, and a blocking one that outlives the runtime
    fx.app.runtime().spawn(std::future::pending::<()>());
    fx.app
        .runtime()
        .spawn_blocking(|| std::thread::sleep(Duration::from_secs(30)));

    let started = Instant::now();
    fx.app.clone().shutdown(Duration::from_millis(400));
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    let again = Instant::now();
    fx.app.clone().shutdown(Duration::from_millis(400));
    assert!(
        again.elapsed() < Duration::from_millis(200),
        "the second call returns at once"
    );
    fx.app
        .send(Command::DismissNotice(filecargo_app_core::NoticeId(1))); // sending afterwards must not panic
}

#[test]
fn a_burst_of_changes_publishes_at_most_about_thirty_snapshots_a_second() {
    let fx = Fixture::new();
    let mut rx = fx.app.state();
    let counter = fx.app.runtime().spawn(async move {
        let mut seen = 0u32;
        let end = tokio::time::Instant::now() + Duration::from_millis(1000);
        while tokio::time::timeout_at(end, rx.changed()).await.is_ok() {
            seen += 1;
        }
        seen
    });
    // 200 settings changes as fast as possible
    for i in 0..200u8 {
        let mut settings = (*fx.state().settings).clone();
        settings.transfers.max_concurrent = 1 + i % 10;
        fx.app.send(Command::UpdateSettings(settings));
    }
    let seen = fx.app.runtime().block_on(counter).unwrap();
    assert!(seen >= 1, "the UI must hear about the changes");
    assert!(seen <= 35, "{seen} snapshots in one second");
}
