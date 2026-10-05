#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::path::PathBuf;
use std::time::Duration;

use filecargo_app_core::prelude::*;
use filecargo_gui::dialogs::{site_editor, tree_ops};
use gpui_kit::component::IndexPath;
use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{AppContext as _, TestAppContext};
use support::Harness;

async fn dialog_open(h: &Harness, cx: &mut TestAppContext) {
    cx.wait_for(h.window.into(), Duration::from_secs(2), |w, _| {
        w.try_find("dialog").is_some()
    })
    .await;
}

fn dialog_is_open(h: &Harness, cx: &mut TestAppContext) -> bool {
    cx.update_window(h.window.into(), |_, window, _| {
        window.try_find("dialog").is_some()
    })
    .unwrap()
}

fn click_ok(h: &Harness, cx: &mut TestAppContext) {
    cx.update_window(h.window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.within("dialog").click("ok", cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn sites(cx: &mut TestAppContext, h: &Harness) -> Vec<Site> {
    cx.read_entity(&h.model, |m, _| m.state.servers.sites().to_vec())
}

#[gpui_kit::test]
async fn the_site_editor_round_trip_create_see_it_in_the_tree_edit_and_persist(
    cx: &mut TestAppContext,
) {
    let h = support::open(cx);
    // ---- create
    let model = h.model.clone();
    let view = cx
        .update_window(h.window.into(), |_, window, cx| {
            site_editor::open(model, None, None, window, cx)
        })
        .unwrap();
    dialog_open(&h, cx).await;
    cx.update_window(h.window.into(), |_, window, cx| {
        view.update(cx, |v, cx| {
            v.name.update(cx, |s, cx| s.set_value("work", window, cx));
            v.host
                .update(cx, |s, cx| s.set_value("example.org", window, cx));
            v.port.update(cx, |s, cx| s.set_value("2222", window, cx));
            v.user.update(cx, |s, cx| s.set_value("me", window, cx));
        });
    })
    .unwrap();
    click_ok(&h, cx);
    h.wait_state(cx, "the new site", |s| s.servers.sites().len() == 1)
        .await;
    cx.run_until_parked();
    assert!(!dialog_is_open(&h, cx), "a saved dialog closes");
    let created = sites(cx, &h).remove(0);
    assert_eq!(
        (created.name.as_str(), created.host.as_str(), created.port),
        ("work", "example.org", Some(2222))
    );

    // ---- it shows in the tree
    let tree = cx.read_entity(&h.workspace, |w, _| w.tree.clone());
    let shown = cx.read_entity(&tree, |t, _| t.node_count());
    assert_eq!(shown, 1);

    // ---- edit
    let model = h.model.clone();
    let original = created.clone();
    let view = cx
        .update_window(h.window.into(), |_, window, cx| {
            site_editor::open(model, Some(original), None, window, cx)
        })
        .unwrap();
    dialog_open(&h, cx).await;
    cx.update_window(h.window.into(), |_, window, cx| {
        view.update(cx, |v, cx| {
            assert_eq!(
                v.values(cx).host,
                "example.org",
                "the editor starts with the stored values"
            );
            v.host
                .update(cx, |s, cx| s.set_value("example.net", window, cx));
            // FTP uses another login set; keep SFTP and switch the login to the agent
            v.auth.update(cx, |s, cx| {
                s.set_selected_index(Some(IndexPath::new(2)), window, cx)
            });
        });
    })
    .unwrap();
    cx.run_until_parked();
    click_ok(&h, cx);
    h.wait_state(cx, "the edit", |s| {
        s.servers
            .sites()
            .first()
            .is_some_and(|x| x.host == "example.net")
    })
    .await;
    let edited = sites(cx, &h).remove(0);
    assert_eq!(edited.id, created.id, "an edit keeps the identity");
    assert_eq!(edited.auth, Auth::Agent);
    assert_eq!(edited.name, "work");
}

#[gpui_kit::test]
async fn an_invalid_form_stays_open_and_says_why(cx: &mut TestAppContext) {
    let h = support::open(cx);
    let model = h.model.clone();
    let view = cx
        .update_window(h.window.into(), |_, window, cx| {
            site_editor::open(model, None, None, window, cx)
        })
        .unwrap();
    dialog_open(&h, cx).await;
    click_ok(&h, cx);
    assert!(dialog_is_open(&h, cx));
    assert_eq!(
        cx.read_entity(&view, |v, _| v.error.clone()).as_deref(),
        Some("Give the site a name.")
    );
    assert!(sites(cx, &h).is_empty());
}

#[gpui_kit::test]
async fn the_password_goes_to_the_keychain_only_when_remembered(cx: &mut TestAppContext) {
    use filecargo_config::{MemoryStore, SecretKey, SecretStore};
    use std::sync::Arc;
    let keychain = Arc::new(MemoryStore::new());
    let kc = keychain.clone();
    let h = support::open_with(cx, move |o| o.secrets = Some(kc));
    let model = h.model.clone();
    let view = cx
        .update_window(h.window.into(), |_, window, cx| {
            site_editor::open(model, None, None, window, cx)
        })
        .unwrap();
    dialog_open(&h, cx).await;
    cx.update_window(h.window.into(), |_, window, cx| {
        view.update(cx, |v, cx| {
            v.name.update(cx, |s, cx| s.set_value("work", window, cx));
            v.host
                .update(cx, |s, cx| s.set_value("example.org", window, cx));
            v.password
                .update(cx, |s, cx| s.set_value("s3cret", window, cx));
            v.set_remember(true, cx);
        });
    })
    .unwrap();
    click_ok(&h, cx);
    h.wait_state(cx, "the site", |s| s.servers.sites().len() == 1)
        .await;
    let id = sites(cx, &h)[0].id;
    for _ in 0..100 {
        if keychain.get(&SecretKey::Password(id)).unwrap().is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("the remembered password never reached the keychain");
}

async fn with_site(cx: &mut TestAppContext) -> (Harness, SiteId) {
    let h = support::open(cx);
    let mut site = Site::new("work", Protocol::Sftp, "example.org");
    site.user = "me".into();
    let id = site.id;
    h.app.send(Command::Tree(TreeOp::AddSite(site)));
    h.wait_state(cx, "the site", |s| s.servers.sites().len() == 1)
        .await;
    (h, id)
}

#[gpui_kit::test]
async fn rename_asks_for_a_name_and_refuses_bad_ones(cx: &mut TestAppContext) {
    let (h, id) = with_site(cx).await;
    let model = h.model.clone();
    let view = cx
        .update_window(h.window.into(), |_, window, cx| {
            tree_ops::rename(model, NodeId::Site(id), "work", window, cx)
        })
        .unwrap();
    dialog_open(&h, cx).await;
    cx.update_window(h.window.into(), |_, window, cx| {
        view.update(cx, |v, cx| {
            v.input.update(cx, |s, cx| s.set_value("a/b", window, cx))
        });
    })
    .unwrap();
    click_ok(&h, cx);
    assert!(dialog_is_open(&h, cx));
    assert_eq!(
        cx.read_entity(&view, |v, _| v.error.clone()).as_deref(),
        Some("The name cannot contain '/'.")
    );
    cx.update_window(h.window.into(), |_, window, cx| {
        view.update(cx, |v, cx| {
            v.input
                .update(cx, |s, cx| s.set_value("office", window, cx))
        });
    })
    .unwrap();
    click_ok(&h, cx);
    h.wait_state(cx, "the rename", |s| s.servers.sites()[0].name == "office")
        .await;
}

#[gpui_kit::test]
async fn new_folder_creates_a_folder_under_the_given_parent(cx: &mut TestAppContext) {
    let h = support::open(cx);
    let model = h.model.clone();
    let view = cx
        .update_window(h.window.into(), |_, window, cx| {
            tree_ops::new_folder(model, None, window, cx)
        })
        .unwrap();
    dialog_open(&h, cx).await;
    cx.update_window(h.window.into(), |_, window, cx| {
        view.update(cx, |v, cx| {
            v.input
                .update(cx, |s, cx| s.set_value("Servers", window, cx))
        });
    })
    .unwrap();
    click_ok(&h, cx);
    h.wait_state(cx, "the folder", |s| {
        s.servers.folders().len() == 1 && s.servers.folders()[0].name == "Servers"
    })
    .await;
}

#[gpui_kit::test]
async fn delete_asks_for_confirmation_and_only_then_deletes(cx: &mut TestAppContext) {
    let (h, id) = with_site(cx).await;
    let model = h.model.clone();
    cx.update_window(h.window.into(), |_, window, cx| {
        tree_ops::delete(model, NodeId::Site(id), "work", window, cx)
    })
    .unwrap();
    dialog_open(&h, cx).await;
    std::thread::sleep(Duration::from_millis(100));
    cx.run_until_parked();
    assert_eq!(
        sites(cx, &h).len(),
        1,
        "nothing is deleted before the answer"
    );
    click_ok(&h, cx);
    h.wait_state(cx, "the delete", |s| s.servers.sites().is_empty())
        .await;
}

#[gpui_kit::test]
async fn move_offers_every_folder_but_the_node_itself_and_moves(cx: &mut TestAppContext) {
    let (h, id) = with_site(cx).await;
    h.app.send(Command::Tree(TreeOp::AddFolder {
        name: "Projects".into(),
        parent: None,
    }));
    h.wait_state(cx, "the folder", |s| s.servers.folders().len() == 1)
        .await;
    let folder = cx.read_entity(&h.model, |m, _| m.state.servers.folders()[0].id);
    let model = h.model.clone();
    let view = cx
        .update_window(h.window.into(), |_, window, cx| {
            tree_ops::move_to(model, NodeId::Site(id), window, cx)
        })
        .unwrap();
    dialog_open(&h, cx).await;
    cx.update_window(h.window.into(), |_, window, cx| {
        view.update(cx, |v, cx| {
            v.select.update(cx, |s, cx| {
                s.set_selected_index(Some(IndexPath::new(1)), window, cx)
            });
        });
    })
    .unwrap();
    cx.run_until_parked();
    click_ok(&h, cx);
    h.wait_state(cx, "the move", move |s| {
        s.servers.sites()[0].folder == Some(folder)
    })
    .await;
}

#[gpui_kit::test]
async fn importing_the_filezilla_fixture_adds_six_sites(cx: &mut TestAppContext) {
    let h = support::open(cx);
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../filecargo-config/tests/fixtures/filezilla/sitemanager.xml");
    let model = h.model.clone();
    let view = cx
        .update_window(h.window.into(), |_, window, cx| {
            tree_ops::import(model, window, cx)
        })
        .unwrap();
    dialog_open(&h, cx).await;
    cx.update_window(h.window.into(), |_, window, cx| {
        view.update(cx, |v, cx| {
            v.path.update(cx, |s, cx| {
                s.set_value(fixture.display().to_string(), window, cx)
            });
        });
    })
    .unwrap();
    click_ok(&h, cx);
    h.wait_state(cx, "the import", |s| s.servers.sites().len() == 6)
        .await;
}
