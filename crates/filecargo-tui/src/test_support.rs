//! Deterministic fixtures for reducer and snapshot tests: fixed names, sizes and times.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use filecargo_app_core::prelude::*;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::reducer;
use crate::ui_state::UiState;
use crate::view;

/// 2026-09-30 12:00 UTC.
pub const NOW: u64 = 1_790_769_600;

pub fn entry(name: &str, dir: bool, size: u64, hours_ago: u64) -> Entry {
    let mut e = Entry::new(name, if dir { EntryKind::Dir } else { EntryKind::File });
    e.size = if dir { 0 } else { size };
    e.modified = Some(UNIX_EPOCH + Duration::from_secs(NOW - hours_ago * 3600));
    e
}

pub fn local_entries() -> Vec<Entry> {
    vec![
        entry("docs", true, 0, 30),
        entry("src", true, 0, 50),
        entry("README.md", false, 2150, 26),
        entry("notes.txt", false, 340, 3),
        entry("big.iso", false, 1_600_000_000, 900),
    ]
}

pub fn remote_entries() -> Vec<Entry> {
    vec![
        entry("html", true, 0, 40),
        entry("index.php", false, 4400, 38),
        entry("style.css", false, 12_800, 38),
    ]
}

pub fn pane<P>(path: P, entries: Vec<Entry>) -> Pane<P> {
    Pane {
        path,
        entries: Arc::from(entries),
        sort: Sort::default(),
        loading: false,
        error: None,
        generation: 1,
    }
}

/// Disconnected, local pane at `/home/me/projects`.
pub fn app() -> AppState {
    AppState {
        startup_error: None,
        servers: Arc::new(ServerTree::default()),
        settings: Arc::new(Settings::default()),
        session: SessionState::Disconnected,
        local: pane(PathBuf::from("/home/me/projects"), local_entries()),
        remote: None,
        queue: Arc::new(QueueSnapshot::default()),
        scope: None,
        site_queue: Arc::new(QueueSnapshot::default()),
        other_sites_active: 0,
        terminal: TerminalState::NotAvailable,
        prompt: None,
        notices: Vec::new(),
        log_generation: 0,
    }
}

/// The same, connected to a server with a remote pane at `/var/www`.
pub fn connected_app() -> AppState {
    let mut app = app();
    app.remote = Some(pane(
        RemotePath::parse("/var/www").unwrap(),
        remote_entries(),
    ));
    app
}

pub fn ui(width: u16, height: u16) -> UiState {
    let mut ui = UiState::new(width, height);
    ui.now = UNIX_EPOCH + Duration::from_secs(NOW);
    ui.home = Some(PathBuf::from("/home/me"));
    ui.color = true;
    ui
}

/// A synced UI for `app`.
pub fn synced(width: u16, height: u16, app: &AppState) -> UiState {
    let mut ui = ui(width, height);
    reducer::sync(&mut ui, app);
    ui
}

/// The screen as text, for snapshots.
pub fn draw(ui: &UiState, app: &AppState) -> String {
    let (width, height) = ui.size;
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| view::render(frame, ui, app)).unwrap();
    terminal.backend().to_string()
}

/// Work/{prod-web (sftp), staging (ftp)}, Personal/{blog (ftps)}, home-nas (sftp).
pub fn sample_tree() -> ServerTree {
    use filecargo_config::{ConfigStore, MemoryStore, Paths};
    let dir = tempfile::tempdir().unwrap();
    let mut store = ConfigStore::open(
        Paths::from_override(Some(dir.path().to_path_buf())),
        Arc::new(MemoryStore::new()),
    )
    .unwrap();
    let mut folder = |name: &str| match store
        .apply(TreeOp::AddFolder {
            name: name.into(),
            parent: None,
        })
        .unwrap()
    {
        NodeId::Folder(id) => id,
        NodeId::Site(_) => unreachable!(),
    };
    let (work, personal) = (folder("Work"), folder("Personal"));
    let mut add = |name: &str, protocol, parent| {
        let mut site = Site::new(name, protocol, format!("{name}.example.org"));
        site.folder = parent;
        store.apply(TreeOp::AddSite(site)).unwrap();
    };
    add("prod-web", Protocol::Sftp, Some(work));
    add("staging", Protocol::Ftp, Some(work));
    add("blog", Protocol::FtpsExplicit, Some(personal));
    add("home-nas", Protocol::Sftp, None);
    store.tree().clone()
}

/// The site called `name` in `tree`.
pub fn site_id(tree: &ServerTree, name: &str) -> SiteId {
    tree.sites().iter().find(|s| s.name == name).unwrap().id
}

/// A queue item uploading (`up`) or downloading `name`, in the given state.
pub fn queue_item(
    id: u64,
    up: bool,
    name: &str,
    size: Option<u64>,
    transferred: u64,
    state: ItemState,
) -> QueueItemView {
    QueueItemView {
        item: QueueItem {
            id: TransferId(id),
            site: test_site(),
            direction: if up {
                Direction::Upload
            } else {
                Direction::Download
            },
            local: PathBuf::from(format!("/home/me/projects/{name}")),
            remote: RemotePath::parse(&format!("/var/www/{name}")).unwrap(),
            is_dir: false,
            size,
            transferred,
            state,
            attempts: 1,
            parent: None,
            conflict: None,
        },
        rate: None,
        eta: None,
    }
}

fn at(seconds_ago: u64) -> std::time::SystemTime {
    UNIX_EPOCH + Duration::from_secs(NOW - seconds_ago)
}

/// One active upload, one waiting on a conflict, one queued directory; two completed, one failed.
pub fn busy_queue() -> QueueSnapshot {
    let mut active = queue_item(
        1,
        true,
        "backup.tar",
        Some(25_000_000),
        11_000_000,
        ItemState::Active { started: at(60) },
    );
    active.rate = Some(1_258_291.0);
    active.eta = Some(Duration::from_secs(12));
    let waiting = queue_item(
        2,
        true,
        "index.php",
        Some(4400),
        0,
        ItemState::AwaitingDecision {
            conflict: ConflictInfo {
                source: entry("index.php", false, 4400, 1),
                target: entry("index.php", false, 3900, 40),
            },
        },
    );
    let mut folder = queue_item(3, false, "photos", None, 0, ItemState::Pending);
    folder.item.is_dir = true;
    let done = |id, name: &str, outcome| {
        queue_item(
            id,
            false,
            name,
            Some(2048),
            2048,
            ItemState::Completed {
                outcome,
                finished: at(300),
            },
        )
    };
    let failed = queue_item(
        6,
        true,
        "big.iso",
        Some(1_600_000_000),
        1000,
        ItemState::Failed {
            reason: "connection reset by peer".to_owned(),
            retryable: true,
            finished: at(30),
        },
    );
    QueueSnapshot {
        pending: vec![active, waiting, folder],
        completed: vec![
            done(4, "style.css", Outcome::Transferred),
            done(5, "notes.txt", Outcome::Skipped),
        ],
        failed: vec![failed],
        processing: true,
        paused_sites: Default::default(),
        totals: Totals {
            bytes_done: 11_000_000,
            bytes_total: 26_000_000,
            rate: Some(1_258_291.0),
            eta: Some(Duration::from_secs(12)),
        },
    }
}

/// A terminal view fed `text` as if the remote had printed it. Needs a tokio runtime.
pub struct FakeShell {
    pub view: TerminalView,
    pub output: tokio::sync::mpsc::UnboundedSender<filecargo_remote_fs::ShellOutput>,
    /// Kept so the shell channel stays open.
    pub _input: tokio::sync::mpsc::UnboundedReceiver<filecargo_remote_fs::ShellInput>,
}

impl FakeShell {
    pub async fn open(cols: u16, rows: u16, text: &str) -> Self {
        let (input_tx, input) = tokio::sync::mpsc::unbounded_channel();
        let (output, output_rx) = tokio::sync::mpsc::unbounded_channel();
        let handle = filecargo_terminal::spawn(
            filecargo_remote_fs::ShellChannel {
                input: input_tx,
                output: output_rx,
            },
            TermSize { cols, rows },
            1000,
        );
        let shell = Self {
            view: TerminalView { handle },
            output,
            _input: input,
        };
        shell.say(text).await;
        shell
    }

    /// Feeds output and waits until the screen has taken it.
    pub async fn say(&self, text: &str) {
        let before = self.view.handle.generation();
        self.output
            .send(filecargo_remote_fs::ShellOutput::Data(
                text.as_bytes().to_vec(),
            ))
            .unwrap();
        for _ in 0..400 {
            if self.view.handle.generation() != before || text.is_empty() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("the screen never took the output");
    }
}

/// The site of every `queue_item` unless a test says otherwise.
pub fn test_site() -> SiteId {
    "00000000-0000-4000-8000-000000000001".parse().unwrap()
}

/// Another site, for items that must not show in `test_site`'s panel.
pub fn other_site() -> SiteId {
    "00000000-0000-4000-8000-000000000002".parse().unwrap()
}

/// Publishes `queue` the way app-core does while connected to `scope`: the full queue, plus the
/// view reduced to the scope's items and the count of other sites' active transfers.
pub fn scope_queue(app: &mut AppState, queue: QueueSnapshot, scope: Option<SiteId>) {
    let only = |views: &[QueueItemView]| -> Vec<QueueItemView> {
        views
            .iter()
            .filter(|v| Some(v.item.site) == scope)
            .cloned()
            .collect()
    };
    app.other_sites_active = queue
        .pending
        .iter()
        .filter(|v| matches!(v.item.state, ItemState::Active { .. }) && Some(v.item.site) != scope)
        .count();
    app.site_queue = Arc::new(QueueSnapshot {
        pending: only(&queue.pending),
        completed: only(&queue.completed),
        failed: only(&queue.failed),
        processing: queue.processing,
        paused_sites: queue
            .paused_sites
            .iter()
            .copied()
            .filter(|s| Some(*s) == scope)
            .collect(),
        totals: queue.totals.clone(),
    });
    app.scope = scope;
    app.queue = Arc::new(queue);
}

/// `scope_queue` for the common case: connected to `test_site()`.
pub fn set_queue(app: &mut AppState, queue: QueueSnapshot) {
    scope_queue(app, queue, Some(test_site()));
}
