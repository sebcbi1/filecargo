//! Server tree, settings, secrets and FileZilla import, shared by the TUI and GUI.

mod error;
mod fsio;
mod model;
mod paths;
mod settings;
mod store;
mod tree;

pub use error::{ConfigError, ValidationError};
pub use model::{Auth, Folder, FolderId, FtpMode, NodeId, Protocol, Site, SiteId};
pub use paths::Paths;
pub use settings::{
    ConflictRule, ConnectionSettings, LogLevel, LogSettings, Settings, TransferSettings, UiSettings,
};
pub use store::ConfigStore;
pub use tree::{Node, ServerTree, TreeOp};
