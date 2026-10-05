#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use filecargo_app_core::prelude::*;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AppContext as _, TestAppContext};
use support::factory::RootedFactory;

fn site(name: &str, host: &str) -> Site {
    let mut site = Site::new(name, Protocol::Sftp, host);
    site.user = "me".into();
    site
}

/// An app with a served `host` (`index.php`, `html/`) and the sites `work` (folder `Work`)
/// and `nas`; folder `Work` is open: rows 0 `Work`, 1 `prod`, 2 `nas`.
async fn setup(cx: &mut TestAppContext) -> (support::Harness, tempfile::TempDir, SiteId) {
    let served = tempfile::tempdir().unwrap();
    std::fs::create_dir(served.path().join("html")).unwrap();
    std::fs::write(served.path().join("index.php"), "<?php").unwrap();
    let factory = RootedFactory::new();
    factory.serve("prod.example.org", served.path().to_path_buf());
    let h = support::open_with(cx, |options| options.connector = Some(factory));

    h.app.send(Command::Tree(TreeOp::AddFolder {
        name: "Work".into(),
        parent: None,
    }));
    h.wait_state(cx, "the folder", |s| s.servers.folders().len() == 1)
        .await;
    let folder = cx.read_entity(&h.model, |m, _| m.state.servers.folders()[0].id);
    let mut prod = site("prod", "prod.example.org");
    prod.folder = Some(folder);
    let prod_id = prod.id;
    h.app.send(Command::Tree(TreeOp::AddSite(prod)));
    h.app.send(Command::Tree(TreeOp::AddSite(site(
        "nas",
        "nas.example.org",
    ))));
    h.wait_state(cx, "the sites", |s| s.servers.sites().len() == 2)
        .await;
    cx.run_until_parked();
    // open the folder by clicking its row
    cx.update_window(h.window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(0usize, cx);
    })
    .unwrap();
    cx.run_until_parked();
    (h, served, prod_id)
}

#[gpui_kit::test]
async fn the_tree_shows_folders_and_sites_and_a_click_selects_a_row(cx: &mut TestAppContext) {
    let (h, _served, prod) = setup(cx).await;
    cx.update_window(h.window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(1usize, cx);
    })
    .unwrap();
    cx.run_until_parked();
    let tree = cx.read_entity(&h.workspace, |w, _| w.tree.clone());
    let selected = cx.read_entity(&tree, |t, _| t.selected);
    assert_eq!(selected, Some(NodeId::Site(prod)));
}

#[gpui_kit::test]
async fn double_clicking_a_site_connects_and_the_remote_pane_lists_the_server(
    cx: &mut TestAppContext,
) {
    let (h, _served, prod) = setup(cx).await;
    cx.update_window(h.window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.double_click(1usize, cx);
    })
    .unwrap();
    h.wait_state(cx, "the session", move |s| {
        matches!(&s.session, SessionState::Connected { site, .. } if *site == prod)
            && s.remote.as_ref().is_some_and(|r| !r.entries.is_empty())
    })
    .await;
    cx.run_until_parked();
    let names: Vec<String> = cx.read_entity(&h.model, |m, _| {
        m.state
            .remote
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .map(|e| e.name.clone())
            .collect()
    });
    assert_eq!(names, ["html", "index.php"]);
    // the remote pane's table got them too
    let remote = cx.read_entity(&h.workspace, |w, _| w.remote.clone());
    let table = cx.read_entity(&remote, |p, _| p.table.clone());
    let shown = cx.read_entity(&table, |t, _| t.delegate().entry(0).map(|e| e.name.clone()));
    assert_eq!(shown.as_deref(), Some("html"), "no `..` row at the root");
}

#[gpui_kit::test]
async fn the_toolbar_connects_the_selected_site_and_disconnects(cx: &mut TestAppContext) {
    let (h, _served, prod) = setup(cx).await;
    cx.update_window(h.window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(1usize, cx); // select prod
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(h.window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("connect", cx);
    })
    .unwrap();
    h.wait_state(
        cx,
        "the session",
        move |s| matches!(&s.session, SessionState::Connected { site, .. } if *site == prod),
    )
    .await;
    cx.update_window(h.window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("disconnect", cx);
    })
    .unwrap();
    h.wait_state(cx, "to disconnect", |s| {
        matches!(s.session, SessionState::Disconnected) && s.remote.is_none()
    })
    .await;
}

#[gpui_kit::test]
async fn deleting_a_site_updates_the_tree_and_clears_the_selection(cx: &mut TestAppContext) {
    let (h, _served, prod) = setup(cx).await;
    cx.update_window(h.window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(1usize, cx);
    })
    .unwrap();
    cx.run_until_parked();
    h.app.send(Command::Tree(TreeOp::Delete {
        node: NodeId::Site(prod),
    }));
    h.wait_state(cx, "the delete", |s| s.servers.sites().len() == 1)
        .await;
    cx.run_until_parked();
    let tree = cx.read_entity(&h.workspace, |w, _| w.tree.clone());
    assert_eq!(cx.read_entity(&tree, |t, _| t.selected), None);
}
