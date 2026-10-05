#![cfg(feature = "integration")]
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Checkpoint B, automated: the whole TUI (key handling, rendering, the real app core and a real
//! SFTP server from tests/docker) driven by keystrokes, without a terminal. The screen is
//! drawn into a `TestBackend` after every step and asserted on as text.

use std::sync::Arc;
use std::time::{Duration, Instant};

use filecargo_app_core::prelude::*;
use filecargo_config::MemoryStore;
use filecargo_tui::reducer::{self, Event};
use filecargo_tui::ui_state::{Focus, UiState};
use filecargo_tui::view;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

const SIZE: (u16, u16) = (120, 36);

struct Driver {
    app: AppHandle,
    ui: UiState,
    terminal: Terminal<TestBackend>,
    _config: tempfile::TempDir,
}

impl Driver {
    fn start() -> Self {
        let config = tempfile::tempdir().unwrap();
        let app = App::start(StartOptions {
            paths: Some(Paths::from_override(Some(config.path().to_path_buf()))),
            secrets: Some(Arc::new(MemoryStore::new())),
            connector: None,
        })
        .unwrap();
        Self {
            app,
            ui: UiState::new(SIZE.0, SIZE.1),
            terminal: Terminal::new(TestBackend::new(SIZE.0, SIZE.1)).unwrap(),
            _config: config,
        }
    }

    fn state(&self) -> Arc<AppState> {
        self.app.state().borrow().clone()
    }

    /// One loop iteration: sync, the housekeeping commands, then the screen as text.
    fn screen(&mut self) -> String {
        let state = self.state();
        reducer::sync(&mut self.ui, &state);
        for command in reducer::housekeeping(&mut self.ui, &state) {
            self.app.send(command);
        }
        let ui = &self.ui;
        self.terminal
            .draw(|frame| view::render(frame, ui, &state))
            .unwrap();
        self.terminal.backend().to_string()
    }

    fn send(&mut self, event: Event) {
        let state = self.state();
        reducer::sync(&mut self.ui, &state);
        for command in reducer::on_event(&mut self.ui, &state, event) {
            self.app.send(command);
        }
    }

    fn press(&mut self, code: KeyCode) {
        self.send(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
    }

    fn press_with(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        self.send(Event::Key(KeyEvent::new(code, modifiers)));
    }

    fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            self.press(KeyCode::Char(c));
        }
    }

    fn tab(&mut self, times: usize) {
        for _ in 0..times {
            self.press(KeyCode::Tab);
        }
    }

    /// Redraws until `predicate` accepts the screen; panics with the screen after 20 s.
    fn wait(&mut self, what: &str, predicate: impl Fn(&str, &AppState) -> bool) -> String {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let screen = self.screen();
            if predicate(&screen, &self.state()) {
                return screen;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}; the screen shows:\n{screen}"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// Moves the cursor of the focused pane onto the entry called `name`.
    fn cursor_to(&mut self, name: &str) {
        let focus = self.ui.focus;
        let state = self.state();
        let pane = match focus {
            Focus::Local => &state.local.entries,
            Focus::Remote => &state.remote.as_ref().unwrap().entries,
            other => panic!("no pane focused: {other:?}"),
        };
        let row = pane.iter().position(|e| e.name == name).unwrap() + 1; // row 0 is `..`
        self.press(KeyCode::Home);
        for _ in 0..row {
            self.press(KeyCode::Down);
        }
    }
}

#[test]
fn create_a_site_connect_trust_the_key_browse_transfer_rename_delete_and_use_the_shell() {
    let local = tempfile::tempdir().unwrap();
    std::fs::write(local.path().join("hello.txt"), "hello from the tui").unwrap();
    let remote_dir = format!("fc-e2e-{}", std::process::id());

    let mut d = Driver::start();
    let screen = d.screen();
    assert!(screen.contains("No servers yet"), "{screen}");

    // ---- new site -----------------------------------------------------------------------
    d.ui.focus = Focus::Tree;
    d.press(KeyCode::Char('n'));
    d.type_text("docker");
    d.tab(2);
    d.type_text("127.0.0.1");
    d.tab(1);
    d.type_text("2222");
    d.tab(1);
    d.type_text("fcuser");
    d.tab(2); // login (Password) -> password
    d.type_text("fcpass");
    d.tab(1);
    d.press(KeyCode::Char(' ')); // remember the password
    d.tab(2); // remote dir -> local dir (FTP mode is hidden for SFTP)
    d.type_text(&local.path().display().to_string());
    let editor = d.screen();
    assert!(
        editor.contains("New site") && editor.contains("•••••"),
        "{editor}"
    );
    assert!(!editor.contains("fcpass"), "the password must not be drawn");
    d.press(KeyCode::Enter);
    let screen = d.wait("the site in the tree", |s, _| {
        s.contains("docker") && !s.contains("New site")
    });
    assert!(!screen.contains("No servers yet"));

    // ---- connect, accept the host key -----------------------------------------------------
    d.press(KeyCode::Enter);
    let screen = d.wait("the host key prompt", |s, _| s.contains("Unknown host key"));
    assert!(
        screen.contains("ssh-ed25519") || screen.contains("ssh-rsa") || screen.contains("ecdsa"),
        "{screen}"
    );
    d.press(KeyCode::Enter); // no default key: nothing happens
    assert!(
        d.screen().contains("Unknown host key"),
        "Enter must not answer a trust prompt"
    );
    d.press(KeyCode::Char('y'));
    let screen = d.wait("the remote listing", |s, st| {
        s.contains("Remote sftp://127.0.0.1")
            && st.remote.is_some()
            && !s.contains("Unknown host key")
    });
    assert!(
        screen.contains("hello.txt"),
        "the local pane shows the site's local dir:\n{screen}"
    );
    assert_eq!(
        d.ui.focus,
        Focus::Remote,
        "the focus follows the connection"
    );

    // ---- new remote folder ------------------------------------------------------------------
    d.press(KeyCode::F(7));
    d.type_text(&remote_dir);
    d.press(KeyCode::Enter);
    d.wait("the new remote folder", |s, _| {
        s.contains(&format!("{remote_dir}/"))
    });
    d.cursor_to(&remote_dir);
    d.press(KeyCode::Enter); // open it
    let inside = d.wait("to be inside the folder", |_, st| {
        st.remote
            .as_ref()
            .is_some_and(|r| r.path.to_string().ends_with(&remote_dir) && !r.loading)
    });
    assert!(inside.contains(&remote_dir));

    // ---- upload ------------------------------------------------------------------------------
    d.ui.focus = Focus::Local;
    d.cursor_to("hello.txt");
    d.press(KeyCode::F(5));
    d.wait("the upload to finish", |_, st| {
        st.queue
            .completed
            .iter()
            .any(|c| c.item.local.ends_with("hello.txt"))
    });
    d.wait("the file in the remote pane", |s, st| {
        s.contains("hello.txt")
            && st
                .remote
                .as_ref()
                .is_some_and(|r| r.entries.iter().any(|e| e.name == "hello.txt"))
    });
    d.press_with(KeyCode::Char('2'), KeyModifiers::ALT);
    let completed = d.wait("the completed tab", |s, _| s.contains("Result"));
    assert!(
        completed.contains("hello.txt") && completed.contains("done"),
        "{completed}"
    );

    // ---- rename ------------------------------------------------------------------------------
    d.ui.focus = Focus::Remote;
    d.cursor_to("hello.txt");
    d.press(KeyCode::F(2));
    d.press_with(KeyCode::Char('u'), KeyModifiers::CONTROL);
    d.type_text("renamed.txt");
    d.press(KeyCode::Enter);
    d.wait("the rename", |_, st| {
        st.remote.as_ref().is_some_and(|r| {
            r.entries.iter().any(|e| e.name == "renamed.txt")
                && !r.entries.iter().any(|e| e.name == "hello.txt")
        })
    });

    // ---- download it back under the same name next to the original ------------------------------
    d.cursor_to("renamed.txt");
    d.press(KeyCode::F(5));
    d.wait("the download", |_, st| {
        st.queue
            .completed
            .iter()
            .any(|c| c.item.local.ends_with("renamed.txt"))
    });
    assert_eq!(
        std::fs::read(local.path().join("renamed.txt")).unwrap(),
        b"hello from the tui"
    );

    // ---- delete with confirmation -----------------------------------------------------------
    d.cursor_to("renamed.txt");
    d.press(KeyCode::F(8));
    let screen = d.wait("the delete confirmation", |s, _| {
        s.contains("Delete \"renamed.txt\"?")
    });
    assert!(screen.contains("y / Enter delete"), "{screen}");
    d.press(KeyCode::Char('y'));
    d.wait("the file to be gone", |_, st| {
        st.remote
            .as_ref()
            .is_some_and(|r| !r.loading && r.entries.is_empty())
    });

    // ---- the shell --------------------------------------------------------------------------
    d.press_with(KeyCode::Char('5'), KeyModifiers::ALT);
    d.wait("the shell to open", |_, st| {
        matches!(st.terminal, TerminalState::Open(_))
    });
    d.type_text("echo fc-marker-$((6*7))");
    d.press(KeyCode::Enter);
    let screen = d.wait("the command output", |s, _| {
        s.lines()
            .any(|l| l.contains("fc-marker-42") && !l.contains("echo"))
    });
    assert!(screen.contains("Terminal"), "{screen}");
    d.press_with(KeyCode::Char('\\'), KeyModifiers::CONTROL);
    assert_eq!(d.ui.focus, Focus::Remote, "Ctrl-\\ leaves the terminal");

    // ---- clean up the remote folder and quit ------------------------------------------------------
    d.press(KeyCode::Backspace); // parent folder
    d.wait("the parent listing", |_, st| {
        st.remote
            .as_ref()
            .is_some_and(|r| r.entries.iter().any(|e| e.name == remote_dir))
    });
    d.cursor_to(&remote_dir);
    d.press(KeyCode::F(8));
    d.wait("the confirmation", |s, _| s.contains("Delete"));
    d.press(KeyCode::Char('y'));
    d.wait("the folder to be gone", |_, st| {
        st.remote
            .as_ref()
            .is_some_and(|r| !r.loading && !r.entries.iter().any(|e| e.name == remote_dir))
    });
    d.press(KeyCode::Char('q'));
    d.app.shutdown(Duration::from_secs(5));
}
