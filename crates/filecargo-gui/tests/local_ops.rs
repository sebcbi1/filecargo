#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::time::Duration;

use filecargo_app_core::prelude::*;
use filecargo_gui::pane::menu_labels;
use gpui_kit::Focusable as _;
use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{AppContext as _, TestAppContext};
use support::Harness;
use support::factory::TestFactory;

struct Env {
    h: Harness,
    local: tempfile::TempDir,
    _server: tempfile::TempDir,
}

/// Connected; the local folder holds `sub/`, `x.txt` and `y.txt` (rows 1, 2, 3 after `..`).
async fn env(cx: &mut TestAppContext) -> Env {
    let server = tempfile::tempdir().unwrap();
    std::fs::write(server.path().join("a.txt"), "a").unwrap();
    let local = tempfile::tempdir().unwrap();
    std::fs::create_dir(local.path().join("sub")).unwrap();
    std::fs::write(local.path().join("x.txt"), "x").unwrap();
    std::fs::write(local.path().join("y.txt"), "y").unwrap();
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
            && s.local.entries.len() == 3
    })
    .await;
    cx.run_until_parked();
    Env {
        h,
        local,
        _server: server,
    }
}

fn focus_local(cx: &mut TestAppContext, h: &Harness) {
    let pane = cx.read_entity(&h.workspace, |w, _| w.local.clone());
    let table = cx.read_entity(&pane, |p, _| p.table.clone());
    cx.update_window(h.window.into(), |_, window, cx| {
        table.update(cx, |t, cx| t.focus_handle(cx).focus(window, cx));
        window.render_frame(cx);
    })
    .unwrap();
}

fn select_row(cx: &mut TestAppContext, h: &Harness, row: usize) {
    let pane = cx.read_entity(&h.workspace, |w, _| w.local.clone());
    let table = cx.read_entity(&pane, |p, _| p.table.clone());
    table.update(cx, |t, cx| t.set_selected_row(row, cx));
}

fn press(cx: &mut TestAppContext, h: &Harness, key: &'static str) {
    cx.update_window(h.window.into(), |_, window, cx| window.press(key, cx))
        .unwrap();
    cx.run_until_parked();
}

async fn dialog_open(h: &Harness, cx: &mut TestAppContext) {
    cx.wait_for(h.window.into(), Duration::from_secs(5), |w, _| {
        w.try_find("dialog").is_some()
    })
    .await;
    cx.run_until_parked();
}

fn type_name_and_ok(cx: &mut TestAppContext, h: &Harness, text: &'static str) {
    cx.update_window(h.window.into(), |_, window, cx| {
        window.within("dialog").click("text-prompt", cx);
        window.input(text, cx);
        window.within("dialog").click("ok", cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn click_ok(cx: &mut TestAppContext, h: &Harness) {
    cx.update_window(h.window.into(), |_, window, cx| {
        window.within("dialog").click("ok", cx)
    })
    .unwrap();
    cx.run_until_parked();
}

async fn wait_path(path: std::path::PathBuf, present: bool) {
    for _ in 0..200 {
        if path.exists() == present {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!(
        "{} never {}",
        path.display(),
        if present { "appeared" } else { "went away" }
    );
}

#[test]
fn the_local_menu_offers_the_same_operations_as_the_remote_one() {
    let local = menu_labels(PaneId::Local);
    for label in ["Upload", "New folder…", "Rename…", "Delete", "Refresh"] {
        assert!(local.contains(&label), "{label} in {local:?}");
    }
    assert_eq!(
        local.contains(&"Permissions…"),
        cfg!(unix),
        "permissions on Unix only"
    );
    let remote = menu_labels(PaneId::Remote);
    for label in [
        "Download",
        "New folder…",
        "Rename…",
        "Delete",
        "Permissions…",
        "Refresh",
    ] {
        assert!(remote.contains(&label), "{label} in {remote:?}");
    }
}

#[gpui_kit::test]
async fn f7_on_the_local_pane_makes_a_local_folder(cx: &mut TestAppContext) {
    let e = env(cx).await;
    focus_local(cx, &e.h);
    press(cx, &e.h, "f7");
    dialog_open(&e.h, cx).await;
    type_name_and_ok(cx, &e.h, "photos");
    wait_path(e.local.path().join("photos"), true).await;
}

#[gpui_kit::test]
async fn f2_on_the_local_pane_renames_the_entry_under_the_cursor(cx: &mut TestAppContext) {
    let e = env(cx).await;
    focus_local(cx, &e.h);
    select_row(cx, &e.h, 2); // x.txt
    press(cx, &e.h, "f2");
    dialog_open(&e.h, cx).await;
    type_name_and_ok(cx, &e.h, "2");
    wait_path(e.local.path().join("x.txt2"), true).await;
    assert!(!e.local.path().join("x.txt").exists());
}

#[gpui_kit::test]
async fn delete_on_the_local_pane_asks_then_removes_the_file(cx: &mut TestAppContext) {
    let e = env(cx).await;
    focus_local(cx, &e.h);
    select_row(cx, &e.h, 2); // x.txt
    press(cx, &e.h, "delete");
    dialog_open(&e.h, cx).await;
    assert!(e.local.path().join("x.txt").exists(), "asks first");
    click_ok(cx, &e.h);
    wait_path(e.local.path().join("x.txt"), false).await;
    assert!(e.local.path().join("y.txt").exists());
}

#[gpui_kit::test]
async fn the_toolbar_acts_on_the_last_focused_pane(cx: &mut TestAppContext) {
    let e = env(cx).await;
    cx.update_window(e.h.window.into(), |_, window, cx| {
        window.render_frame(cx);
    })
    .unwrap();
    e.h.workspace
        .update(cx, |w, _| w.focused_pane = PaneId::Local);
    select_row(cx, &e.h, 3); // y.txt
    cx.update_window(e.h.window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("delete", cx);
    })
    .unwrap();
    cx.run_until_parked();
    dialog_open(&e.h, cx).await;
    assert!(e.local.path().join("y.txt").exists(), "asks first");
    click_ok(cx, &e.h);
    wait_path(e.local.path().join("y.txt"), false).await;
}

#[gpui_kit::test]
async fn the_toolbar_folder_button_makes_a_local_folder_when_local_is_focused(
    cx: &mut TestAppContext,
) {
    let e = env(cx).await;
    e.h.workspace
        .update(cx, |w, _| w.focused_pane = PaneId::Local);
    cx.update_window(e.h.window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("new-pane-folder", cx);
    })
    .unwrap();
    cx.run_until_parked();
    dialog_open(&e.h, cx).await;
    type_name_and_ok(cx, &e.h, "from-toolbar");
    wait_path(e.local.path().join("from-toolbar"), true).await;
}
