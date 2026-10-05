//! SFTP backend: one SSH connection carrying the SFTP subsystem and, on demand, PTY shells.
//! Every russh type stays inside this module.

mod auth;
mod fs;
mod host_keys;
mod transport;

use std::sync::Arc;

use filecargo_config::Site;

pub use auth::authenticate;
pub use fs::SftpFs;
pub use transport::{SshConnection, connect_ssh};

use crate::{ConnectContext, ConnectError};

/// Connects, verifies the host key, authenticates and starts the SFTP subsystem.
pub async fn open(site: &Site, ctx: &ConnectContext) -> Result<SftpFs, ConnectError> {
    let mut conn = connect_ssh(site, ctx).await?;
    authenticate(&mut conn, site, ctx).await?;
    fs::open_sftp(Arc::new(conn), ctx).await
}
