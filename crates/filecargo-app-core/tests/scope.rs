#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::time::Duration;

use filecargo_app_core::{AppState, Command, PromptKind};
use filecargo_config::SiteId;
use filecargo_transfer::ItemState;
use support::{Fixture, TestFactory, server_tree, site_for};

fn active(state: &AppState) -> usize {
    state
        .queue
        .pending
        .iter()
        .filter(|v| matches!(v.item.state, ItemState::Active { .. }))
        .count()
}

/// Two sites whose transfers never finish on their own; the local folder holds `x.txt` and
/// `y.txt`.
struct Two {
    fx: Fixture,
    a: SiteId,
    b: SiteId,
    _dirs: (tempfile::TempDir, tempfile::TempDir, tempfile::TempDir),
}

fn two_sites() -> Two {
    let (sa, sb) = (server_tree(), server_tree());
    let local = tempfile::tempdir().unwrap();
    std::fs::write(local.path().join("x.txt"), "x").unwrap();
    std::fs::write(local.path().join("y.txt"), "y").unwrap();
    let factory = TestFactory::new();
    factory.serve("host-a", sa.path().to_path_buf(), None);
    factory.serve("host-b", sb.path().to_path_buf(), None);
    *factory.transfer_delay.lock().unwrap() = Duration::from_secs(60);
    let fx = Fixture::with_factory(factory);
    let mut site_a = site_for("a", "host-a");
    site_a.remote_dir = Some("/projects".into());
    site_a.local_dir = Some(local.path().to_path_buf());
    let mut site_b = site_for("b", "host-b");
    site_b.remote_dir = Some("/projects".into());
    site_b.local_dir = Some(local.path().to_path_buf());
    let (a, b) = (fx.add_site(site_a), fx.add_site(site_b));
    Two {
        fx,
        a,
        b,
        _dirs: (sa, sb, local),
    }
}

fn connect(fx: &Fixture, site: SiteId) {
    fx.app.send(Command::Connect(site));
    fx.wait_for("the session", move |s| {
        matches!(&s.session, filecargo_app_core::SessionState::Connected { site: c, .. } if *c == site)
            && s.remote.is_some()
            && s.local.entries.len() >= 2
    });
}

#[test]
fn the_scoped_queue_follows_the_connected_site() {
    let t = two_sites();
    let fx = &t.fx;

    connect(fx, t.a);
    fx.app.send(Command::Upload {
        names: vec!["x.txt".into()],
    });
    let state = fx.wait_for("A's transfer to run", |s| active(s) == 1);
    assert_eq!(state.scope, Some(t.a));
    assert_eq!(state.site_queue.pending.len(), 1);
    assert_eq!(state.other_sites_active, 0);

    // B is connected while A's transfer keeps running in the background
    connect(fx, t.b);
    let state = fx.wait_for("the view of B", |s| s.scope == Some(t.b));
    assert!(state.site_queue.pending.is_empty(), "B has nothing queued");
    assert_eq!(state.other_sites_active, 1);
    assert_eq!(state.queue.pending.len(), 1, "the full queue still has A's");

    fx.app.send(Command::Upload {
        names: vec!["y.txt".into()],
    });
    let state = fx.wait_for("B's transfer to run", |s| active(s) == 2);
    assert_eq!(state.site_queue.pending.len(), 1);
    assert!(
        state.site_queue.pending.iter().all(|v| v.item.site == t.b),
        "only B's items"
    );
    assert_eq!(state.other_sites_active, 1, "A's");

    // not connected: the lists are empty and every active transfer is "on another site"
    fx.app.send(Command::Disconnect);
    let state = fx.wait_for("the disconnect", |s| s.scope.is_none());
    assert!(state.site_queue.pending.is_empty());
    assert!(state.site_queue.completed.is_empty() && state.site_queue.failed.is_empty());
    assert_eq!(state.other_sites_active, 2);

    // quitting still counts every site
    fx.app.send(Command::Quit);
    let state = fx.wait_for("the quit question", |s| s.prompt.is_some());
    assert!(matches!(
        state.prompt.as_ref().unwrap().kind,
        PromptKind::ConfirmQuit {
            active_transfers: 2
        }
    ));
}

#[test]
fn the_scoped_snapshot_is_rebuilt_only_when_the_queue_or_the_scope_changes() {
    let t = two_sites();
    connect(&t.fx, t.a);
    let first = t.fx.state();
    // an unrelated change publishes a new snapshot but shares the scoped queue
    t.fx.app.send(Command::SetSort {
        pane: filecargo_app_core::PaneId::Local,
        sort: filecargo_app_core::Sort {
            key: filecargo_app_core::SortKey::Size,
            ascending: true,
        },
    });
    let second = t.fx.wait_for("the new sort", |s| {
        s.local.sort.key == filecargo_app_core::SortKey::Size
    });
    assert!(std::sync::Arc::ptr_eq(&first.queue, &second.queue));
    assert!(std::sync::Arc::ptr_eq(
        &first.site_queue,
        &second.site_queue
    ));
}
