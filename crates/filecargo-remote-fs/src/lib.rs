//! One async filesystem interface over SFTP, FTP, FTPS and the local disk, plus connecting.

mod connect;
mod credentials;
mod entry;
mod error;
mod fs;
pub mod ftp;
pub mod local;
mod path;
mod prompt;
pub mod sftp;
mod tls;
mod trust;

pub use connect::{ConnectContext, ConnectError, Session, SessionInfo, connect};
pub use entry::{Capabilities, Entry, EntryKind};
pub use error::FsError;
pub use fs::{NoProgress, Progress, RemoteFs};
pub use ftp::FtpFs;
pub use local::RootedFs;
pub use path::{PathError, RemotePath};
pub use prompt::{
    CertificateProblem, CertificatePrompt, CredentialAnswer, CredentialPrompt, HostKeyPrompt,
    Prompter, TrustDecision,
};
pub use sftp::{ShellChannel, ShellInput, ShellOpener, ShellOutput};
pub use trust::SessionTrust;
