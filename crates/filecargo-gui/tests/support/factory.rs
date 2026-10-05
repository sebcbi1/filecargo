#![allow(unused_imports)]
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]
//! `SessionFactory` serving directories of the local disk through `RootedFs`, with the knobs
//! the prompt tests need: a password to ask for, host-key / certificate prompts, a transfer
//! delay (so a transfer stays active) and a kill switch. Adapted from app-core's test support.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use std::time::SystemTime;

use async_trait::async_trait;
use filecargo_app_core::SessionFactory;
use filecargo_app_core::prelude::{Protocol, Site, SiteId};
use filecargo_config::{SecretKey, SecretStore};
use filecargo_remote_fs::{
    Capabilities, ConnectContext, ConnectError, CredentialPrompt, Entry, FsError, Progress,
    RemoteFs, RemotePath, RootedFs, Session, SessionInfo,
};
use tokio::io::{AsyncRead, AsyncWrite};

pub struct Served {
    pub root: PathBuf,
    /// `Some`: connecting asks for this password through the prompter.
    pub password: Option<String>,
}

/// A `RemoteFs` over a directory that can be "killed" (every call then fails as a lost
/// connection) and that counts being closed.
pub struct KillableFs {
    inner: RootedFs,
    dead: Arc<AtomicBool>,
    closes: Arc<AtomicUsize>,
    transfer_delay: Duration,
}

impl KillableFs {
    fn check(&self) -> Result<(), FsError> {
        if self.dead.load(Ordering::SeqCst) {
            Err(FsError::Disconnected("connection reset by peer".to_owned()))
        } else {
            Ok(())
        }
    }
}

#[async_trait]
impl RemoteFs for KillableFs {
    fn protocol(&self) -> Protocol {
        self.inner.protocol()
    }
    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }
    async fn home(&self) -> Result<RemotePath, FsError> {
        self.check()?;
        self.inner.home().await
    }
    async fn list(&self, dir: &RemotePath) -> Result<Vec<Entry>, FsError> {
        self.check()?;
        self.inner.list(dir).await
    }
    async fn stat(&self, path: &RemotePath) -> Result<Option<Entry>, FsError> {
        self.check()?;
        self.inner.stat(path).await
    }
    async fn mkdir(&self, path: &RemotePath) -> Result<(), FsError> {
        self.check()?;
        self.inner.mkdir(path).await
    }
    async fn rename(&self, from: &RemotePath, to: &RemotePath) -> Result<(), FsError> {
        self.check()?;
        self.inner.rename(from, to).await
    }
    async fn remove_file(&self, path: &RemotePath) -> Result<(), FsError> {
        self.check()?;
        self.inner.remove_file(path).await
    }
    async fn remove_dir(&self, path: &RemotePath) -> Result<(), FsError> {
        self.check()?;
        self.inner.remove_dir(path).await
    }
    async fn chmod(&self, path: &RemotePath, mode: u32) -> Result<(), FsError> {
        self.check()?;
        self.inner.chmod(path, mode).await
    }
    async fn set_modified(&self, path: &RemotePath, time: SystemTime) -> Result<(), FsError> {
        self.check()?;
        self.inner.set_modified(path, time).await
    }
    async fn download(
        &self,
        path: &RemotePath,
        offset: u64,
        sink: &mut (dyn AsyncWrite + Send + Unpin),
        progress: &dyn Progress,
    ) -> Result<u64, FsError> {
        self.check()?;
        tokio::time::sleep(self.transfer_delay).await;
        self.inner.download(path, offset, sink, progress).await
    }
    async fn upload(
        &self,
        path: &RemotePath,
        offset: u64,
        source: &mut (dyn AsyncRead + Send + Unpin),
        progress: &dyn Progress,
    ) -> Result<u64, FsError> {
        self.check()?;
        tokio::time::sleep(self.transfer_delay).await;
        self.inner.upload(path, offset, source, progress).await
    }
    async fn close(&self) {
        self.closes.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Default)]
pub struct TestFactory {
    sites: Mutex<HashMap<String, Served>>,
    pub connects: AtomicUsize,
    pub closes: Arc<AtomicUsize>,
    pub delay: Mutex<Duration>,
    /// The next `connect` fails with this.
    pub fail_next: Mutex<Option<ConnectError>>,
    /// The kill switches of every connection handed out, oldest first.
    pub switches: Mutex<Vec<Arc<AtomicBool>>>,
    /// Every transfer waits this long before starting.
    pub transfer_delay: Mutex<Duration>,
    /// Connecting asks about this host key first (answer `Reject` fails the connection).
    pub host_key: Mutex<Option<filecargo_remote_fs::HostKeyPrompt>>,
    /// Connecting asks about this certificate first.
    pub certificate: Mutex<Option<filecargo_remote_fs::CertificatePrompt>>,
    /// SFTP-like sessions get a shell from this backend.
    pub shell: Mutex<Option<Arc<super::terminal::FakeShell>>>,
}

impl TestFactory {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// A server at `host`, serving `root`.
    pub fn serve(&self, host: &str, root: PathBuf, password: Option<&str>) {
        self.sites.lock().unwrap().insert(
            host.to_owned(),
            Served {
                root,
                password: password.map(str::to_owned),
            },
        );
    }

    pub fn connects(&self) -> usize {
        self.connects.load(Ordering::SeqCst)
    }

    pub fn closes(&self) -> usize {
        self.closes.load(Ordering::SeqCst)
    }

    /// Makes the newest connection fail every call, as if the network dropped.
    pub fn kill_latest(&self) {
        self.switches
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .store(true, Ordering::SeqCst);
    }
}

#[async_trait]
impl SessionFactory for TestFactory {
    async fn connect(&self, site: &Site, ctx: &ConnectContext) -> Result<Session, ConnectError> {
        let delay = *self.delay.lock().unwrap();
        tokio::time::sleep(delay).await;
        if let Some(error) = self.fail_next.lock().unwrap().take() {
            return Err(error);
        }
        let host_key = self.host_key.lock().unwrap().clone();
        if let Some(prompt) = host_key
            && ctx.prompter.host_key(prompt).await == filecargo_remote_fs::TrustDecision::Reject
        {
            return Err(ConnectError::HostKeyRejected);
        }
        let certificate = self.certificate.lock().unwrap().clone();
        if let Some(prompt) = certificate
            && ctx.prompter.certificate(prompt).await == filecargo_remote_fs::TrustDecision::Reject
        {
            return Err(ConnectError::CertificateRejected);
        }
        let (root, password) = {
            let sites = self.sites.lock().unwrap();
            let served = sites
                .get(&site.host)
                .ok_or_else(|| ConnectError::Refused(site.host.clone()))?;
            (served.root.clone(), served.password.clone())
        };
        if let Some(expected) = password {
            for attempt in 0..2 {
                let answer = ctx
                    .prompter
                    .credential(CredentialPrompt::Password {
                        site: site.name.clone(),
                        user: site.user.clone(),
                        retry: attempt > 0,
                    })
                    .await
                    .ok_or(ConnectError::Cancelled)?;
                let typed =
                    filecargo_config::ExposeSecret::expose_secret(&answer.values[0]).to_owned();
                if typed == expected {
                    if answer.remember {
                        let _ = ctx
                            .secrets
                            .set(&SecretKey::Password(site.id), &answer.values[0]);
                    }
                    break;
                }
                if attempt == 1 {
                    return Err(ConnectError::AuthFailed {
                        methods_tried: vec!["password".to_owned()],
                    });
                }
            }
        }
        self.connects.fetch_add(1, Ordering::SeqCst);
        let dead = Arc::new(AtomicBool::new(false));
        self.switches.lock().unwrap().push(dead.clone());
        let fs = KillableFs {
            inner: RootedFs::new(root).map_err(|e| ConnectError::Network(e.to_string()))?,
            dead,
            closes: self.closes.clone(),
            transfer_delay: *self.transfer_delay.lock().unwrap(),
        };
        let shell = self
            .shell
            .lock()
            .unwrap()
            .clone()
            .filter(|_| site.protocol == Protocol::Sftp)
            .map(|backend| filecargo_remote_fs::ShellOpener::from_backend(backend));
        Ok(Session {
            fs: Arc::new(fs),
            info: SessionInfo {
                protocol: site.protocol,
                banner: None,
                tls: None,
                home: RemotePath::root(),
            },
            shell,
        })
    }
}

pub fn site_for(name: &str, host: &str) -> Site {
    let mut site = Site::new(name, Protocol::Sftp, host);
    site.user = "me".to_owned();
    site
}
