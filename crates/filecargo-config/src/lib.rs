//! Server tree, settings, secrets and FileZilla import, shared by the TUI and GUI.

mod error;
mod fsio;
mod import;
mod model;
mod paths;
mod secrets;
mod settings;
mod store;
mod tree;

pub use error::{ConfigError, ImportError, ValidationError};
pub use import::{
    ImportOptions, ImportReport, PasswordNotImported, SkippedSite, default_filezilla_path,
};
pub use model::{Auth, Folder, FolderId, FtpMode, NodeId, Protocol, Site, SiteId};
pub use paths::Paths;
pub use secrets::{
    ExposeSecret, KeyringStore, MemoryStore, SERVICE, SecretError, SecretKey, SecretStore,
    SecretString, UnavailableStore, default_secret_store,
};
pub use settings::{
    ConflictRule, ConnectionSettings, LogLevel, LogSettings, Settings, TransferSettings, UiSettings,
};
pub use store::ConfigStore;
pub use tree::{Node, ServerTree, TreeOp};
