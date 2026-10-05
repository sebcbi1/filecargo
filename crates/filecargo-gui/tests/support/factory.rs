#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]
//! A `SessionFactory` that serves directories of the local disk through `RootedFs`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use filecargo_app_core::SessionFactory;
use filecargo_app_core::prelude::*;
use filecargo_remote_fs::{ConnectContext, ConnectError, RootedFs, Session};

#[derive(Default)]
pub struct RootedFactory {
    roots: Mutex<HashMap<String, PathBuf>>,
    pub connects: std::sync::atomic::AtomicUsize,
}

impl RootedFactory {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// A server at `host` whose files are `root`.
    pub fn serve(&self, host: &str, root: PathBuf) {
        self.roots.lock().unwrap().insert(host.to_owned(), root);
    }
}

#[async_trait]
impl SessionFactory for RootedFactory {
    async fn connect(&self, site: &Site, _ctx: &ConnectContext) -> Result<Session, ConnectError> {
        let root = self
            .roots
            .lock()
            .unwrap()
            .get(&site.host)
            .cloned()
            .ok_or_else(|| ConnectError::Refused(site.host.clone()))?;
        self.connects
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(Session {
            fs: Arc::new(RootedFs::new(root).map_err(|e| ConnectError::Network(e.to_string()))?),
            info: SessionInfo {
                protocol: site.protocol,
                banner: None,
                tls: None,
                home: RemotePath::root(),
            },
            shell: None,
        })
    }
}
