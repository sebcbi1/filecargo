//! SFTP backend: one SSH connection carrying the SFTP subsystem and, on demand, PTY shells.
//! Every russh type stays inside this module.

mod host_keys;
mod transport;

pub use transport::{SshConnection, connect_ssh};
