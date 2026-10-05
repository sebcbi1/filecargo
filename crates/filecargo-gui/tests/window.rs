#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use filecargo_app_core::prelude::*;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AppContext as _, TestAppContext};

fn site(name: &str) -> Site {
    let mut site = Site::new(name, Protocol::Sftp, "example.org");
    site.user = "me".into();
    site
}

#[gpui_kit::test]
async fn the_window_opens_with_the_workspace(cx: &mut TestAppContext) {
    let h = support::open(cx);
    cx.run_until_parked();
    let renders = cx.read_entity(&h.workspace, |w, _| w.renders);
    assert!(renders >= 1, "the workspace drew at least once");
    assert!(cx.read_entity(&h.model, |m, _| m.state.servers.sites().is_empty()));
}

#[gpui_kit::test]
async fn a_new_snapshot_re_renders_the_workspace(cx: &mut TestAppContext) {
    let h = support::open(cx);
    cx.run_until_parked();
    let before = cx.read_entity(&h.workspace, |w, _| w.renders);
    h.app.send(Command::Tree(TreeOp::AddSite(site("work"))));
    h.wait_state(cx, "the new site", |s| s.servers.sites().len() == 1)
        .await;
    cx.update_window(h.window.into(), |_, window, cx| window.render_frame(cx))
        .ok();
    let after = cx.read_entity(&h.workspace, |w, _| w.renders);
    assert!(after > before, "{before} -> {after}");
}

#[gpui_kit::test]
async fn switching_the_theme_re_renders_without_a_restart(cx: &mut TestAppContext) {
    use gpui_kit::component::{ActiveTheme as _, Theme, ThemeMode};
    let h = support::open(cx);
    cx.run_until_parked();
    let before = cx.read_entity(&h.workspace, |w, _| w.renders);
    cx.update(|cx| Theme::change(ThemeMode::Dark, None, cx));
    assert!(cx.update(|cx| cx.theme().is_dark()));
    cx.update_window(h.window.into(), |_, window, cx| window.render_frame(cx))
        .ok();
    cx.update(|cx| Theme::change(ThemeMode::Light, None, cx));
    assert!(!cx.update(|cx| cx.theme().is_dark()));
    cx.update_window(h.window.into(), |_, window, cx| window.render_frame(cx))
        .ok();
    let after = cx.read_entity(&h.workspace, |w, _| w.renders);
    assert!(after > before, "{before} -> {after}");
}
