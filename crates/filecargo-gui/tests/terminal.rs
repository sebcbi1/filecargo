#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::time::Duration;

use filecargo_app_core::prelude::*;
use filecargo_remote_fs::ShellInput;
use gpui_kit::Focusable as _;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AppContext as _, TestAppContext};
use support::Harness;
use support::factory::TestFactory;
use support::terminal::{FakeShell, ShellRemote};

struct Env {
    h: Harness,
    remote: ShellRemote,
    _server: tempfile::TempDir,
}

/// Connected over (fake) SFTP with the terminal tab showing and its shell open.
async fn env(cx: &mut TestAppContext) -> Env {
    let server = tempfile::tempdir().unwrap();
    let factory = TestFactory::new();
    factory.serve("h.example.org", server.path().to_path_buf(), None);
    let shell = FakeShell::new();
    *factory.shell.lock().unwrap() = Some(shell.clone());
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
    let bottom = cx.read_entity(&h.workspace, |w, _| w.bottom.clone());
    bottom.update(cx, |b, cx| b.select_tab(4, cx));
    cx.update_window(h.window.into(), |_, window, cx| {
        // the first frame requests the shell; focus the terminal like a click on it would
        window.render_frame(cx);
        let terminal = bottom.read(cx).terminal.clone();
        terminal.read(cx).focus_handle(cx).focus(window, cx);
    })
    .unwrap();
    h.wait_state(cx, "the shell", |s| {
        matches!(s.terminal, TerminalState::Open(_))
    })
    .await;
    cx.update_window(h.window.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
    let remote = shell.take_remote();
    Env {
        h,
        remote,
        _server: server,
    }
}

fn handle(cx: &mut TestAppContext, h: &Harness) -> TerminalHandle {
    cx.read_entity(&h.model, |m, _| match &m.state.terminal {
        TerminalState::Open(view) => view.handle.clone(),
        other => panic!("{other:?}"),
    })
}

/// Everything the shell has been sent since the last call, as `Data` bytes and resizes.
fn sent(env: &mut Env) -> (Vec<u8>, Vec<(u16, u16)>) {
    std::thread::sleep(Duration::from_millis(150));
    let (mut bytes, mut sizes) = (Vec::new(), Vec::new());
    for message in env.remote.received() {
        match message {
            ShellInput::Data(data) => bytes.extend(data),
            ShellInput::Resize { cols, rows } => sizes.push((cols, rows)),
            ShellInput::Close => {}
        }
    }
    (bytes, sizes)
}

fn press(cx: &mut TestAppContext, h: &Harness, key: &'static str) {
    cx.update_window(h.window.into(), |_, window, cx| window.press(key, cx))
        .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
async fn the_tab_opens_the_shell_and_the_painted_size_reaches_it(cx: &mut TestAppContext) {
    let mut e = env(cx).await;
    let (_, sizes) = sent(&mut e);
    let (cols, rows) = *sizes.last().expect("the first paint sends the grid size");
    assert!(cols > 10 && rows > 1, "{cols}x{rows}");
}

#[gpui_kit::test]
async fn typed_keys_reach_the_shell_encoded_as_xterm_expects(cx: &mut TestAppContext) {
    let mut e = env(cx).await;
    sent(&mut e);
    press(cx, &e.h, "up");
    press(cx, &e.h, "ctrl-c");
    press(cx, &e.h, "tab");
    press(cx, &e.h, "x");
    let (bytes, _) = sent(&mut e);
    assert_eq!(bytes, b"\x1b[A\x03\tx");
}

#[gpui_kit::test]
async fn global_shortcuts_do_not_steal_keys_from_the_shell(cx: &mut TestAppContext) {
    let mut e = env(cx).await;
    sent(&mut e);
    // Ctrl-Q would quit the application anywhere else
    press(cx, &e.h, "ctrl-q");
    let (bytes, _) = sent(&mut e);
    assert_eq!(bytes, [0x11], "Ctrl-Q reached the shell");
    assert!(cx.read_entity(&e.h.model, |m, _| !matches!(
        m.state.terminal,
        TerminalState::Closed
    )));
}

#[gpui_kit::test]
async fn output_is_drawn_without_trouble_and_lands_in_the_emulator(cx: &mut TestAppContext) {
    let e = env(cx).await;
    e.remote
        .say("\x1b[1;32muser@host\x1b[0m:~$ 日本語 \x1b[7mdone\x1b[0m\r\n");
    let term = handle(cx, &e.h);
    for _ in 0..200 {
        if term.with_screen(|s| s.contents().contains("done")) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(term.with_screen(|s| s.contents().contains("user@host:~$ 日本語")));
    // a frame with colours, a wide-character segment and a cursor must not panic
    for _ in 0..3 {
        cx.update_window(e.h.window.into(), |_, window, cx| window.render_frame(cx))
            .unwrap();
    }
}

#[gpui_kit::test]
async fn an_exited_shell_offers_to_reopen_on_enter(cx: &mut TestAppContext) {
    let mut e = env(cx).await;
    sent(&mut e);
    e.remote
        .output
        .send(filecargo_remote_fs::ShellOutput::Exit(Some(0)))
        .unwrap();
    e.remote
        .output
        .send(filecargo_remote_fs::ShellOutput::Closed)
        .unwrap();
    e.h.wait_state(cx, "the exit", |s| {
        matches!(s.terminal, TerminalState::Exited { .. })
    })
    .await;
    cx.update_window(e.h.window.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
    press(cx, &e.h, "enter");
    e.h.wait_state(cx, "the shell to reopen", |s| {
        matches!(s.terminal, TerminalState::Open(_))
    })
    .await;
}
