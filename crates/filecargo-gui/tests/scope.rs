#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::time::Duration;

use filecargo_app_core::prelude::*;
use filecargo_gui::bottom::queue::{ListKind, column_names, row_paths};
use gpui_kit::{AppContext as _, TestAppContext};
use support::Harness;
use support::factory::{TestFactory, site_for};

struct Env {
    h: Harness,
    a: SiteId,
    b: SiteId,
    _dirs: (tempfile::TempDir, tempfile::TempDir, tempfile::TempDir),
}

/// Two sites whose transfers never finish by themselves; the local folder holds `x.txt` and
/// `y.txt`.
async fn env(cx: &mut TestAppContext) -> Env {
    let (sa, sb) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let local = tempfile::tempdir().unwrap();
    for name in ["x.txt", "y.txt"] {
        std::fs::write(local.path().join(name), name).unwrap();
    }
    let factory = TestFactory::new();
    factory.serve("a.example.org", sa.path().to_path_buf(), None);
    factory.serve("b.example.org", sb.path().to_path_buf(), None);
    *factory.transfer_delay.lock().unwrap() = Duration::from_secs(60);
    let f = factory.clone();
    let h = support::open_with(cx, move |o| o.connector = Some(f));
    let mut ids = Vec::new();
    for (name, host) in [("a", "a.example.org"), ("b", "b.example.org")] {
        let mut site = site_for(name, host);
        site.local_dir = Some(local.path().to_path_buf());
        ids.push(site.id);
        h.app.send(Command::Tree(TreeOp::AddSite(site)));
    }
    h.wait_state(cx, "the sites", |s| s.servers.sites().len() == 2)
        .await;
    Env {
        h,
        a: ids[0],
        b: ids[1],
        _dirs: (sa, sb, local),
    }
}

async fn connect(e: &Env, cx: &mut TestAppContext, site: SiteId) {
    e.h.app.send(Command::Connect(site));
    e.h.wait_state(cx, "the session", move |s| {
        s.scope == Some(site) && s.remote.is_some() && s.local.entries.len() >= 2
    })
    .await;
    cx.run_until_parked();
}

fn queue_items(cx: &mut TestAppContext, h: &Harness, kind: ListKind) -> Vec<QueueItem> {
    let bottom = cx.read_entity(&h.workspace, |w, _| w.bottom.clone());
    let table = cx.read_entity(&bottom, |b, _| match kind {
        ListKind::Queue => b.queue.clone(),
        ListKind::Completed => b.completed.clone(),
        ListKind::Failed => b.failed.clone(),
    });
    cx.read_entity(&table, |t, _| {
        t.delegate()
            .items()
            .iter()
            .map(|v| v.item.clone())
            .collect()
    })
}

fn hint_shown(cx: &mut TestAppContext, h: &Harness) -> bool {
    cx.read_entity(&h.workspace, |w, cx| w.other_sites_hint(cx).is_some())
}

fn upload(e: &Env, name: &str) {
    e.h.app.send(Command::Upload {
        names: vec![name.to_owned()],
    });
}

#[gpui_kit::test]
async fn the_bottom_panel_shows_only_the_connected_sites_rows(cx: &mut TestAppContext) {
    let e = env(cx).await;
    connect(&e, cx, e.a).await;
    upload(&e, "x.txt");
    e.h.wait_state(cx, "A's transfer", |s| s.site_queue.pending.len() == 1)
        .await;
    cx.run_until_parked();
    assert_eq!(queue_items(cx, &e.h, ListKind::Queue).len(), 1);
    assert!(!hint_shown(cx, &e.h), "nothing runs on other sites yet");

    connect(&e, cx, e.b).await;
    e.h.wait_state(cx, "B's view", |s| s.other_sites_active == 1)
        .await;
    cx.run_until_parked();
    assert!(
        queue_items(cx, &e.h, ListKind::Queue).is_empty(),
        "A's transfer is not B's business"
    );
    assert!(hint_shown(cx, &e.h), "but it is still running");

    upload(&e, "y.txt");
    e.h.wait_state(cx, "B's transfer", |s| s.site_queue.pending.len() == 1)
        .await;
    cx.run_until_parked();
    let items = queue_items(cx, &e.h, ListKind::Queue);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].site, e.b);

    // not connected: empty lists, and the hint counts both running transfers
    e.h.app.send(Command::Disconnect);
    e.h.wait_state(cx, "the disconnect", |s| s.scope.is_none())
        .await;
    cx.run_until_parked();
    for kind in [ListKind::Queue, ListKind::Completed, ListKind::Failed] {
        assert!(queue_items(cx, &e.h, kind).is_empty(), "{kind:?}");
    }
    assert!(hint_shown(cx, &e.h));
}

#[test]
fn every_list_has_a_local_and_a_remote_path_column() {
    for kind in [ListKind::Queue, ListKind::Completed, ListKind::Failed] {
        let columns = column_names(kind);
        let local = columns.iter().position(|c| *c == "Local");
        let remote = columns.iter().position(|c| *c == "Remote");
        assert!(
            matches!((local, remote), (Some(l), Some(r)) if l < r),
            "{kind:?}: {columns:?}"
        );
    }
}

#[test]
fn rows_show_the_paths_cut_from_the_left() {
    let site = SiteId::new();
    let item = QueueItem {
        id: TransferId(1),
        site,
        direction: Direction::Upload,
        local: "/home/me/projects/site/public/assets/images/header.jpg".into(),
        remote: RemotePath::parse("/www/index.php").unwrap(),
        is_dir: false,
        size: None,
        transferred: 0,
        state: ItemState::Pending,
        attempts: 0,
        parent: None,
        conflict: None,
    };
    let (local, remote) = row_paths(&item);
    assert!(
        local.starts_with('…') && local.ends_with("images/header.jpg"),
        "{local}"
    );
    assert_eq!(remote, "/www/index.php");
}
