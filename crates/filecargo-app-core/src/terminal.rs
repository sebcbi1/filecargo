//! The shell tab: opens a shell on the SFTP session and exposes its emulator to the UI.

use filecargo_remote_fs::FsError;
use filecargo_terminal::{TermSize, TermStatus, spawn};

use crate::app::{Core, Msg};
use crate::command::Command;
use crate::logging::site_span;
use crate::state::{Level, TerminalState, TerminalView};

const SCROLLBACK: usize = 5_000;
const TERM: &str = "xterm-256color";

/// The result of opening a shell.
pub(crate) struct Opened {
    /// Which connection it was opened on, so a result from a replaced session is dropped.
    pub epoch: u64,
    pub size: TermSize,
    pub result: Result<filecargo_remote_fs::ShellChannel, FsError>,
}

impl Core {
    pub(crate) fn terminal_command(&mut self, command: Command) {
        match command {
            Command::TerminalOpen { cols, rows } => self.open_terminal(TermSize { cols, rows }),
            Command::TerminalInput(bytes) => {
                if let Some(handle) = &self.terminal {
                    handle.send_raw(bytes);
                }
            }
            Command::TerminalResize { cols, rows } => {
                if let Some(handle) = &self.terminal {
                    handle.resize(TermSize { cols, rows });
                }
            }
            Command::TerminalScroll(lines) => {
                if let Some(handle) = &self.terminal {
                    handle.scroll(lines);
                }
            }
            Command::TerminalClose => {
                if let Some(handle) = &self.terminal {
                    handle.close();
                }
            }
            _ => {}
        }
    }

    /// Opens the shell the first time the UI shows the tab (and again after it ended).
    fn open_terminal(&mut self, size: TermSize) {
        let reopen = matches!(
            self.state.terminal,
            TerminalState::Closed | TerminalState::Exited { .. }
        );
        let Some(shell) = self.shell.clone().filter(|_| reopen) else {
            return;
        };
        let epoch = self.connect_epoch;
        let messages = self.messages.clone();
        tokio::spawn(async move {
            let result = shell.open(TERM, size.cols, size.rows).await;
            let _ = messages.send(Msg::TerminalOpened(Opened {
                epoch,
                size,
                result,
            }));
        });
    }

    pub(crate) fn on_terminal_opened(&mut self, opened: Opened) {
        if opened.epoch != self.connect_epoch || self.live.is_none() {
            return; // the session was replaced or closed meanwhile
        }
        match opened.result {
            Ok(channel) => {
                let handle = spawn(channel, opened.size, SCROLLBACK);
                // learn when the shell ends without polling
                let watcher = handle.clone();
                let messages = self.messages.clone();
                let epoch = opened.epoch;
                tokio::spawn(async move {
                    let status = watcher.wait_ended().await;
                    let _ = messages.send(Msg::TerminalEnded { epoch, status });
                });
                self.terminal = Some(handle.clone());
                self.state.terminal = TerminalState::Open(TerminalView { handle });
                self.changed();
            }
            Err(error) => {
                let _span = self
                    .live
                    .as_ref()
                    .map(|live| site_span(live.site).entered());
                tracing::warn!(target: "filecargo::app", %error, "could not open a shell");
                self.notice(Level::Error, format!("Could not open a shell: {error}"));
            }
        }
    }

    pub(crate) fn on_terminal_ended(&mut self, epoch: u64, status: TermStatus) {
        if epoch != self.connect_epoch {
            return;
        }
        if let (TerminalState::Open(view), Some(_)) = (&self.state.terminal, &self.terminal) {
            let view = view.clone();
            self.state.terminal = match status {
                TermStatus::Exited(code) => TerminalState::Exited { code, view },
                _ => TerminalState::Closed,
            };
            self.terminal = None;
            self.changed();
        }
    }

    /// The shell belongs to the session: it goes away with it.
    pub(crate) fn close_terminal(&mut self) {
        if let Some(handle) = self.terminal.take() {
            handle.close();
        }
    }
}
