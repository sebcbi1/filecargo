#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use filecargo_app_core::prelude::*;
use gpui_kit::Focusable as _;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AppContext as _, Modifiers, TestAppContext};
use support::Harness;
use support::factory::TestFactory;

struct Env {
    h: Harness,
    factory: Arc<TestFactory>,
    server: tempfile::TempDir,
    local: tempfile::TempDir,
}

/// Connected to a server holding `down.txt`; the local folder holds `up.txt`. Rows of the local
/// pane: 0 `..`, 1 `up.txt`; of the remote pane (root, no `..`): 0 `down.txt`.
async fn env(cx: &mut TestAppContext) -> Env {
    let server = tempfile::tempdir().unwrap();
    std::fs::write(server.path().join("down.txt"), "from the server").unwrap();
    let local = tempfile::tempdir().unwrap();
    std::fs::write(local.path().join("up.txt"), "from here").unwrap();
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
        s.remote.as_ref().is_some_and(|r| !r.entries.is_empty())
            && s.local.path == wanted
            && !s.local.entries.is_empty()
    })
    .await;
    cx.run_until_parked();
    Env {
        h,
        factory,
        server,
        local,
    }
}

async fn wait_file(path: &std::path::Path, content: &str) {
    for _ in 0..200 {
        if std::fs::read_to_string(path).is_ok_and(|c| c == content) {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("{} never got its content", path.display());
}

fn local_table(
    cx: &mut TestAppContext,
    h: &Harness,
) -> gpui_kit::Entity<gpui_kit::component::table::TableState<filecargo_gui::pane::PaneDelegate>> {
    let pane = cx.read_entity(&h.workspace, |w, _| w.local.clone());
    cx.read_entity(&pane, |p, _| p.table.clone())
}

fn remote_table(
    cx: &mut TestAppContext,
    h: &Harness,
) -> gpui_kit::Entity<gpui_kit::component::table::TableState<filecargo_gui::pane::PaneDelegate>> {
    let pane = cx.read_entity(&h.workspace, |w, _| w.remote.clone());
    cx.read_entity(&pane, |p, _| p.table.clone())
}

#[gpui_kit::test]
async fn f5_uploads_the_selection_from_the_focused_local_pane(cx: &mut TestAppContext) {
    let e = env(cx).await;
    let table = local_table(cx, &e.h);
    table.update(cx, |t, cx| {
        t.delegate_mut().click_row(1, Modifiers::default());
        cx.notify();
    });
    cx.update_window(e.h.window.into(), |_, window, cx| {
        table.update(cx, |t, cx| t.focus_handle(cx).focus(window, cx));
        window.render_frame(cx);
        window.press("f5", cx);
    })
    .unwrap();
    wait_file(&e.server.path().join("up.txt"), "from here").await;
    assert!(
        cx.read_entity(&table, |t, _| t.delegate().selected.is_empty()),
        "the selection is cleared once it is queued"
    );
}

#[gpui_kit::test]
async fn the_download_button_downloads_from_the_remote_pane(cx: &mut TestAppContext) {
    let e = env(cx).await;
    let table = remote_table(cx, &e.h);
    table.update(cx, |t, cx| {
        t.delegate_mut().click_row(0, Modifiers::default());
        cx.notify();
    });
    cx.update_window(e.h.window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("download", cx);
    })
    .unwrap();
    wait_file(&e.local.path().join("down.txt"), "from the server").await;
}

#[gpui_kit::test]
async fn double_clicking_a_file_sends_it_to_the_other_side(cx: &mut TestAppContext) {
    let e = env(cx).await;
    cx.update_window(e.h.window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.double_click(("row", 1usize), cx);
    })
    .unwrap();
    wait_file(&e.server.path().join("up.txt"), "from here").await;
}

#[gpui_kit::test]
async fn the_queue_lists_a_running_transfer_pause_toggles_and_delete_removes_it(
    cx: &mut TestAppContext,
) {
    let e = env(cx).await;
    *e.factory.transfer_delay.lock().unwrap() = Duration::from_secs(60);
    // a new connection picks the delay up
    e.h.app.send(Command::Disconnect);
    e.h.wait_state(cx, "to disconnect", |s| s.remote.is_none())
        .await;
    let site = cx.read_entity(&e.h.model, |m, _| m.state.servers.sites()[0].id);
    e.h.app.send(Command::Connect(site));
    e.h.wait_state(cx, "to reconnect", |s| s.remote.is_some())
        .await;
    e.h.app.send(Command::Upload {
        names: vec!["up.txt".into()],
    });
    e.h.wait_state(cx, "the queued item", |s| s.queue.pending.len() == 1)
        .await;
    cx.run_until_parked();
    let bottom = cx.read_entity(&e.h.workspace, |w, _| w.bottom.clone());
    let queue = cx.read_entity(&bottom, |b, _| b.queue.clone());
    assert_eq!(cx.read_entity(&queue, |t, _| t.delegate().items().len()), 1);

    // pause / resume this site through the Queue tab
    cx.update_window(e.h.window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("queue-pause", cx);
    })
    .unwrap();
    e.h.wait_state(cx, "paused", |s| s.site_paused()).await;
    cx.update_window(e.h.window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("queue-pause", cx);
    })
    .unwrap();
    e.h.wait_state(cx, "resumed", |s| !s.site_paused()).await;

    // Delete removes the item under the cursor of the list on show
    queue.update(cx, |t, cx| t.set_selected_row(0, cx));
    cx.update_window(e.h.window.into(), |_, window, cx| {
        queue.update(cx, |t, cx| t.focus_handle(cx).focus(window, cx));
        window.render_frame(cx);
        window.press("delete", cx);
    })
    .unwrap();
    e.h.wait_state(cx, "the removal", |s| s.queue.pending.is_empty())
        .await;
}

#[gpui_kit::test]
async fn the_tabs_switch_and_the_log_shows_what_the_app_logged(cx: &mut TestAppContext) {
    let e = env(cx).await;
    let bottom = cx.read_entity(&e.h.workspace, |w, _| w.bottom.clone());
    cx.update_window(e.h.window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.within("bottom-tabs").click(3usize, cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(cx.read_entity(&bottom, |b, _| b.tab), 3);
    let log = e.h.app.log();
    for n in 0..50 {
        log.push(LogLine {
            time: std::time::UNIX_EPOCH,
            level: LogLevel::Info,
            target: "test".into(),
            message: format!("line {n}"),
            site: None,
        });
    }
    cx.update_window(e.h.window.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
    let view = cx.read_entity(&bottom, |b, _| b.log.clone());
    assert!(cx.read_entity(&view, |v, _| v.len()) >= 50);
    assert!(
        cx.read_entity(&view, |v, _| v.follow),
        "the log follows the newest line"
    );
}
