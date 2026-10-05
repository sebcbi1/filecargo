//! How sessions are opened.

use std::sync::Arc;

use async_trait::async_trait;
use filecargo_config::Site;
use filecargo_remote_fs::{ConnectContext, ConnectError, Session};

/// The browsing session and every transfer worker connection go through this. The default
/// calls `remote_fs::connect`; tests serve a directory on disk.
#[async_trait]
pub trait SessionFactory: Send + Sync {
    async fn connect(&self, site: &Site, ctx: &ConnectContext) -> Result<Session, ConnectError>;
}

pub(crate) struct RealFactory;

#[async_trait]
impl SessionFactory for RealFactory {
    async fn connect(&self, site: &Site, ctx: &ConnectContext) -> Result<Session, ConnectError> {
        filecargo_remote_fs::connect(site, ctx).await
    }
}

pub(crate) fn default_factory() -> Arc<dyn SessionFactory> {
    Arc::new(RealFactory)
}
