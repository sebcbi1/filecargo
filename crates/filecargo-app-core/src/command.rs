//! What a front-end can ask for.

use std::path::PathBuf;

use filecargo_config::{SecretString, TreeOp};
use filecargo_config::{Settings, SiteId};
use filecargo_transfer::TransferId;

use crate::state::{NoticeId, PaneId, PromptAnswer, PromptId, Sort};

#[derive(Debug)]
pub enum Command {
    // ---- server tree (thin wrappers over `ConfigStore`; errors become a `Message` prompt) ----
    Tree(TreeOp),
    ImportFileZilla {
        path: PathBuf,
        import_passwords: bool,
    },
    /// Backs up an unreadable config file and reloads.
    ResetConfig,
    /// From the site editor: stores the password in the keychain.
    SetSitePassword {
        site: SiteId,
        secret: SecretString,
    },

    // ---- session ----
    Connect(SiteId),
    Disconnect,

    // ---- panes ----
    /// Absolute, or relative to the pane's path; `~` is the home directory on the local pane.
    Navigate {
        pane: PaneId,
        path: String,
    },
    Up(PaneId),
    Refresh(PaneId),
    SetSort {
        pane: PaneId,
        sort: Sort,
    },

    // ---- remote file operations (names are entries of the remote pane's directory) ----
    Mkdir {
        name: String,
    },
    Rename {
        from: String,
        to: String,
    },
    /// Asks first when `settings.ui.confirm_delete`.
    Delete {
        names: Vec<String>,
    },
    Chmod {
        names: Vec<String>,
        mode: u32,
    },

    // ---- transfers (names are entries of the *source* pane's directory) ----
    Upload {
        names: Vec<String>,
    },
    Download {
        names: Vec<String>,
    },
    QueueRetry(TransferId),
    QueueRetryFailed,
    QueueRemove(TransferId),
    QueueClearCompleted,
    QueueSetProcessing(bool),

    // ---- terminal ----
    TerminalOpen {
        cols: u16,
        rows: u16,
    },
    TerminalInput(Vec<u8>),
    TerminalResize {
        cols: u16,
        rows: u16,
    },
    TerminalScroll(i32),
    TerminalClose,

    // ---- prompts, settings, notices ----
    /// An answer to an unknown or stale prompt id is ignored.
    Answer {
        id: PromptId,
        answer: PromptAnswer,
    },
    DismissNotice(NoticeId),
    UpdateSettings(Settings),
    /// Asks for confirmation when transfers are active.
    Quit,
}
