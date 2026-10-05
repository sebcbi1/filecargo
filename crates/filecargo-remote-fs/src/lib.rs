//! One async filesystem interface over SFTP, FTP, FTPS and the local disk, plus connecting.

mod connect;
mod credentials;
mod entry;
mod error;
mod fs;
pub mod local;
mod path;
mod prompt;
pub mod sftp;
mod trust;

pub use connect::{ConnectContext, ConnectError};
pub use entry::{Capabilities, Entry, EntryKind};
pub use error::FsError;
pub use fs::{NoProgress, Progress, RemoteFs};
pub use local::RootedFs;
pub use path::{PathError, RemotePath};
pub use prompt::{CredentialAnswer, CredentialPrompt, HostKeyPrompt, Prompter, TrustDecision};
pub use trust::SessionTrust;
