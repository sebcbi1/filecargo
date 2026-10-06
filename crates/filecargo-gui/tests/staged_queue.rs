#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::time::Duration;

use filecargo_app_core::prelude::*;
use filecargo_gui::bottom::queue::{ListKind, menu_entries, waiting_label};
use filecargo_gui::pane::menu_labels;
use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{AppContext as _, Modifiers, TestAppContext};
use support::Harness;
use support::factory::TestFactory;

struct Env {
    h: Harness,
    server: tempfile::TempDir,
    _local: tempfile::TempDir,
}

/// Connected; the local folder holds `x.txt` and `y.txt` (rows 1 and 2 after `..`).
async fn env(cx: &mut TestAppContext) -> Env {
    let server = tempfile::tempdir().unwrap();
    let local = tempfile::tempdir().unwrap();
    for name in ["x.txt", "y.txt"] {
        std::fs::write(local.path().join(name), name).unwrap();
    }
    let factory = TestFactory::new();
    factory.serve("h.example.org", server.path().to_path_buf(), None);
    let f = factory.clone();
    let h = support::open_with(cx, move |o| o.connector = Some(f));
    let mut site = Site::new("s", Protocol::Sftp, "h.example.org");
    site.user = "me".into();
    site.local_dir = Some(local.path().to_path_buf());
    let id = site.id;
    h.app.send(Command::Tree(TreeOp::AddSite(site)));
    h.wait_state(cx, "the site", |s| s.servers.sites().len() == 1)
        .await;
    h.app.send(Command::Connect(id));
    let wanted = support::canonical(local.path());
    h.wait_state(cx, "the session", move |s| {
        s.remote.is_some() && s.local.path == wanted && s.local.entries.len() == 2
    })
    .await;
    cx.run_until_parked();
    Env {
        h,
        server,
        _local: local,
    }
}

fn held(s: &AppState) -> usize {
    s.site_queue
        .pending
        .iter()
        .filter(|v| matches!(v.item.state, ItemState::Held))
        .count()
}

fn click(cx: &mut TestAppContext, h: &Harness, id: &'static str) {
    cx.update_window(h.window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn select_local_row(cx: &mut TestAppContext, h: &Harness, row: usize) {
    let pane = cx.read_entity(&h.workspace, |w, _| w.local.clone());
    let table = cx.read_entity(&pane, |p, _| p.table.clone());
    table.update(cx, |t, cx| {
        t.delegate_mut().click_row(row, Modifiers::default());
        cx.notify();
    });
}

async fn dialog_open(h: &Harness, cx: &mut TestAppContext) {
    cx.wait_for(h.window.into(), Duration::from_secs(5), |w, _| {
        w.try_find("dialog").is_some()
    })
    .await;
    cx.run_until_parked();
}

fn answer(cx: &mut TestAppContext, h: &Harness, button: &'static str) {
    cx.update_window(h.window.into(), |_, window, cx| {
        window.within("dialog").click(button, cx)
    })
    .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
async fn add_to_queue_holds_the_selection_and_start_queue_transfers_it(cx: &mut TestAppContext) {
    let e = env(cx).await;
    select_local_row(cx, &e.h, 1);
    click(cx, &e.h, "add-to-queue");
    e.h.wait_state(cx, "a held item", |s| held(s) == 1).await;
    std::thread::sleep(Duration::from_millis(300));
    assert!(!e.server.path().join("x.txt").exists(), "nothing started");
    assert_eq!(snap(&e.h).queue.completed.len(), 0);

    click(cx, &e.h, "queue-start");
    e.h.wait_state(cx, "the transfer", |s| s.queue.completed.len() == 1)
        .await;
    assert!(e.server.path().join("x.txt").exists());
}

#[gpui_kit::test]
async fn the_pause_toggle_is_per_site_and_flips_back(cx: &mut TestAppContext) {
    let e = env(cx).await;
    click(cx, &e.h, "queue-pause");
    e.h.wait_state(cx, "paused", |s| s.site_paused()).await;
    cx.run_until_parked();
    click(cx, &e.h, "queue-pause");
    e.h.wait_state(cx, "resumed", |s| !s.site_paused()).await;
    assert!(snap(&e.h).queue.processing, "the global switch stays on");
}

#[gpui_kit::test]
async fn clear_queue_asks_and_only_a_yes_empties_the_queue(cx: &mut TestAppContext) {
    let e = env(cx).await;
    e.h.app.send(Command::Enqueue {
        from: PaneId::Local,
        names: vec!["x.txt".into(), "y.txt".into()],
    });
    e.h.wait_state(cx, "held items", |s| held(s) == 2).await;
    cx.run_until_parked();

    click(cx, &e.h, "queue-clear");
    dialog_open(&e.h, cx).await;
    assert!(matches!(
        snap(&e.h).prompt.as_ref().map(|p| &p.kind),
        Some(PromptKind::ConfirmClearQueue { items: 2, .. })
    ));
    answer(cx, &e.h, "cancel");
    e.h.wait_state(cx, "the prompt to go", |s| s.prompt.is_none())
        .await;
    assert_eq!(held(&snap(&e.h)), 2, "declining clears nothing");

    click(cx, &e.h, "queue-clear");
    dialog_open(&e.h, cx).await;
    answer(cx, &e.h, "ok");
    e.h.wait_state(cx, "the clear", |s| s.site_queue.pending.is_empty())
        .await;
}

#[test]
fn menus_and_labels_offer_the_new_actions() {
    for pane in [PaneId::Local, PaneId::Remote] {
        assert!(menu_labels(pane).contains(&"Add to queue"), "{pane:?}");
    }
    let failed = menu_entries(ListKind::Failed);
    let (_, clear) = failed
        .iter()
        .find(|(label, _)| *label == "Clear failed")
        .expect("Clear failed in the Failed menu");
    assert_eq!(format!("{:?}", clear(TransferId(1))), "QueueClearFailed");
    assert!(
        !menu_entries(ListKind::Queue)
            .iter()
            .any(|(label, _)| *label == "Clear failed")
    );
    assert_eq!(waiting_label(&ItemState::Held), "queued (held)");
    assert_eq!(waiting_label(&ItemState::Pending), "queued");
}

fn snap(h: &Harness) -> std::sync::Arc<AppState> {
    h.app.state().borrow().clone()
}
