#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use filecargo_app_core::prelude::*;
use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{AppContext as _, TestAppContext};
use support::Harness;
use support::factory::TestFactory;

struct Env {
    h: Harness,
    factory: Arc<TestFactory>,
    server: tempfile::TempDir,
    local: tempfile::TempDir,
    site: SiteId,
}

/// An app with one site `prod` served from `server` (holding `a.txt`), whose local folder is
/// `local` (holding `a.txt`, different content).
async fn env(cx: &mut TestAppContext, password: Option<&str>) -> Env {
    let server = tempfile::tempdir().unwrap();
    std::fs::write(server.path().join("a.txt"), "old").unwrap();
    let local = tempfile::tempdir().unwrap();
    std::fs::write(local.path().join("a.txt"), "new content").unwrap();
    let factory = TestFactory::new();
    factory.serve("prod.example.org", server.path().to_path_buf(), password);
    let f = factory.clone();
    let h = support::open_with(cx, move |o| o.connector = Some(f));
    let mut site = Site::new("prod", Protocol::Sftp, "prod.example.org");
    site.user = "me".into();
    site.local_dir = Some(local.path().to_path_buf());
    let id = site.id;
    h.app.send(Command::Tree(TreeOp::AddSite(site)));
    h.wait_state(cx, "the site", |s| s.servers.sites().len() == 1)
        .await;
    Env {
        h,
        factory,
        server,
        local,
        site: id,
    }
}

fn connected(s: &AppState) -> bool {
    matches!(s.session, SessionState::Connected { .. }) && s.remote.is_some()
}

async fn dialog_open(h: &Harness, cx: &mut TestAppContext) {
    cx.wait_for(h.window.into(), Duration::from_secs(5), |w, _| {
        w.try_find("dialog").is_some()
    })
    .await;
    cx.run_until_parked();
}

async fn dialog_closed(h: &Harness, cx: &mut TestAppContext) {
    cx.wait_for(h.window.into(), Duration::from_secs(5), |w, _| {
        w.try_find("dialog").is_none()
    })
    .await;
}

/// Presses a button of the open dialog.
fn press(h: &Harness, cx: &mut TestAppContext, id: &'static str) {
    cx.update_window(h.window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.within("dialog").click(id, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

async fn connect(e: &Env, _cx: &mut TestAppContext) {
    e.h.app.send(Command::Connect(e.site));
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

#[gpui_kit::test]
async fn a_password_prompt_takes_typed_text_and_answers(cx: &mut TestAppContext) {
    let e = env(cx, Some("pw")).await;
    connect(&e, cx).await;
    dialog_open(&e.h, cx).await;
    cx.update_window(e.h.window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.within("dialog").click("credential-0", cx);
        window.input("pw", cx);
    })
    .unwrap();
    press(&e.h, cx, "ok");
    e.h.wait_state(cx, "the session", connected).await;
    dialog_closed(&e.h, cx).await;
}

#[gpui_kit::test]
async fn cancelling_a_password_prompt_cancels_the_connection(cx: &mut TestAppContext) {
    let e = env(cx, Some("pw")).await;
    connect(&e, cx).await;
    dialog_open(&e.h, cx).await;
    press(&e.h, cx, "cancel");
    e.h.wait_state(cx, "the prompt to go", |s| s.prompt.is_none())
        .await;
    cx.run_until_parked();
    assert!(!cx.read_entity(&e.h.model, |m, _| connected(&m.state)));
}

fn host_key() -> filecargo_remote_fs::HostKeyPrompt {
    filecargo_remote_fs::HostKeyPrompt {
        host: "prod.example.org".into(),
        port: 22,
        algorithm: "ssh-ed25519".into(),
        fingerprint: "SHA256:abc".into(),
    }
}

#[gpui_kit::test]
async fn a_host_key_prompt_offers_three_answers_and_trust_once_connects(cx: &mut TestAppContext) {
    let e = env(cx, None).await;
    *e.factory.host_key.lock().unwrap() = Some(host_key());
    connect(&e, cx).await;
    dialog_open(&e.h, cx).await;
    for id in ["reject", "once", "always"] {
        let found = cx
            .update_window(e.h.window.into(), |_, window, _| {
                window.within("dialog").try_find(id).is_some()
            })
            .unwrap();
        assert!(found, "button {id}");
    }
    press(&e.h, cx, "once");
    e.h.wait_state(cx, "the session", connected).await;
}

#[gpui_kit::test]
async fn rejecting_a_host_key_fails_the_connection(cx: &mut TestAppContext) {
    let e = env(cx, None).await;
    *e.factory.host_key.lock().unwrap() = Some(host_key());
    connect(&e, cx).await;
    dialog_open(&e.h, cx).await;
    press(&e.h, cx, "reject");
    e.h.wait_state(cx, "the failure", |s| {
        matches!(s.session, SessionState::Failed { .. })
    })
    .await;
}

#[gpui_kit::test]
async fn a_certificate_prompt_trusts_always(cx: &mut TestAppContext) {
    let e = env(cx, None).await;
    *e.factory.certificate.lock().unwrap() = Some(filecargo_remote_fs::CertificatePrompt {
        host: "prod.example.org".into(),
        port: 21,
        problem: CertificateProblem::SelfSigned,
        subject: "CN=prod".into(),
        issuer: "CN=prod".into(),
        not_after: "2027-01-01 00:00:00 UTC".into(),
        sha256: "00ff".into(),
    });
    connect(&e, cx).await;
    dialog_open(&e.h, cx).await;
    press(&e.h, cx, "always");
    e.h.wait_state(cx, "the session", connected).await;
}

#[gpui_kit::test]
async fn a_conflict_prompt_compares_both_files_and_the_rule_reaches_the_queue(
    cx: &mut TestAppContext,
) {
    let e = env(cx, None).await;
    connect(&e, cx).await;
    e.h.wait_state(cx, "the session", connected).await;
    e.h.wait_state(cx, "the local folder", {
        let local = support::canonical(e.local.path());
        move |s| s.local.path == local && !s.local.entries.is_empty()
    })
    .await;
    e.h.app.send(Command::Upload {
        names: vec!["a.txt".into()],
    });
    e.h.wait_state(cx, "the conflict", |s| {
        matches!(
            s.prompt.as_ref().map(|p| &p.kind),
            Some(PromptKind::Conflict { .. })
        )
    })
    .await;
    dialog_open(&e.h, cx).await;
    assert_eq!(
        read(&e.server.path().join("a.txt")),
        "old",
        "nothing is written before the answer"
    );
    press(&e.h, cx, "overwrite");
    for _ in 0..200 {
        if read(&e.server.path().join("a.txt")) == "new content" {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("the file was not overwritten");
}

#[gpui_kit::test]
async fn skipping_in_a_conflict_prompt_leaves_the_file(cx: &mut TestAppContext) {
    let e = env(cx, None).await;
    connect(&e, cx).await;
    e.h.wait_state(cx, "the session", connected).await;
    e.h.wait_state(cx, "the local folder", {
        let local = support::canonical(e.local.path());
        move |s| s.local.path == local && !s.local.entries.is_empty()
    })
    .await;
    e.h.app.send(Command::Upload {
        names: vec!["a.txt".into()],
    });
    dialog_open(&e.h, cx).await;
    press(&e.h, cx, "skip");
    e.h.wait_state(cx, "the item to finish", |s| {
        s.queue.pending.is_empty() && !s.queue.completed.is_empty()
    })
    .await;
    assert_eq!(read(&e.server.path().join("a.txt")), "old");
}

#[gpui_kit::test]
async fn confirm_delete_asks_and_only_then_deletes(cx: &mut TestAppContext) {
    let e = env(cx, None).await;
    connect(&e, cx).await;
    e.h.wait_state(cx, "the session", connected).await;
    e.h.app.send(Command::Delete {
        pane: PaneId::Remote,
        names: vec!["a.txt".into()],
    });
    dialog_open(&e.h, cx).await;
    assert!(e.server.path().join("a.txt").exists());
    press(&e.h, cx, "cancel");
    e.h.wait_state(cx, "the prompt to go", |s| s.prompt.is_none())
        .await;
    assert!(
        e.server.path().join("a.txt").exists(),
        "declining deletes nothing"
    );
    e.h.app.send(Command::Delete {
        pane: PaneId::Remote,
        names: vec!["a.txt".into()],
    });
    dialog_open(&e.h, cx).await;
    press(&e.h, cx, "ok");
    for _ in 0..200 {
        if !e.server.path().join("a.txt").exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("the file was not deleted");
}

#[gpui_kit::test]
async fn confirm_quit_waits_for_the_answer_while_a_transfer_runs(cx: &mut TestAppContext) {
    let e = env(cx, None).await;
    *e.factory.transfer_delay.lock().unwrap() = Duration::from_secs(60);
    connect(&e, cx).await;
    e.h.wait_state(cx, "the session", connected).await;
    e.h.wait_state(cx, "the local folder", {
        let local = support::canonical(e.local.path());
        move |s| s.local.path == local && !s.local.entries.is_empty()
    })
    .await;
    e.h.app.send(Command::Upload {
        names: vec!["a.txt".into()],
    });
    // the conflict comes first: skip it by overwriting, which then sits in the slow transfer
    dialog_open(&e.h, cx).await;
    press(&e.h, cx, "overwrite");
    e.h.wait_state(cx, "an active transfer", |s| {
        s.queue
            .pending
            .iter()
            .any(|i| matches!(i.item.state, ItemState::Active { .. }))
    })
    .await;
    e.h.app.send(Command::Quit);
    e.h.wait_state(cx, "the quit question", |s| {
        matches!(
            s.prompt.as_ref().map(|p| &p.kind),
            Some(PromptKind::ConfirmQuit { .. })
        )
    })
    .await;
    dialog_open(&e.h, cx).await;
    press(&e.h, cx, "cancel");
    e.h.wait_state(cx, "the prompt to go", |s| s.prompt.is_none())
        .await;
    assert!(
        cx.read_entity(&e.h.model, |m, _| connected(&m.state)),
        "declining keeps the app running"
    );
}

#[gpui_kit::test]
async fn a_message_prompt_is_dismissed_with_ok(cx: &mut TestAppContext) {
    let e = env(cx, None).await;
    // the same name twice is refused by the tree and reported as a message
    let mut again = Site::new("prod", Protocol::Sftp, "other.example.org");
    again.user = "me".into();
    e.h.app.send(Command::Tree(TreeOp::AddSite(again)));
    e.h.wait_state(cx, "the message", |s| {
        matches!(
            s.prompt.as_ref().map(|p| &p.kind),
            Some(PromptKind::Message { .. })
        )
    })
    .await;
    dialog_open(&e.h, cx).await;
    press(&e.h, cx, "ok");
    e.h.wait_state(cx, "the prompt to go", |s| s.prompt.is_none())
        .await;
}
