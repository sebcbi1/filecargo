//! What a front-end renders: one immutable, cheap-to-clone snapshot of the whole application.

use std::path::PathBuf;
use std::sync::Arc;

use filecargo_config::{ServerTree, Settings, SiteId};
use filecargo_remote_fs::{
    CertificatePrompt, CredentialAnswer, CredentialPrompt, Entry, HostKeyPrompt, RemotePath,
    SessionInfo, TrustDecision,
};
use filecargo_terminal::TerminalHandle;
use filecargo_transfer::{ConflictDecision, ConflictInfo, QueueSnapshot, TransferId};

#[derive(Debug, Clone)]
pub struct AppState {
    /// A config file could not be read (corrupt, or written by a newer version). The tree is
    /// empty and the UI offers `Command::ResetConfig`.
    pub startup_error: Option<String>,
    pub servers: Arc<ServerTree>,
    pub settings: Arc<Settings>,
    pub session: SessionState,
    pub local: Pane<PathBuf>,
    /// `Some` while connected.
    pub remote: Option<Pane<RemotePath>>,
    pub queue: Arc<QueueSnapshot>,
    /// The site the bottom panel is about: the connected one, `None` when not connected.
    pub scope: Option<SiteId>,
    /// `queue` reduced to `scope`'s items (empty without a scope). What the Queue, Completed and
    /// Failed lists show.
    pub site_queue: Arc<QueueSnapshot>,
    /// Transfers running for other sites than `scope`; they keep going in the background.
    pub other_sites_active: usize,
    pub terminal: TerminalState,
    /// The one prompt to show now; the rest wait behind it in order.
    pub prompt: Option<Prompt>,
    /// Transient toasts; the UI dismisses them.
    pub notices: Vec<Notice>,
    /// Bumps when new log lines arrive.
    pub log_generation: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SessionState {
    Disconnected,
    Connecting { site: SiteId, step: ConnectStep },
    Connected { site: SiteId, info: SessionInfo },
    Failed { site: SiteId, error: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectStep {
    Resolving,
    Connecting,
    Authenticating,
    Listing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Size,
    Modified,
}

/// Directories always come first; `ascending` applies within each group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sort {
    pub key: SortKey,
    pub ascending: bool,
}

impl Default for Sort {
    fn default() -> Self {
        Self {
            key: SortKey::Name,
            ascending: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Pane<P> {
    pub path: P,
    /// Sorted and filtered; `..` is **not** included (the UI adds its own).
    pub entries: Arc<[Entry]>,
    pub sort: Sort,
    pub loading: bool,
    /// The last listing failed; the previous entries are kept.
    pub error: Option<String>,
    /// Bumps on every new listing; the UI resets its cursor when it changes.
    pub generation: u64,
}

impl<P> Pane<P> {
    pub(crate) fn new(path: P) -> Self {
        Self {
            path,
            entries: Arc::from(Vec::new()),
            sort: Sort::default(),
            loading: false,
            error: None,
            generation: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PaneId {
    Local,
    Remote,
}

#[derive(Clone)]
pub enum TerminalState {
    /// FTP sessions and no session at all.
    NotAvailable,
    Closed,
    Open(TerminalView),
    /// The shell ended; the screen stays readable through `view`. The UI offers "reopen".
    Exited {
        code: Option<u32>,
        view: TerminalView,
    },
}

impl std::fmt::Debug for TerminalState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAvailable => f.write_str("NotAvailable"),
            Self::Closed => f.write_str("Closed"),
            Self::Open(_) => f.write_str("Open(..)"),
            Self::Exited { code, .. } => write!(f, "Exited({code:?})"),
        }
    }
}

/// Draws and types into the shell: the emulator handle, shared with the app.
#[derive(Clone)]
pub struct TerminalView {
    pub handle: TerminalHandle,
}

// ---- prompts --------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PromptId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NoticeId(pub u64);

#[derive(Debug, Clone)]
pub struct Prompt {
    pub id: PromptId,
    pub kind: PromptKind,
}

#[derive(Debug, Clone)]
pub enum PromptKind {
    Credential(CredentialPrompt),
    HostKey(HostKeyPrompt),
    Certificate(CertificatePrompt),
    Conflict {
        transfer: TransferId,
        conflict: ConflictInfo,
    },
    ConfirmDelete {
        pane: PaneId,
        names: Vec<String>,
        recursive: bool,
    },
    ConfirmQuit {
        active_transfers: usize,
    },
    /// e.g. an import report or an error the user must read.
    Message {
        level: Level,
        title: String,
        body: String,
    },
}

pub enum PromptAnswer {
    /// `None` cancels.
    Credential(Option<CredentialAnswer>),
    Trust(TrustDecision),
    Conflict(ConflictDecision),
    Confirm(bool),
    Dismiss,
}

impl std::fmt::Debug for PromptAnswer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Credential(answer) => write!(
                f,
                "Credential({})",
                if answer.is_some() {
                    "answered"
                } else {
                    "cancelled"
                }
            ),
            Self::Trust(decision) => write!(f, "Trust({decision:?})"),
            Self::Conflict(decision) => write!(f, "Conflict({decision:?})"),
            Self::Confirm(yes) => write!(f, "Confirm({yes})"),
            Self::Dismiss => f.write_str("Dismiss"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub id: NoticeId,
    pub level: Level,
    pub text: String,
}

/// Why `App::start` failed. Config problems are **not** start errors: they become
/// [`AppState::startup_error`].
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error("cannot start the async runtime: {0}")]
    Runtime(String),
}
