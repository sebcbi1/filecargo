#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::time::Duration;

use filecargo_app_core::prelude::*;
#[cfg(unix)]
use filecargo_gui::dialogs::permissions;
use filecargo_gui::dialogs::settings;
use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{AppContext as _, TestAppContext};
use support::Harness;
#[cfg(unix)]
use support::factory::TestFactory;

async fn dialog_open(h: &Harness, cx: &mut TestAppContext) {
    cx.wait_for(h.window.into(), Duration::from_secs(5), |w, _| {
        w.try_find("dialog").is_some()
    })
    .await;
    cx.run_until_parked();
}

fn click(h: &Harness, cx: &mut TestAppContext, id: &'static str) {
    cx.update_window(h.window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.within("dialog").click(id, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

#[cfg(unix)]
#[gpui_kit::test]
async fn the_permissions_dialog_keeps_octal_and_boxes_in_step_and_applies_chmod(
    cx: &mut TestAppContext,
) {
    use std::os::unix::fs::PermissionsExt as _;
    let server = tempfile::tempdir().unwrap();
    let file = server.path().join("a.txt");
    std::fs::write(&file, "x").unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o640)).unwrap();
    let factory = TestFactory::new();
    factory.serve("h.example.org", server.path().to_path_buf(), None);
    let f = factory.clone();
    let h = support::open_with(cx, move |o| o.connector = Some(f));
    let mut site = Site::new("s", Protocol::Sftp, "h.example.org");
    site.user = "me".into();
    let id = site.id;
    h.app.send(Command::Tree(TreeOp::AddSite(site)));
    h.wait_state(cx, "the site", |s| s.servers.sites().len() == 1)
        .await;
    h.app.send(Command::Connect(id));
    h.wait_state(cx, "the session", |s| s.remote.is_some())
        .await;

    let model = h.model.clone();
    let view = cx
        .update_window(h.window.into(), |_, window, cx| {
            permissions::open(model, vec!["a.txt".into()], 0o640, window, cx)
        })
        .unwrap();
    dialog_open(&h, cx).await;
    assert_eq!(cx.read_entity(&view, |v, _| v.mode), 0o640);
    click(&h, cx, "bit-10"); // group execute
    assert_eq!(cx.read_entity(&view, |v, _| v.mode), 0o650);
    let octal = cx.read_entity(&view, |v, _| v.octal.clone());
    assert_eq!(cx.read_entity(&octal, |s, _| s.value().to_string()), "650");
    click(&h, cx, "ok");
    for _ in 0..200 {
        let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        if mode == 0o650 {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("the mode was not changed");
}

#[gpui_kit::test]
async fn the_settings_dialog_saves_and_refuses_bad_numbers(cx: &mut TestAppContext) {
    let h = support::open(cx);
    let model = h.model.clone();
    let view = cx
        .update_window(h.window.into(), |_, window, cx| {
            settings::open(model, window, cx)
        })
        .unwrap();
    dialog_open(&h, cx).await;
    cx.update_window(h.window.into(), |_, window, cx| {
        view.update(cx, |v, cx| {
            v.max_concurrent
                .update(cx, |s, cx| s.set_value("0", window, cx))
        });
    })
    .unwrap();
    click(&h, cx, "ok");
    let still_open = cx
        .update_window(h.window.into(), |_, window, _| {
            window.try_find("dialog").is_some()
        })
        .unwrap();
    assert!(still_open);
    assert!(
        cx.read_entity(&view, |v, _| v.error.clone())
            .unwrap()
            .contains("Simultaneous")
    );

    cx.update_window(h.window.into(), |_, window, cx| {
        view.update(cx, |v, cx| {
            v.max_concurrent
                .update(cx, |s, cx| s.set_value("4", window, cx));
            v.set_show_hidden(true, cx);
        });
    })
    .unwrap();
    click(&h, cx, "ok");
    h.wait_state(cx, "the settings", |s| {
        s.settings.transfers.max_concurrent == 4 && s.settings.ui.show_hidden
    })
    .await;
}

#[gpui_kit::test]
async fn a_notice_becomes_a_notification_and_closing_it_dismisses_the_notice(
    cx: &mut TestAppContext,
) {
    let h = support::open(cx);
    // transfers without a session only raise a notice
    h.app.send(Command::Upload {
        names: vec!["x".into()],
    });
    h.wait_state(cx, "the notice", |s| !s.notices.is_empty())
        .await;
    cx.wait_for(h.window.into(), Duration::from_secs(5), |w, _| {
        w.try_find("notification").is_some()
    })
    .await;
    cx.update_window(h.window.into(), |_, window, cx| {
        window.render_frame(cx);
        // the close button only shows while the pointer is over the notification
        window.hover("notification", cx);
        window.render_frame(cx);
        window.within("notification").click("close", cx);
    })
    .unwrap();
    h.wait_state(cx, "the notice to be dismissed", |s| s.notices.is_empty())
        .await;
}
