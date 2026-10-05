#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]
//! A "server" made of a directory on disk, wrapped to count what the queue does to it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use filecargo_config::{Protocol, SiteId};
use filecargo_remote_fs::{Capabilities, Entry, FsError, Progress, RemoteFs, RemotePath, RootedFs};
use filecargo_transfer::{
    ConflictInfo, Connector, Queue, QueueEvent, QueueLimits, TransferError, TransferId,
};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;

#[path = "flaky.rs"]
pub mod flaky;
#[allow(unused_imports)]
pub use flaky::{Fault, FlakyControl, FlakyFs};

/// What the queue did to the server.
#[derive(Default)]
pub struct Stats {
    pub in_flight: AtomicUsize,
    pub peak_in_flight: AtomicUsize,
    pub connects: AtomicUsize,
    pub closes: AtomicUsize,
    pub transfers: AtomicUsize,
}

impl Stats {
    pub fn peak(&self) -> usize {
        self.peak_in_flight.load(Ordering::SeqCst)
    }
    pub fn connects(&self) -> usize {
        self.connects.load(Ordering::SeqCst)
    }
}

/// A `RemoteFs` over a directory that counts concurrent transfers and can delay each one.
pub struct CountingFs {
    inner: RootedFs,
    pub stats: Arc<Stats>,
    pub delay: Duration,
}

struct InFlight<'a>(&'a Stats);

impl<'a> InFlight<'a> {
    fn enter(stats: &'a Stats) -> Self {
        let now = stats.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        stats.peak_in_flight.fetch_max(now, Ordering::SeqCst);
        stats.transfers.fetch_add(1, Ordering::SeqCst);
        Self(stats)
    }
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.0.in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

#[async_trait]
impl RemoteFs for CountingFs {
    fn protocol(&self) -> Protocol {
        Protocol::Sftp
    }
    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }
    async fn home(&self) -> Result<RemotePath, FsError> {
        self.inner.home().await
    }
    async fn list(&self, dir: &RemotePath) -> Result<Vec<Entry>, FsError> {
        self.inner.list(dir).await
    }
    async fn stat(&self, path: &RemotePath) -> Result<Option<Entry>, FsError> {
        self.inner.stat(path).await
    }
    async fn mkdir(&self, path: &RemotePath) -> Result<(), FsError> {
        self.inner.mkdir(path).await
    }
    async fn rename(&self, from: &RemotePath, to: &RemotePath) -> Result<(), FsError> {
        self.inner.rename(from, to).await
    }
    async fn remove_file(&self, path: &RemotePath) -> Result<(), FsError> {
        self.inner.remove_file(path).await
    }
    async fn remove_dir(&self, path: &RemotePath) -> Result<(), FsError> {
        self.inner.remove_dir(path).await
    }
    async fn chmod(&self, path: &RemotePath, mode: u32) -> Result<(), FsError> {
        self.inner.chmod(path, mode).await
    }
    async fn set_modified(&self, path: &RemotePath, time: SystemTime) -> Result<(), FsError> {
        self.inner.set_modified(path, time).await
    }
    async fn download(
        &self,
        path: &RemotePath,
        offset: u64,
        sink: &mut (dyn AsyncWrite + Send + Unpin),
        progress: &dyn Progress,
    ) -> Result<u64, FsError> {
        let _guard = InFlight::enter(&self.stats);
        tokio::time::sleep(self.delay).await;
        self.inner.download(path, offset, sink, progress).await
    }
    async fn upload(
        &self,
        path: &RemotePath,
        offset: u64,
        source: &mut (dyn AsyncRead + Send + Unpin),
        progress: &dyn Progress,
    ) -> Result<u64, FsError> {
        let _guard = InFlight::enter(&self.stats);
        tokio::time::sleep(self.delay).await;
        self.inner.upload(path, offset, source, progress).await
    }
    async fn close(&self) {
        self.stats.closes.fetch_add(1, Ordering::SeqCst);
    }
}

/// Hands out [`CountingFs`] connections to the "server" directory registered for a site.
pub struct TestConnector {
    roots: Mutex<HashMap<SiteId, PathBuf>>,
    pub stats: Arc<Stats>,
    pub delay: Mutex<Duration>,
    pub flaky: Arc<FlakyControl>,
}

impl TestConnector {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            roots: Mutex::default(),
            stats: Arc::default(),
            delay: Mutex::new(Duration::ZERO),
            flaky: Arc::default(),
        })
    }

    /// A new site whose server is `root`.
    pub fn site(&self, root: &Path) -> SiteId {
        let site = SiteId::new();
        self.roots.lock().unwrap().insert(site, root.to_path_buf());
        site
    }

    pub fn set_delay(&self, delay: Duration) {
        *self.delay.lock().unwrap() = delay;
    }
}

#[async_trait]
impl Connector for TestConnector {
    async fn connect(&self, site: SiteId) -> Result<Arc<dyn RemoteFs>, TransferError> {
        let root = self
            .roots
            .lock()
            .unwrap()
            .get(&site)
            .cloned()
            .ok_or(TransferError::SiteDeleted)?;
        if self.flaky.take_connect_failure() {
            return Err(TransferError::Connect("connection refused".to_owned()));
        }
        self.stats.connects.fetch_add(1, Ordering::SeqCst);
        let inner = RootedFs::new(root).map_err(|e| TransferError::Connect(e.to_string()))?;
        let counting = Arc::new(CountingFs {
            inner,
            stats: self.stats.clone(),
            delay: *self.delay.lock().unwrap(),
        });
        Ok(Arc::new(FlakyFs::new(counting, self.flaky.clone())))
    }
}

pub struct Harness {
    pub queue: Queue,
    pub events: mpsc::UnboundedReceiver<QueueEvent>,
    pub connector: Arc<TestConnector>,
    pub local: tempfile::TempDir,
    pub server: tempfile::TempDir,
    pub site: SiteId,
    pub data: tempfile::TempDir,
    /// `ItemFinished` events seen so far, successful and failed.
    pub finished_ok: usize,
    pub finished_failed: usize,
    /// Every `ConflictAsked` seen so far.
    pub conflicts: Vec<(TransferId, ConflictInfo)>,
}

impl Harness {
    pub async fn new(max_concurrent: u8) -> Self {
        let connector = TestConnector::new();
        let server = tempfile::tempdir().unwrap();
        let site = connector.site(server.path());
        let data = tempfile::tempdir().unwrap();
        let (queue, events) = Queue::start(
            connector.clone(),
            data.path().join("queue.json"),
            QueueLimits {
                max_concurrent,
                ..QueueLimits::default()
            },
        )
        .await
        .unwrap();
        Self {
            queue,
            events,
            connector,
            local: tempfile::tempdir().unwrap(),
            server,
            site,
            data,
            finished_ok: 0,
            finished_failed: 0,
            conflicts: Vec::new(),
        }
    }

    /// Waits for the next conflict question.
    pub async fn next_conflict(&mut self) -> (TransferId, ConflictInfo) {
        let wait = async {
            while let Some(event) = self.events.recv().await {
                match event {
                    QueueEvent::ConflictAsked { id, conflict } => {
                        self.conflicts.push((id, conflict.clone()));
                        return (id, conflict);
                    }
                    QueueEvent::ItemFinished { ok: true, .. } => self.finished_ok += 1,
                    QueueEvent::ItemFinished { ok: false, .. } => self.finished_failed += 1,
                    QueueEvent::Idle | QueueEvent::Changed => {}
                }
            }
            panic!("the event channel closed before a conflict was asked");
        };
        tokio::time::timeout(Duration::from_secs(30), wait)
            .await
            .expect("no conflict was asked within 30 s")
    }

    /// Waits for the queue to go idle (20 s of real time).
    pub async fn idle(&mut self) {
        self.idle_for(Duration::from_secs(20)).await;
    }

    /// Waits for the queue to go idle; paused-time tests pass a long *virtual* duration.
    pub async fn idle_for(&mut self, limit: Duration) {
        let wait = async {
            while let Some(event) = self.events.recv().await {
                match event {
                    QueueEvent::Idle => return,
                    QueueEvent::ItemFinished { ok: true, .. } => self.finished_ok += 1,
                    QueueEvent::ItemFinished { ok: false, .. } => self.finished_failed += 1,
                    QueueEvent::ConflictAsked { id, conflict } => {
                        self.conflicts.push((id, conflict))
                    }
                    _ => {}
                }
            }
            panic!("the event channel closed before the queue went idle");
        };
        if tokio::time::timeout(limit, wait).await.is_err() {
            panic!(
                "the queue did not go idle within {limit:?}; snapshot: {:#?}",
                self.queue.snapshot()
            );
        }
    }
}

pub fn remote(path: &str) -> RemotePath {
    RemotePath::parse(path).unwrap()
}

pub fn sample_bytes(seed: u64, len: usize) -> Vec<u8> {
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 24) as u8
        })
        .collect()
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A tree like `top0/sub0 .. top4/sub8`: 5 + 45 = 50 directories, `per_dir` files in each.
pub fn build_tree(root: &Path, per_dir: usize) {
    let mut seed = 1u64;
    std::fs::create_dir_all(root).unwrap();
    for a in 0..5 {
        let top = root.join(format!("top{a}"));
        std::fs::create_dir_all(&top).unwrap();
        let mut dirs = vec![top.clone()];
        for b in 0..9 {
            let sub = top.join(format!("sub{b}"));
            std::fs::create_dir_all(&sub).unwrap();
            dirs.push(sub);
        }
        for dir in dirs {
            for f in 0..per_dir {
                seed += 1;
                std::fs::write(
                    dir.join(format!("file{f:02}.dat")),
                    sample_bytes(seed, (seed as usize * 37) % 3000),
                )
                .unwrap();
            }
        }
    }
}

/// Relative path (with `/`) of every file under `root` -> SHA-256, and the directories.
pub fn tree_hashes(root: &Path) -> (std::collections::BTreeMap<String, String>, Vec<String>) {
    fn walk(
        base: &Path,
        dir: &Path,
        files: &mut std::collections::BTreeMap<String, String>,
        dirs: &mut Vec<String>,
    ) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let relative = entry
                .path()
                .strip_prefix(base)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                dirs.push(relative);
                walk(base, &entry.path(), files, dirs);
            } else if kind.is_file() {
                files.insert(relative, sha256_hex(&std::fs::read(entry.path()).unwrap()));
            }
        }
    }
    let (mut files, mut dirs) = (std::collections::BTreeMap::new(), Vec::new());
    walk(root, root, &mut files, &mut dirs);
    dirs.sort();
    (files, dirs)
}

/// Sets a file's modification time (seconds since the epoch).
pub fn set_mtime(path: &Path, secs: u64) {
    let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    file.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(secs))
        .unwrap();
}
