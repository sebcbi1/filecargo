//! Everything a front-end needs, in one place: front-ends depend **only** on this crate. What
//! they render or build from the lower crates is re-exported here, so churn below stays below.

pub use crate::app::{App, AppHandle, StartOptions};
pub use crate::command::Command;
pub use crate::logging::{LogBuffer, LogLayer, LogLine, init as init_logging};
pub use crate::session::SessionFactory;
pub use crate::state::{
    AppState, ConnectStep, Level, Notice, NoticeId, Pane, PaneId, Prompt, PromptAnswer, PromptId,
    PromptKind, SessionState, Sort, SortKey, StartError, TerminalState, TerminalView,
};

// config
pub use filecargo_config::{
    Auth, ConflictRule, ConnectionSettings, Folder, FolderId, FtpMode, LogLevel, LogSettings, Node,
    NodeId, Paths, Protocol, SecretString, ServerTree, Settings, Site, SiteId, TransferSettings,
    TreeOp, UiSettings,
};
// remote-fs
pub use filecargo_remote_fs::{
    CertificateProblem, CertificatePrompt, CredentialAnswer, CredentialPrompt, Entry, EntryKind,
    HostKeyPrompt, RemotePath, SessionInfo, TrustDecision,
};
// transfer
pub use filecargo_transfer::{
    ConflictDecision, ConflictInfo, Direction, ItemState, Outcome, QueueItem, QueueItemView,
    QueueSnapshot, Totals, TransferId,
};
// terminal
pub use filecargo_terminal::{
    Key, Modes, Mods, Screen, TermSize, TermStatus, TerminalHandle, encode,
};
