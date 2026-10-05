//! One async filesystem interface over SFTP, FTP, FTPS and the local disk, plus connecting.

mod entry;
mod error;
mod fs;
pub mod local;
mod path;

pub use entry::{Capabilities, Entry, EntryKind};
pub use error::FsError;
pub use fs::{NoProgress, Progress, RemoteFs};
pub use local::RootedFs;
pub use path::{PathError, RemotePath};
