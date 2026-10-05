#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::sync::atomic::Ordering;
use std::time::Duration;

use filecargo_app_core::{AppState, Command, Protocol, TerminalState};
use filecargo_remote_fs::{ShellInput, ShellOutput};
use support::{FakeShell, Fixture, TestFactory, server_tree, site_for};

fn setup(
    protocol: Protocol,
) -> (
    Fixture,
    std::sync::Arc<FakeShell>,
    std::sync::Arc<TestFactory>,
    tempfile::TempDir,
) {
    let server = server_tree();
    let factory = TestFactory::new();
    factory.serve("host", server.path().to_path_buf(), None);
    let shell = FakeShell::new();
    *factory.shell.lock().unwrap() = Some(shell.clone());
    let fx = Fixture::with_factory(factory.clone());
    let mut site = site_for("s", "host");
    site.protocol = protocol;
    let id = fx.add_site(site);
    fx.app.send(Command::Connect(id));
    fx.wait_for("connected", |s| s.remote.is_some());
    (fx, shell, factory, server)
}

fn open_view(state: &AppState) -> Option<filecargo_app_core::TerminalView> {
    match &state.terminal {
        TerminalState::Open(view) => Some(view.clone()),
        _ => None,
    }
}

fn screen(view: &filecargo_app_core::TerminalView) -> String {
    view.handle.with_screen(|s| s.contents())
}

#[test]
fn sftp_sessions_offer_a_terminal_and_ftp_sessions_do_not() {
    let (fx, _shell, _factory, _server) = setup(Protocol::Sftp);
    assert!(matches!(fx.state().terminal, TerminalState::Closed));
    let (fx, _shell, _factory, _server) = setup(Protocol::Ftp);
    assert!(matches!(fx.state().terminal, TerminalState::NotAvailable));
    fx.app.send(Command::TerminalOpen { cols: 80, rows: 24 });
    std::thread::sleep(Duration::from_millis(150));
    assert!(
        matches!(fx.state().terminal, TerminalState::NotAvailable),
        "FTP has nothing to open"
    );
}

#[test]
fn opening_runs_a_shell_that_draws_receives_input_resizes_and_scrolls() {
    let (fx, shell, _factory, _server) = setup(Protocol::Sftp);
    fx.app.send(Command::TerminalOpen { cols: 80, rows: 24 });
    let state = fx.wait_for("the terminal", |s| open_view(s).is_some());
    let view = open_view(&state).unwrap();
    assert_eq!(
        shell.opened.lock().unwrap()[0],
        ("xterm-256color".to_owned(), 80, 24)
    );
    let mut remote = shell.take_remote();

    remote.say("\x1b[2J\x1b[1;1Hhello from the server");
    for _ in 0..100 {
        if screen(&view).starts_with("hello from the server") {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(screen(&view).starts_with("hello from the server"));

    fx.app.send(Command::TerminalInput(b"ls -l\r".to_vec()));
    fx.app.send(Command::TerminalResize {
        cols: 100,
        rows: 30,
    });
    std::thread::sleep(Duration::from_millis(200));
    let received = remote.received();
    assert!(
        received.contains(&ShellInput::Data(b"ls -l\r".to_vec())),
        "{received:?}"
    );
    assert!(
        received.contains(&ShellInput::Resize {
            cols: 100,
            rows: 30
        }),
        "{received:?}"
    );
    assert_eq!(view.handle.with_screen(|s| s.size()), (30, 100));

    let lines: String = (1..=100).map(|n| format!("line {n:03}\r\n")).collect();
    remote.say(&lines);
    std::thread::sleep(Duration::from_millis(200));
    fx.app.send(Command::TerminalScroll(20));
    std::thread::sleep(Duration::from_millis(100));
    assert!(view.handle.with_screen(|s| s.scrollback()) > 0);

    // opening again while open does nothing
    fx.app.send(Command::TerminalOpen { cols: 80, rows: 24 });
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(shell.opened(), 1);
}

#[test]
fn when_the_shell_exits_the_state_says_so_the_screen_stays_and_it_can_be_reopened() {
    let (fx, shell, _factory, _server) = setup(Protocol::Sftp);
    fx.app.send(Command::TerminalOpen { cols: 80, rows: 24 });
    fx.wait_for("the terminal", |s| open_view(s).is_some());
    let remote = shell.take_remote();
    remote.say("goodbye");
    remote.output.send(ShellOutput::Exit(Some(0))).unwrap();

    let state = fx.wait_for("the exit", |s| {
        matches!(s.terminal, TerminalState::Exited { .. })
    });
    match &state.terminal {
        TerminalState::Exited { code, view } => {
            assert_eq!(*code, Some(0));
            assert!(
                screen(view).starts_with("goodbye"),
                "the screen stays readable"
            );
        }
        other => panic!("{other:?}"),
    }
    fx.app.send(Command::TerminalOpen { cols: 80, rows: 24 });
    fx.wait_for("a fresh shell", |s| open_view(s).is_some());
    assert_eq!(shell.opened(), 2);
}

#[test]
fn disconnecting_closes_the_shell_and_removes_the_terminal() {
    let (fx, shell, _factory, _server) = setup(Protocol::Sftp);
    fx.app.send(Command::TerminalOpen { cols: 80, rows: 24 });
    fx.wait_for("the terminal", |s| open_view(s).is_some());
    let mut remote = shell.take_remote();
    fx.app.send(Command::Disconnect);
    fx.wait_for("disconnected", |s| {
        matches!(s.terminal, TerminalState::NotAvailable)
    });
    std::thread::sleep(Duration::from_millis(150));
    assert!(remote.received().contains(&ShellInput::Close));
}

#[test]
fn a_shell_that_cannot_be_opened_raises_a_notice_and_the_tab_stays_closed() {
    let (fx, shell, _factory, _server) = setup(Protocol::Sftp);
    shell.fail_next.store(true, Ordering::SeqCst);
    fx.app.send(Command::TerminalOpen { cols: 80, rows: 24 });
    let state = fx.wait_for("a notice", |s| !s.notices.is_empty());
    assert!(
        state.notices[0].text.contains("Could not open a shell"),
        "{}",
        state.notices[0].text
    );
    assert!(matches!(state.terminal, TerminalState::Closed));
    // and a later attempt works
    fx.app.send(Command::TerminalOpen { cols: 80, rows: 24 });
    fx.wait_for("the terminal", |s| open_view(s).is_some());
}
