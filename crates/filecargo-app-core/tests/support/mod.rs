#![allow(dead_code, unused_imports, clippy::unwrap_used, clippy::expect_used)]
//! Headless harness: a temp config dir, an in-memory keychain, and helpers that wait on the
//! published snapshots (blocking on the app's own runtime, so tests are plain `#[test]`s).

use std::sync::Arc;
use std::time::Duration;

use filecargo_app_core::{App, AppHandle, AppState, StartOptions};
use filecargo_config::{MemoryStore, Paths};

pub struct Fixture {
    pub app: AppHandle,
    pub config: tempfile::TempDir,
}

impl Fixture {
    pub fn new() -> Self {
        Self::with(|_| {})
    }

    /// Starts an app on a fresh config dir after letting `prepare` write files into it.
    pub fn with(prepare: impl FnOnce(&std::path::Path)) -> Self {
        let config = tempfile::tempdir().unwrap();
        prepare(config.path());
        let app = App::start(StartOptions {
            paths: Some(Paths::from_override(Some(config.path().to_path_buf()))),
            secrets: Some(Arc::new(MemoryStore::new())),
            connector: None,
        })
        .unwrap();
        Self { app, config }
    }

    pub fn state(&self) -> Arc<AppState> {
        self.app.state().borrow().clone()
    }

    /// Waits (up to 5 s) for a snapshot satisfying `condition` and returns it.
    pub fn wait_for(&self, what: &str, condition: impl Fn(&AppState) -> bool) -> Arc<AppState> {
        let mut rx = self.app.state();
        self.app.runtime().block_on(async {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            loop {
                {
                    let state = rx.borrow_and_update().clone();
                    if condition(&state) {
                        return state;
                    }
                }
                if tokio::time::timeout_at(deadline, rx.changed())
                    .await
                    .is_err()
                {
                    panic!(
                        "timed out waiting for {what}; last state: {:#?}",
                        rx.borrow()
                    );
                }
            }
        })
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.app.clone().shutdown(Duration::from_secs(2));
    }
}

impl Fixture {
    /// Waits (up to 5 s) for a spawned task and returns its result.
    pub fn join<T: Send + 'static>(&self, task: tokio::task::JoinHandle<T>) -> T {
        self.app.runtime().block_on(async {
            tokio::time::timeout(Duration::from_secs(5), task)
                .await
                .expect("the task did not finish within 5 s")
                .unwrap()
        })
    }

    /// Waits for a prompt other than `after` to be showing.
    pub fn wait_for_prompt_after(
        &self,
        after: Option<filecargo_app_core::PromptId>,
    ) -> filecargo_app_core::Prompt {
        let state = self.wait_for("a new prompt", |s| {
            s.prompt.as_ref().is_some_and(|p| Some(p.id) != after)
        });
        state.prompt.clone().unwrap()
    }
}

// ---- a served directory as a "server" -------------------------------------------------------

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::SystemTime;

use async_trait::async_trait;
use filecargo_app_core::SessionFactory;
use filecargo_config::{Protocol, SecretKey, SecretStore, Site, SiteId};
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
                let typed = secrecy::ExposeSecret::expose_secret(&answer.values[0]).to_owned();
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
        };
        Ok(Session {
            fs: Arc::new(fs),
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

pub fn site_for(name: &str, host: &str) -> Site {
    let mut site = Site::new(name, Protocol::Sftp, host);
    site.user = "me".to_owned();
    site
}

impl Fixture {
    /// An app whose sessions are served by `factory`.
    pub fn with_factory(factory: Arc<TestFactory>) -> Self {
        let config = tempfile::tempdir().unwrap();
        let app = App::start(StartOptions {
            paths: Some(Paths::from_override(Some(config.path().to_path_buf()))),
            secrets: Some(Arc::new(MemoryStore::new())),
            connector: Some(factory),
        })
        .unwrap();
        Self { app, config }
    }

    /// Adds `site` to the tree and waits for it.
    pub fn add_site(&self, site: Site) -> SiteId {
        let id = site.id;
        self.app.send(filecargo_app_core::Command::Tree(
            filecargo_config::TreeOp::AddSite(site),
        ));
        self.wait_for("the site", |s| s.servers.site(id).is_some());
        id
    }
}

/// A directory tree to serve: `projects/{a.txt, b.txt, sub/c.txt}`, `readme.md`.
pub fn server_tree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("projects/sub")).unwrap();
    std::fs::write(dir.path().join("projects/a.txt"), "aaa").unwrap();
    std::fs::write(dir.path().join("projects/b.txt"), "bb").unwrap();
    std::fs::write(dir.path().join("projects/sub/c.txt"), "c").unwrap();
    std::fs::write(dir.path().join("readme.md"), "# hi").unwrap();
    dir
}
