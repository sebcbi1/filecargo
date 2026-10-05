#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::time::Duration;

use filecargo_app_core::prelude::*;
use gpui_kit::Focusable as _;
use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{AppContext as _, TestAppContext};
use support::Harness;
use support::factory::TestFactory;

struct Env {
    h: Harness,
    server: tempfile::TempDir,
    local: tempfile::TempDir,
}

/// Connected; server holds `a.txt`, local folder holds `sub/`.
async fn env(cx: &mut TestAppContext) -> Env {
    let server = tempfile::tempdir().unwrap();
    std::fs::write(server.path().join("a.txt"), "a").unwrap();
    let local = tempfile::tempdir().unwrap();
    std::fs::create_dir(local.path().join("sub")).unwrap();
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
    let wanted = local.path().to_path_buf();
    h.wait_state(cx, "the session", move |s| {
        s.remote.as_ref().is_some_and(|r| !r.entries.is_empty())
            && s.local.path == wanted
            && !s.local.entries.is_empty()
    })
    .await;
    cx.run_until_parked();
    Env { h, server, local }
}

fn focus_pane(cx: &mut TestAppContext, h: &Harness, remote: bool) {
    let pane = cx.read_entity(&h.workspace, |w, _| {
        if remote {
            w.remote.clone()
        } else {
            w.local.clone()
        }
    });
    let table = cx.read_entity(&pane, |p, _| p.table.clone());
    cx.update_window(h.window.into(), |_, window, cx| {
        table.update(cx, |t, cx| t.focus_handle(cx).focus(window, cx));
        window.render_frame(cx);
    })
    .unwrap();
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

#[gpui_kit::test]
async fn f7_makes_a_remote_folder_through_a_dialog(cx: &mut TestAppContext) {
    let e = env(cx).await;
    focus_pane(cx, &e.h, true);
    press(cx, &e.h, "f7");
    dialog_open(&e.h, cx).await;
    cx.update_window(e.h.window.into(), |_, window, cx| {
        window.within("dialog").click("text-prompt", cx);
        window.input("photos", cx);
        window.within("dialog").click("ok", cx);
    })
    .unwrap();
    cx.run_until_parked();
    wait_path(e.server.path().join("photos"), true).await;
}

#[gpui_kit::test]
async fn f2_renames_the_entry_under_the_cursor(cx: &mut TestAppContext) {
    let e = env(cx).await;
    focus_pane(cx, &e.h, true);
    let pane = cx.read_entity(&e.h.workspace, |w, _| w.remote.clone());
    let table = cx.read_entity(&pane, |p, _| p.table.clone());
    table.update(cx, |t, cx| t.set_selected_row(0, cx));
    press(cx, &e.h, "f2");
    dialog_open(&e.h, cx).await;
    cx.update_window(e.h.window.into(), |_, window, cx| {
        window.within("dialog").click("text-prompt", cx);
        window.input("2", cx); // the field starts with the current name
        window.within("dialog").click("ok", cx);
    })
    .unwrap();
    cx.run_until_parked();
    wait_path(e.server.path().join("a.txt2"), true).await;
}

#[gpui_kit::test]
async fn delete_asks_through_the_app_and_removes_the_file(cx: &mut TestAppContext) {
    let e = env(cx).await;
    focus_pane(cx, &e.h, true);
    let pane = cx.read_entity(&e.h.workspace, |w, _| w.remote.clone());
    let table = cx.read_entity(&pane, |p, _| p.table.clone());
    table.update(cx, |t, cx| t.set_selected_row(0, cx));
    press(cx, &e.h, "delete");
    dialog_open(&e.h, cx).await;
    assert!(
        e.server.path().join("a.txt").exists(),
        "nothing is deleted before the confirmation"
    );
    cx.update_window(e.h.window.into(), |_, window, cx| {
        window.within("dialog").click("ok", cx)
    })
    .unwrap();
    cx.run_until_parked();
    wait_path(e.server.path().join("a.txt"), false).await;
}

#[gpui_kit::test]
async fn secondary_l_focuses_the_path_bar_and_enter_navigates(cx: &mut TestAppContext) {
    let e = env(cx).await;
    focus_pane(cx, &e.h, false);
    press(cx, &e.h, "secondary-l");
    let pane = cx.read_entity(&e.h.workspace, |w, _| w.local.clone());
    let input = cx.read_entity(&pane, |p, _| p.path_input.clone());
    let sub = e.local.path().join("sub").display().to_string();
    cx.update_window(e.h.window.into(), |_, window, cx| {
        assert!(
            input.read(cx).focus_handle(cx).is_focused(window),
            "the path bar has the focus"
        );
        input.update(cx, |s, cx| s.set_value(sub.clone(), window, cx));
        window.press("enter", cx);
    })
    .unwrap();
    let inside = e.local.path().join("sub");
    e.h.wait_state(cx, "to enter sub", move |s| s.local.path == inside)
        .await;
}

#[gpui_kit::test]
async fn secondary_digits_pick_the_bottom_tab_and_ctrl_shift_digits_work_from_the_terminal(
    cx: &mut TestAppContext,
) {
    let e = env(cx).await;
    focus_pane(cx, &e.h, false);
    let bottom = cx.read_entity(&e.h.workspace, |w, _| w.bottom.clone());
    press(cx, &e.h, "secondary-3");
    assert_eq!(cx.read_entity(&bottom, |b, _| b.tab), 2);
    press(cx, &e.h, "secondary-4");
    assert_eq!(cx.read_entity(&bottom, |b, _| b.tab), 3);
    // the terminal takes the focus; secondary-1 is the shell's there, ctrl-shift-1 is ours
    press(cx, &e.h, "ctrl-shift-5");
    assert_eq!(cx.read_entity(&bottom, |b, _| b.tab), 4);
    cx.update_window(e.h.window.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
    press(cx, &e.h, "ctrl-shift-1");
    assert_eq!(cx.read_entity(&bottom, |b, _| b.tab), 0);
}

#[gpui_kit::test]
async fn secondary_k_connects_the_selected_site(cx: &mut TestAppContext) {
    let e = env(cx).await;
    e.h.app.send(Command::Disconnect);
    e.h.wait_state(cx, "to disconnect", |s| s.remote.is_none())
        .await;
    // select the site by clicking its row, then press the shortcut
    cx.update_window(e.h.window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.within("server-tree").click(0usize, cx);
    })
    .unwrap();
    cx.run_until_parked();
    press(cx, &e.h, "secondary-k");
    e.h.wait_state(cx, "the session", |s| s.remote.is_some())
        .await;
}
