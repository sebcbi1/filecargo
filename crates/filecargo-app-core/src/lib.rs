//! UI-agnostic application state and commands, shared by the TUI and the GUI.

mod app;
mod command;
mod logging;
mod ops;
mod pane;
mod prompt;
mod session;
mod sort;
mod state;
mod transfers;
mod tree;

pub use app::{App, AppHandle, StartOptions};
pub use command::Command;
pub use filecargo_remote_fs::Entry;
pub use logging::{LogBuffer, LogLayer, LogLine, init as init_logging};
pub use session::SessionFactory;
pub use state::{
    AppState, ConnectStep, Level, Notice, NoticeId, Pane, PaneId, Prompt, PromptAnswer, PromptId,
    PromptKind, SessionState, Sort, SortKey, StartError, TerminalState, TerminalView,
};
