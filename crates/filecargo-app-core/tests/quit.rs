#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::time::{Duration, Instant};

use filecargo_app_core::{Command, PromptAnswer, PromptKind, SessionState};
use filecargo_transfer::ItemState;
use support::{Fixture, TestFactory, server_tree, site_for};

/// Waits for the actor to end: the state channel closes.
fn wait_closed(fx: &Fixture, within: Duration) -> bool {
    let mut rx = fx.app.state();
    fx.app.runtime().block_on(async {
        tokio::time::timeout(within, async { while rx.changed().await.is_ok() {} })
            .await
            .is_ok()
    })
}

#[test]
fn quitting_with_nothing_running_ends_the_app_at_once() {
    let fx = Fixture::new();
    fx.app.send(Command::Quit);
    assert!(
        wait_closed(&fx, Duration::from_secs(3)),
        "the state channel must close"
    );
    let started = Instant::now();
    fx.app.clone().shutdown(Duration::from_secs(2));
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn quitting_during_a_transfer_asks_first_and_confirming_persists_the_queue() {
    let local = tempfile::tempdir().unwrap();
    std::fs::write(local.path().join("big.bin"), vec![7u8; 100_000]).unwrap();
    let server = server_tree();
    let factory = TestFactory::new();
    factory.serve("host", server.path().to_path_buf(), None);
    *factory.transfer_delay.lock().unwrap() = Duration::from_secs(30); // a transfer that is still running
    let fx = Fixture::with_factory(factory);
    let mut site = site_for("s", "host");
    site.remote_dir = Some("/projects".into());
    site.local_dir = Some(local.path().to_path_buf());
    let id = fx.add_site(site);
    fx.app.send(Command::Connect(id));
    fx.wait_for("connected", |s| s.remote.is_some());
    fx.wait_for("the local pane", |s| {
        s.local.entries.iter().any(|e| e.name == "big.bin")
    });

    fx.app.send(Command::Upload {
        names: vec!["big.bin".into()],
    });
    fx.wait_for("the transfer to be running", |s| {
        s.queue
            .pending
            .iter()
            .any(|v| matches!(v.item.state, ItemState::Active { .. }))
    });

    // 1. quit asks; declining keeps everything running
    fx.app.send(Command::Quit);
    let state = fx.wait_for("the confirmation", |s| s.prompt.is_some());
    let prompt = state.prompt.clone().unwrap();
    assert!(
        matches!(
            prompt.kind,
            PromptKind::ConfirmQuit {
                active_transfers: 1
            }
        ),
        "{:?}",
        prompt.kind
    );
    fx.app.send(Command::Answer {
        id: prompt.id,
        answer: PromptAnswer::Confirm(false),
    });
    fx.wait_for("the prompt to go", |s| s.prompt.is_none());
    assert!(
        !wait_closed(&fx, Duration::from_millis(400)),
        "declining must not quit"
    );

    // 2. confirming persists the queue and ends the app
    fx.app.send(Command::Quit);
    let prompt = fx
        .wait_for("the confirmation again", |s| s.prompt.is_some())
        .prompt
        .clone()
        .unwrap();
    fx.app.send(Command::Answer {
        id: prompt.id,
        answer: PromptAnswer::Confirm(true),
    });
    assert!(
        wait_closed(&fx, Duration::from_secs(5)),
        "the app must end after the confirmation"
    );

    let saved = std::fs::read_to_string(fx.config.path().join("queue.json")).unwrap();
    assert!(
        saved.contains("big.bin"),
        "the interrupted transfer is saved: {saved}"
    );
    let last = fx.state();
    assert_eq!(last.session, SessionState::Disconnected);
    assert!(last.remote.is_none());

    let started = Instant::now();
    fx.app.clone().shutdown(Duration::from_secs(2));
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the handle shuts down within its timeout"
    );
}

#[test]
fn a_saved_queue_comes_back_when_the_app_starts_again() {
    let local = tempfile::tempdir().unwrap();
    std::fs::write(local.path().join("again.bin"), "x").unwrap();
    let server = server_tree();
    let factory = TestFactory::new();
    factory.serve("host", server.path().to_path_buf(), None);
    *factory.transfer_delay.lock().unwrap() = Duration::from_secs(30);
    let config = tempfile::tempdir().unwrap();
    let start = |factory: std::sync::Arc<TestFactory>| {
        filecargo_app_core::App::start(filecargo_app_core::StartOptions {
            paths: Some(filecargo_config::Paths::from_override(Some(
                config.path().to_path_buf(),
            ))),
            secrets: Some(std::sync::Arc::new(filecargo_config::MemoryStore::new())),
            connector: Some(factory),
        })
        .unwrap()
    };
    let fx = Fixture {
        app: start(factory.clone()),
        config: tempfile::tempdir().unwrap(),
    };
    let mut site = site_for("s", "host");
    site.remote_dir = Some("/projects".into());
    site.local_dir = Some(local.path().to_path_buf());
    let id = fx.add_site(site);
    fx.app.send(Command::Connect(id));
    fx.wait_for("connected", |s| s.remote.is_some());
    fx.wait_for("the pane", |s| {
        s.local.entries.iter().any(|e| e.name == "again.bin")
    });
    fx.app.send(Command::Upload {
        names: vec!["again.bin".into()],
    });
    fx.wait_for("running", |s| {
        s.queue
            .pending
            .iter()
            .any(|v| matches!(v.item.state, ItemState::Active { .. }))
    });
    fx.app.send(Command::Quit);
    let prompt = fx
        .wait_for("the confirmation", |s| s.prompt.is_some())
        .prompt
        .clone()
        .unwrap();
    fx.app.send(Command::Answer {
        id: prompt.id,
        answer: PromptAnswer::Confirm(true),
    });
    assert!(wait_closed(&fx, Duration::from_secs(5)));
    fx.app.clone().shutdown(Duration::from_secs(2));

    *factory.transfer_delay.lock().unwrap() = Duration::ZERO;
    let again = Fixture {
        app: start(factory),
        config: tempfile::tempdir().unwrap(),
    };
    let state = again.wait_for("the restored queue", |s| {
        !s.queue.pending.is_empty() || !s.queue.completed.is_empty()
    });
    assert_eq!(
        state.queue.pending.len() + state.queue.completed.len(),
        1,
        "{:?}",
        state.queue
    );
}
