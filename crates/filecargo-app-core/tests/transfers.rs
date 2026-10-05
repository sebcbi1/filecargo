#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use filecargo_app_core::{AppState, Command, PromptAnswer, PromptKind};
use filecargo_config::ConflictRule;
use filecargo_transfer::{ConflictDecision, ItemState, Outcome};
use support::{Fixture, TestFactory, server_tree, site_for};

fn remote_names(state: &AppState) -> Vec<String> {
    state
        .remote
        .as_ref()
        .unwrap()
        .entries
        .iter()
        .map(|e| e.name.clone())
        .collect()
}

fn local_names(state: &AppState) -> Vec<String> {
    state.local.entries.iter().map(|e| e.name.clone()).collect()
}

/// Connected app: remote pane in `/projects` of the server tree, local pane in `local`.
fn connected(local: &std::path::Path) -> (Fixture, tempfile::TempDir) {
    let server = server_tree();
    let factory = TestFactory::new();
    factory.serve("host", server.path().to_path_buf(), None);
    let fx = Fixture::with_factory(factory);
    let mut site = site_for("s", "host");
    site.remote_dir = Some("/projects".into());
    site.local_dir = Some(local.to_path_buf());
    let id = fx.add_site(site);
    fx.app.send(Command::Connect(id));
    fx.wait_for("connected", |s| s.remote.is_some());
    fx.wait_for("the local pane", |s| {
        s.local.generation > 0 && s.local.path.ends_with(local.file_name().unwrap())
    });
    (fx, server)
}

fn idle(state: &AppState) -> bool {
    let q = &state.queue;
    q.pending.is_empty() && !q.completed.is_empty()
}

#[test]
fn a_mixed_selection_uploads_and_the_remote_pane_refreshes() {
    let local = tempfile::tempdir().unwrap();
    std::fs::write(local.path().join("report.txt"), "report").unwrap();
    std::fs::create_dir_all(local.path().join("album/inner")).unwrap();
    std::fs::write(local.path().join("album/one.jpg"), "1").unwrap();
    std::fs::write(local.path().join("album/inner/two.jpg"), "22").unwrap();
    let (fx, server) = connected(local.path());

    fx.app.send(Command::Upload {
        names: vec!["report.txt".into(), "album".into(), "ghost".into()],
    });
    let state = fx.wait_for("the upload to finish", |s| {
        idle(s) && s.queue.completed.len() >= 5
    });
    assert!(state.queue.failed.is_empty(), "{:?}", state.queue.failed);
    assert_eq!(
        std::fs::read(server.path().join("projects/report.txt")).unwrap(),
        b"report"
    );
    assert_eq!(
        std::fs::read(server.path().join("projects/album/inner/two.jpg")).unwrap(),
        b"22"
    );
    let state = fx.wait_for("the pane refresh", |s| {
        remote_names(s).contains(&"album".to_owned())
            && remote_names(s).contains(&"report.txt".to_owned())
    });
    assert!(
        state
            .remote
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .any(|e| e.name == "album" && e.is_dir())
    );
}

#[test]
fn a_mixed_selection_downloads_and_the_local_pane_refreshes() {
    let local = tempfile::tempdir().unwrap();
    let (fx, _server) = connected(local.path());
    fx.app.send(Command::Download {
        names: vec!["a.txt".into(), "sub".into()],
    });
    fx.wait_for("the download to finish", |s| {
        idle(s) && s.queue.completed.len() >= 3
    });
    assert_eq!(std::fs::read(local.path().join("a.txt")).unwrap(), b"aaa");
    assert_eq!(std::fs::read(local.path().join("sub/c.txt")).unwrap(), b"c");
    let state = fx.wait_for("the pane refresh", |s| {
        local_names(s).contains(&"sub".to_owned())
    });
    assert_eq!(local_names(&state), ["sub", "a.txt"]);
}

#[test]
fn transfers_without_a_session_only_raise_a_notice() {
    let fx = Fixture::new();
    fx.app.send(Command::Upload {
        names: vec!["x".into()],
    });
    let state = fx.wait_for("a notice", |s| !s.notices.is_empty());
    assert!(state.notices[0].text.contains("Connect to a server"));
}

#[test]
fn a_conflict_becomes_a_prompt_and_the_answer_reaches_the_queue() {
    let local = tempfile::tempdir().unwrap();
    std::fs::write(local.path().join("a.txt"), "NEW CONTENT").unwrap();
    let (fx, server) = connected(local.path());

    fx.app.send(Command::Upload {
        names: vec!["a.txt".into()],
    }); // projects/a.txt already exists
    let state = fx.wait_for("the conflict prompt", |s| s.prompt.is_some());
    let prompt = state.prompt.clone().unwrap();
    match &prompt.kind {
        PromptKind::Conflict { conflict, .. } => {
            assert_eq!(conflict.source.size, 11);
            assert_eq!(conflict.target.size, 3);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        std::fs::read(server.path().join("projects/a.txt")).unwrap(),
        b"aaa",
        "nothing is written before the answer"
    );

    fx.app.send(Command::Answer {
        id: prompt.id,
        answer: PromptAnswer::Conflict(ConflictDecision {
            rule: ConflictRule::Overwrite,
            apply_to_all: false,
        }),
    });
    let state = fx.wait_for("the transfer to finish", idle);
    assert!(matches!(
        state.queue.completed[0].item.state,
        ItemState::Completed {
            outcome: Outcome::Transferred,
            ..
        }
    ));
    assert_eq!(
        std::fs::read(server.path().join("projects/a.txt")).unwrap(),
        b"NEW CONTENT"
    );
}

#[test]
fn dismissing_a_conflict_prompt_skips_the_file() {
    let local = tempfile::tempdir().unwrap();
    std::fs::write(local.path().join("a.txt"), "NEW").unwrap();
    let (fx, server) = connected(local.path());
    fx.app.send(Command::Upload {
        names: vec!["a.txt".into()],
    });
    let prompt = fx
        .wait_for("the prompt", |s| s.prompt.is_some())
        .prompt
        .clone()
        .unwrap();
    fx.app.send(Command::Answer {
        id: prompt.id,
        answer: PromptAnswer::Dismiss,
    });
    let state = fx.wait_for("the item to finish", idle);
    assert!(matches!(
        state.queue.completed[0].item.state,
        ItemState::Completed {
            outcome: Outcome::Skipped,
            ..
        }
    ));
    assert_eq!(
        std::fs::read(server.path().join("projects/a.txt")).unwrap(),
        b"aaa"
    );
}

#[test]
fn queue_commands_pause_retry_remove_and_clear() {
    let local = tempfile::tempdir().unwrap();
    std::fs::write(local.path().join("x.txt"), "x").unwrap();
    let (fx, _server) = connected(local.path());

    fx.app.send(Command::QueueSetProcessing(false));
    fx.wait_for("paused", |s| !s.queue.processing);
    fx.app.send(Command::Upload {
        names: vec!["x.txt".into()],
    });
    let state = fx.wait_for("the queued item", |s| s.queue.pending.len() == 1);
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        fx.state().queue.completed.is_empty(),
        "paused: nothing may start"
    );
    let id = state.queue.pending[0].item.id;

    fx.app.send(Command::QueueRemove(id));
    fx.wait_for("removed", |s| s.queue.pending.is_empty());

    fx.app.send(Command::QueueSetProcessing(true));
    fx.app.send(Command::Upload {
        names: vec!["x.txt".into()],
    });
    fx.wait_for("done", |s| s.queue.completed.len() == 1);
    fx.app.send(Command::QueueClearCompleted);
    fx.wait_for("cleared", |s| s.queue.completed.is_empty());
}

#[test]
fn a_failed_transfer_can_be_retried_from_the_failed_list() {
    let local = tempfile::tempdir().unwrap();
    std::fs::write(local.path().join("late.txt"), "late").unwrap();
    let (fx, server) = connected(local.path());
    // the source vanishes after the pane listed it
    std::fs::remove_file(local.path().join("late.txt")).unwrap();
    fx.app.send(Command::Upload {
        names: vec!["late.txt".into()],
    });
    fx.wait_for("the failure", |s| s.queue.failed.len() == 1);
    std::fs::write(local.path().join("late.txt"), "late").unwrap();
    fx.app.send(Command::QueueRetryFailed);
    fx.wait_for("the retry", |s| {
        s.queue.completed.len() == 1 && s.queue.failed.is_empty()
    });
    assert_eq!(
        std::fs::read(server.path().join("projects/late.txt")).unwrap(),
        b"late"
    );
}

#[test]
fn snapshots_stay_under_thirty_a_second_and_share_the_entries_during_a_thousand_file_transfer() {
    let local = tempfile::tempdir().unwrap();
    let batch = local.path().join("batch");
    std::fs::create_dir(&batch).unwrap();
    for i in 0..1000 {
        std::fs::write(batch.join(format!("f{i:04}")), format!("file {i}")).unwrap();
    }
    let (fx, server) = connected(local.path());

    let mut rx = fx.app.state();
    let counter = fx.app.runtime().spawn(async move {
        let mut seen = 0u32;
        let mut last_pane: Option<(u64, Arc<[filecargo_app_core::Entry]>)> = None;
        let mut shared = true;
        let mut finished_at = None;
        let start = tokio::time::Instant::now();
        loop {
            if tokio::time::timeout(Duration::from_millis(1500), rx.changed())
                .await
                .is_err()
            {
                break;
            }
            seen += 1;
            let snapshot = rx.borrow_and_update().clone();
            // while the pane has not been re-listed (same generation), snapshots must share the
            // very same entries instead of copying them
            if let Some((generation, entries)) = &last_pane
                && *generation == snapshot.local.generation
            {
                shared &= Arc::ptr_eq(entries, &snapshot.local.entries);
            }
            last_pane = Some((snapshot.local.generation, snapshot.local.entries.clone()));
            if snapshot.queue.completed.len() >= 1000 && finished_at.is_none() {
                finished_at = Some(start.elapsed());
            }
        }
        (seen, shared, finished_at, start.elapsed())
    });

    fx.app.send(Command::Upload {
        names: vec!["batch".into()],
    });
    let (seen, shared, finished_at, total) =
        fx.app.runtime().block_on(async { counter.await.unwrap() });
    finished_at.expect("the transfer should have completed");
    let seconds = (total.as_secs_f64()).max(1.0);
    assert!(
        f64::from(seen) <= seconds * 30.0 + 5.0,
        "{seen} snapshots in {seconds:.1}s"
    );
    assert!(
        shared,
        "snapshots must share the pane's entries, not copy them"
    );
    assert!(server.path().join("projects/batch/f0999").exists());
}
