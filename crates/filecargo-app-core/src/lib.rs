//! UI-agnostic application state and commands, shared by the TUI and the GUI.

mod app;
mod command;
mod logging;
mod session;
mod state;

pub use app::{App, AppHandle, StartOptions};
pub use command::Command;
pub use logging::{LogBuffer, LogLine};
pub use session::SessionFactory;
pub use state::{
    AppState, ConnectStep, Level, Notice, NoticeId, Pane, PaneId, Prompt, PromptAnswer, PromptId,
    PromptKind, SessionState, Sort, SortKey, StartError, TerminalState, TerminalView,
};
