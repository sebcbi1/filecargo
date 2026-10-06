#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Add to queue, start, per-site pause, clear and clear-failed: all act on the connected site.

mod support;

use std::time::Duration;

use filecargo_app_core::{
    AppState, Command, Level, PaneId, PromptAnswer, PromptKind, SessionState,
};
use filecargo_config::SiteId;
use filecargo_transfer::ItemState;
use support::{Fixture, TestFactory, server_tree, site_for};

struct Two {
    fx: Fixture,
    a: SiteId,
    b: SiteId,
    server_a: tempfile::TempDir,
    server_b: tempfile::TempDir,
    local: tempfile::TempDir,
}

/// Two sites over their own servers; the local folder holds `x.txt`, `y.txt` and `z.txt`.
fn two_sites() -> Two {
    let (server_a, server_b) = (server_tree(), server_tree());
    let local = tempfile::tempdir().unwrap();
    for name in ["x.txt", "y.txt", "z.txt"] {
        std::fs::write(local.path().join(name), name).unwrap();
    }
    let factory = TestFactory::new();
    factory.serve("host-a", server_a.path().to_path_buf(), None);
    factory.serve("host-b", server_b.path().to_path_buf(), None);
    let fx = Fixture::with_factory(factory);
    let mut ids = Vec::new();
    for (name, host) in [("a", "host-a"), ("b", "host-b")] {
        let mut site = site_for(name, host);
        site.remote_dir = Some("/projects".into());
        site.local_dir = Some(local.path().to_path_buf());
        ids.push(fx.add_site(site));
    }
    Two {
        fx,
        a: ids[0],
        b: ids[1],
        server_a,
        server_b,
        local,
    }
}

fn connect(fx: &Fixture, site: SiteId) {
    fx.app.send(Command::Connect(site));
    fx.wait_for("the session", move |s| {
        matches!(&s.session, SessionState::Connected { site: c, .. } if *c == site)
            && s.remote.is_some()
            && !s.local.entries.is_empty()
    });
}

fn held(state: &AppState) -> usize {
    state
        .queue
        .pending
        .iter()
        .filter(|v| matches!(v.item.state, ItemState::Held))
        .count()
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|n| (*n).to_owned()).collect()
}

#[test]
fn enqueue_leaves_held_items_and_nothing_transfers_until_started() {
    let t = two_sites();
    connect(&t.fx, t.a);
    t.fx.app.send(Command::Enqueue {
        from: PaneId::Local,
        names: names(&["x.txt", "y.txt", "z.txt"]),
    });
    let state = t.fx.wait_for("three held items", |s| held(s) == 3);
    assert_eq!(state.site_queue.pending.len(), 3);
    std::thread::sleep(Duration::from_millis(400));
    let state = t.fx.state();
    assert_eq!(held(&state), 3, "still held");
    assert!(state.queue.completed.is_empty());
    assert!(!t.server_a.path().join("projects/x.txt").exists());

    t.fx.app.send(Command::QueueStartHeld);
    t.fx.wait_for("the transfers", |s| s.queue.completed.len() == 3);
    for name in ["x.txt", "y.txt", "z.txt"] {
        assert!(
            t.server_a.path().join("projects").join(name).exists(),
            "{name}"
        );
    }
}

#[test]
fn enqueueing_from_the_remote_pane_downloads_later() {
    let t = two_sites();
    connect(&t.fx, t.a);
    t.fx.app.send(Command::Enqueue {
        from: PaneId::Remote,
        names: names(&["a.txt"]),
    });
    t.fx.wait_for("a held download", |s| held(s) == 1);
    t.fx.app.send(Command::QueueStartHeld);
    t.fx.wait_for("the download", |s| s.queue.completed.len() == 1);
    assert_eq!(std::fs::read(t.local.path().join("a.txt")).unwrap(), b"aaa");
}

#[test]
fn pausing_a_site_leaves_other_sites_running() {
    let t = two_sites();
    connect(&t.fx, t.a);
    assert!(!t.fx.state().site_paused());
    t.fx.app.send(Command::QueueSetSitePaused(true));
    let state = t.fx.wait_for("the pause", |s| s.site_paused());
    assert!(state.queue.processing, "the global switch is not used");

    // B is not paused: its transfer runs
    connect(&t.fx, t.b);
    assert!(!t.fx.state().site_paused(), "the flag is per site");
    t.fx.app.send(Command::Upload {
        names: names(&["x.txt"]),
    });
    t.fx.wait_for("B's transfer", |s| s.queue.completed.len() == 1);
    assert!(t.server_b.path().join("projects/x.txt").exists());

    // A is: its transfer waits
    connect(&t.fx, t.a);
    assert!(t.fx.state().site_paused(), "paused again once A is shown");
    t.fx.app.send(Command::Upload {
        names: names(&["y.txt"]),
    });
    t.fx.wait_for("A's item", |s| s.site_queue.pending.len() == 1);
    std::thread::sleep(Duration::from_millis(400));
    assert!(!t.server_a.path().join("projects/y.txt").exists());
    assert_eq!(t.fx.state().site_queue.pending.len(), 1);

    t.fx.app.send(Command::QueueSetSitePaused(false));
    t.fx.wait_for("A's transfer after resuming", |s| {
        s.queue.completed.len() == 2 && !s.site_paused()
    });
    assert!(t.server_a.path().join("projects/y.txt").exists());
}

#[test]
fn clear_asks_first_and_only_a_yes_empties_the_connected_sites_queue() {
    let t = two_sites();
    connect(&t.fx, t.a);
    t.fx.app.send(Command::Enqueue {
        from: PaneId::Local,
        names: names(&["x.txt", "y.txt"]),
    });
    t.fx.wait_for("A's held items", |s| held(s) == 2);
    connect(&t.fx, t.b);
    t.fx.app.send(Command::Enqueue {
        from: PaneId::Local,
        names: names(&["z.txt"]),
    });
    t.fx.wait_for("B's held item", |s| held(s) == 3);
    connect(&t.fx, t.a);

    t.fx.app.send(Command::QueueClear);
    let state = t.fx.wait_for("the question", |s| s.prompt.is_some());
    match &state.prompt.as_ref().unwrap().kind {
        PromptKind::ConfirmClearQueue { items, active } => assert_eq!((*items, *active), (2, 0)),
        other => panic!("{other:?}"),
    }
    t.fx.app.send(Command::Answer {
        id: state.prompt.as_ref().unwrap().id,
        answer: PromptAnswer::Confirm(false),
    });
    t.fx.wait_for("the prompt to go", |s| s.prompt.is_none());
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(held(&t.fx.state()), 3, "declined: nothing is cleared");

    t.fx.app.send(Command::QueueClear);
    let state = t.fx.wait_for("the question again", |s| s.prompt.is_some());
    t.fx.app.send(Command::Answer {
        id: state.prompt.as_ref().unwrap().id,
        answer: PromptAnswer::Confirm(true),
    });
    let state =
        t.fx.wait_for("the clear", |s| s.site_queue.pending.is_empty());
    assert_eq!(state.queue.pending.len(), 1, "B's item is left");
    assert_eq!(state.queue.pending[0].item.site, t.b);
}

#[test]
fn clear_failed_removes_the_connected_sites_failures_only() {
    let t = two_sites();
    connect(&t.fx, t.a);
    t.fx.app.send(Command::Upload {
        names: names(&["x.txt"]),
    });
    t.fx.wait_for("A's transfer", |s| s.queue.completed.len() == 1);
    // the source disappears after queueing: both uploads fail
    t.fx.app.send(Command::Enqueue {
        from: PaneId::Local,
        names: names(&["y.txt"]),
    });
    t.fx.wait_for("held", |s| held(s) == 1);
    std::fs::remove_file(t.local.path().join("y.txt")).unwrap();
    t.fx.app.send(Command::QueueStartHeld);
    t.fx.wait_for("the failure", |s| s.site_queue.failed.len() == 1);
    connect(&t.fx, t.b);
    t.fx.app.send(Command::Enqueue {
        from: PaneId::Local,
        names: names(&["z.txt"]),
    });
    t.fx.wait_for("held", |s| held(s) == 1);
    std::fs::remove_file(t.local.path().join("z.txt")).unwrap();
    t.fx.app.send(Command::QueueStartHeld);
    t.fx.wait_for("B's failure", |s| s.queue.failed.len() == 2);

    t.fx.app.send(Command::QueueClearFailed);
    let state = t.fx.wait_for("the clear", |s| s.queue.failed.len() == 1);
    assert!(state.site_queue.failed.is_empty(), "B's are gone");
    assert_eq!(state.queue.failed[0].item.site, t.a, "A's are not");
}

#[test]
fn clear_completed_is_scoped_to_the_connected_site() {
    let t = two_sites();
    for site in [t.a, t.b] {
        connect(&t.fx, site);
        let before = t.fx.state().queue.completed.len();
        t.fx.app.send(Command::Upload {
            names: names(&["x.txt"]),
        });
        t.fx.wait_for("the upload", move |s| s.queue.completed.len() == before + 1);
    }
    t.fx.app.send(Command::QueueClearCompleted);
    let state = t.fx.wait_for("the clear", |s| s.queue.completed.len() == 1);
    assert_eq!(state.queue.completed[0].item.site, t.a);
}

#[test]
fn queue_commands_without_a_session_raise_a_notice_and_change_nothing() {
    let t = two_sites();
    let commands = || {
        vec![
            Command::Enqueue {
                from: PaneId::Local,
                names: names(&["x.txt"]),
            },
            Command::QueueStartHeld,
            Command::QueueSetSitePaused(true),
            Command::QueueClear,
            Command::QueueClearFailed,
            Command::QueueClearCompleted,
        ]
    };
    for command in commands() {
        let label = format!("{command:?}");
        let before = t.fx.state().notices.len();
        t.fx.app.send(command);
        let state =
            t.fx.wait_for("a notice", move |s| s.notices.len() == before + 1);
        let notice = state.notices.last().unwrap();
        assert_eq!(notice.level, Level::Warning, "{label}");
        assert!(
            notice.text.contains("Connect to a server"),
            "{label}: {}",
            notice.text
        );
    }
    let state = t.fx.state();
    assert!(state.queue.pending.is_empty() && state.prompt.is_none());
    assert!(!state.site_paused());
}
