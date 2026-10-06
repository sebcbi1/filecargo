# Spec: app-core

> Module id: `app-core` · Crate: `crates/filecargo-app-core` · Depends on: `config`, `remote-fs`, `transfer`, `terminal` · Status: **implemented, awaiting final review**
> Project-wide rules: [SPEC.md](SPEC.md). Plan: [tasks/app-core/plan.md](tasks/app-core/plan.md).

## Objective

The **one API both front-ends drive**. It owns the application state and the async runtime, and
turns user intents (*connect to this site*, *upload these files*, *answer this prompt*) into
work on the lower modules. A front-end only:
1. sends `Command`s
2. renders the latest `AppState` snapshot
3. keeps its own presentation state (focus, cursor, selection, scroll, form contents)

The TUI and GUI must be able to share all behavior. If one UI would need logic the other
doesn't have, that logic belongs here.

## Architecture

```
 UI thread (TUI loop / gpui)                     app-core (own tokio runtime, 2 workers)
 ───────────────────────────                     ──────────────────────────────────────
 handle.send(Command) ── mpsc (unbounded) ─────▶ App actor: owns ConfigStore, Session,
 state.changed().await ◀── watch<Arc<AppState>> ─  Queue, Terminal, prompts; publishes a new
 render(&*state.borrow())                          snapshot after each change (≤ 30/s)
```
- `App::start(options) -> Result<AppHandle, StartError>` creates the runtime, opens the config
  store (with `default_secret_store()`), starts the transfer queue, and installs the log layer.
- The UI side only uses `tokio::sync` channels, which are executor-agnostic. gpui can await
  `watch::Receiver::changed()` on its own executor, so **no gpui↔tokio bridge is needed**.
- `AppState` is immutable and cheap to clone: heavy parts sit behind `Arc` (entries, tree,
  queue snapshot). The log and terminal screen are shared buffers behind a mutex, which
  the UI reads at render time.

## Public API

```rust
pub struct StartOptions { pub paths: Option<Paths> /* None = Paths::resolve() */,
                          pub secrets: Option<Arc<dyn SecretStore>> /* None = OS keychain */,
                          pub connector: Option<Arc<dyn SessionFactory>> /* tests */ }

/// How sessions are opened. The default calls `remote_fs::connect`; tests serve `RootedFs`.
/// The browsing session and every transfer worker connection go through it.
#[async_trait]
pub trait SessionFactory: Send + Sync {
    async fn connect(&self, site: &Site, ctx: &ConnectContext) -> Result<Session, ConnectError>;
}

pub struct AppHandle { /* clone-able */ }
impl AppHandle {
    pub fn send(&self, command: Command);
    pub fn state(&self) -> watch::Receiver<Arc<AppState>>;
    pub fn log(&self) -> LogBuffer;                     // shared ring buffer (see Log)
    /// The app's tokio runtime, for front-ends that need timers (the TUI runs its loop on it
    /// with `block_on`). The GUI never uses it.
    pub fn runtime(&self) -> tokio::runtime::Handle;
    /// Flushes the queue, closes sessions, stops the runtime. Idempotent.
    pub fn shutdown(self, timeout: Duration);
}
```

### State

```rust
pub struct AppState {
    pub startup_error: Option<String>,           // e.g. corrupt servers.toml: UI shows reset option
    pub servers: Arc<ServerTree>,
    pub settings: Arc<Settings>,
    pub session: SessionState,
    pub local: Pane<PathBuf>,
    pub remote: Option<Pane<RemotePath>>,        // Some while connected
    pub queue: Arc<QueueSnapshot>,               // every site (quit counting, background transfers)
    pub scope: Option<SiteId>,                   // v1.1: the connected site; None when not connected
    pub site_queue: Arc<QueueSnapshot>,          // v1.1: `queue` reduced to `scope` (empty without a scope)
    pub other_sites_active: usize,               // v1.1: running transfers of other sites
    // plus `AppState::site_paused()`: is the connected site paused?
    pub terminal: TerminalState,                 // NotAvailable | Closed | Open(TerminalView) | Exited { code, view }
    pub prompt: Option<Prompt>,                  // the one prompt to show now (FIFO behind it)
    pub notices: Vec<Notice>,                    // transient toasts: id, level, text (UI dismisses)
    pub log_generation: u64,                     // bumps when new log lines arrive
}

pub enum SessionState {
    Disconnected,
    Connecting { site: SiteId, step: ConnectStep },   // Resolving | Connecting | Authenticating | Listing
    Connected  { site: SiteId, info: SessionInfo },
    Failed     { site: SiteId, error: String },
}

pub struct Pane<P> {
    pub path: P,
    pub entries: Arc<[Entry]>,                   // sorted and filtered; ".." is NOT included
    pub sort: Sort,                              // Name | Size | Modified, asc/desc; dirs first
    pub loading: bool,
    pub error: Option<String>,                   // listing failed; the previous entries are kept
    pub generation: u64,                         // bumps on every new listing (UI resets cursor)
}
```

### Prompts (all human decisions go through one mechanism)

```rust
pub struct Prompt { pub id: PromptId, pub kind: PromptKind }
pub enum PromptKind {
    Credential(CredentialPrompt),                // from remote-fs
    HostKey(HostKeyPrompt),
    Certificate(CertificatePrompt),
    Conflict { transfer: TransferId, conflict: ConflictInfo },
    ConfirmDelete { pane: PaneId, names: Vec<String>, recursive: bool },
    ConfirmQuit { active_transfers: usize },
    ConfirmClearQueue { items: usize, active: usize },       // v1.1: `QueueClear` of the connected site
    Message { level: Level, title: String, body: String },   // e.g. ImportReport, fatal errors
}
pub enum PromptAnswer {
    Credential(Option<CredentialAnswer>), Trust(TrustDecision), Conflict(ConflictDecision),
    Confirm(bool), Dismiss,
}
```
`app-core` implements `remote_fs::Prompter` by queuing a `Prompt` and awaiting the matching
`Command::Answer`. The transfer queue's `ConflictAsked` events become `Conflict` prompts too.
Answers with an unknown or stale `PromptId` are ignored.

### Commands

```rust
pub enum PaneId { Local, Remote }

pub enum Command {
    // server tree (thin wrappers over ConfigStore; errors → Message prompt)
    Tree(TreeOp), ImportFileZilla { path: PathBuf, import_passwords: bool }, ResetConfig,
    SetSitePassword { site: SiteId, secret: SecretString },     // from the site editor
    // session
    Connect(SiteId), Disconnect,
    // panes
    Navigate { pane: PaneId, path: String },     // absolute, or relative to the pane path
    Up(PaneId), Refresh(PaneId), SetSort { pane: PaneId, sort: Sort },
    // file operations on the entries of `pane` (v1.1: the local pane too; chmod not on Windows)
    Mkdir { pane: PaneId, name: String }, Rename { pane: PaneId, from: String, to: String },
    Delete { pane: PaneId, names: Vec<String> }, // confirm prompt when settings.ui.confirm_delete
    Chmod { pane: PaneId, names: Vec<String>, mode: u32 },
    // transfers (names are entries of the source pane's current dir)
    Upload { names: Vec<String> }, Download { names: Vec<String> },
    QueueRetry(TransferId), QueueRetryFailed, QueueRemove(TransferId),
    // queue (v1.1: these act on the connected site; without one they raise a notice and do nothing)
    Enqueue { from: PaneId, names: Vec<String> },   // like Upload/Download, but every item is Held
    QueueStartHeld, QueueSetSitePaused(bool),
    QueueClear,                                     // asks (ConfirmClearQueue), then clears the site's queue
    QueueClearCompleted, QueueClearFailed,          // scoped to the site
    QueueSetProcessing(bool),                       // queue-wide; the front-ends use QueueSetSitePaused
    // terminal
    TerminalOpen { cols: u16, rows: u16 }, TerminalInput(Vec<u8>),
    TerminalResize { cols: u16, rows: u16 }, TerminalScroll(i32), TerminalClose,
    // prompts, settings, notices
    Answer { id: PromptId, answer: PromptAnswer }, DismissNotice(NoticeId),
    UpdateSettings(Settings),
    Quit,                                         // ConfirmQuit when transfers are active
}
```

## Behavior

### Startup
- Config errors (corrupt or newer `servers.toml` / `settings.toml`) don't abort. The state
  starts with an empty tree and `startup_error`; `ResetConfig` backs the file up and reloads.
- The local pane opens `settings.ui.local_start_dir` (`~` expanded), falling back to the home dir.
- No keychain → `Notice` (warning) once: "passwords can't be saved on this system".

### Connecting
1. `Connect(site)` while connected → disconnect first (one session in v1). Active transfers
   keep running on their own connections.
2. `Connecting` with steps, then `remote_fs::connect`. Prompts appear as described above.
3. Remote pane opens `site.remote_dir` if it exists, else the server home. The local pane
   switches to `site.local_dir` if set.
4. Failure → `Failed { error }`, a log line, and a `Message` prompt for auth or TLS errors.
5. Connection loss while browsing → `Failed`. The next pane command reconnects automatically,
   once.

### Panes
- Listing runs in the background. A newer navigation supersedes an older in-flight listing; a
  late result is discarded via `generation`.
- Sort: directories first, then by key. Names compare case-insensitively with natural number
  order (`file2` < `file10`).
- `show_hidden = false` hides dotfiles (and, on Windows local listings, entries with the
  hidden attribute).
- Mkdir, rename, delete and chmod take a `pane`. For `PaneId::Local` they run on a `RootedFs` over the local directory through the same op path as the remote pane, then refresh the local pane. Delete is permanent and honours `confirm_delete`; local chmod is rejected with a notice on Windows.

### Transfers
- `Upload { names }`: for each local entry, a `NewTransfer` from `local.path/name` to
  `remote.path/name`, with `is_dir` from the entry. `Download` is the mirror image.
- The `Connector` given to the queue calls `remote_fs::connect` with the same `ConnectContext`
  (shared `SessionTrust`), so workers never re-prompt for what the user already answered.
- When an upload into the remote pane's current directory completes, that pane refreshes
  (debounced to 1/s). Same for downloads into the local pane.

### Scope and staged queue (v1.1)
- `scope` is the connected site; `site_queue` and `other_sites_active` are recomputed when the
  queue snapshot (by `Arc` pointer) or the session changes. Not connected: `site_queue` is
  empty. `queue` stays the full snapshot, so quit confirmation counts every site.
- `Enqueue { from, names }` expands and resolves conflicts like Upload/Download but enqueues
  held items. `QueueStartHeld` turns the site's held items pending (and resumes the site).
  `QueueSetSitePaused` pauses only that site. `QueueClear` raises `ConfirmClearQueue` (nothing
  happens when the site has no items); `Confirm(true)` cancels active items (partial files stay)
  and removes pending and held ones. Restored items arrive held (see SPEC-transfer).

### Terminal
- Available only for SFTP sessions (`TerminalState::NotAvailable` otherwise).
- Opened lazily on `TerminalOpen` (sent by the UI when the tab is first shown, with its size).
  Closed on disconnect. `TerminalView` exposes the `terminal::TerminalHandle` for rendering
  and key encoding.

### Log
- A `tracing` layer feeds a ring buffer of 5,000 `LogLine { time, level, target, message, site: Option<SiteId> }`; `site` comes from the nearest span with a `site` field (session work and each transfer job), and `LogBuffer::lines_for(scope)` returns that site's lines plus app-wide ones (`None`: app-wide only).
  It follows `settings.log.level`, defaulting to `info`. Protocol lines come from the
  `filecargo::protocol` target.
- If `FILECARGO_LOG_FILE` is set, the same lines are also appended to that file.

### Shutdown
`Quit` → `ConfirmQuit` if transfers are active → `Queue::shutdown` (persists) → session close
→ runtime stops → the final snapshot has `SessionState::Disconnected` and the UI exits.
`shutdown(timeout)` forces it after the timeout (default 3 s).

### Re-exports
Front-ends depend **only** on `filecargo-app-core`. It re-exports what they render or build:
- from `config`: `Site`, `Auth`, `Protocol`, `FtpMode`, `Folder`, `ServerTree`, `Node`, `TreeOp`, `Settings` and its parts, `ConflictRule`, `SecretString`
- from `remote-fs`: `Entry`, `EntryKind`, `RemotePath`, prompt types, `TrustDecision`, `SessionInfo`
- from `transfer`: `QueueSnapshot`, `QueueItem`, `ItemState`, `Outcome`, `TransferId`, `ConflictDecision`
- from `terminal`: `TerminalHandle`, `Key`, `Screen`

## Acceptance criteria

All tests use temp dirs, `MemoryStore` and a `SessionFactory` that serves `RootedFs`. No network.

1. Startup with a fresh dir gives a snapshot with an empty tree, `Disconnected`, and the local pane at the start dir.
2. A corrupt `servers.toml` sets `startup_error`; `ResetConfig` clears it and the tree is usable.
3. Connect flow: snapshots go `Connecting` → `Connected`, and the remote pane lists `site.remote_dir`. Connecting to a second site disconnects the first.
4. A password prompt appears as `PromptKind::Credential`. Answering continues the connect; cancelling gives `Failed` and no keychain write.
5. Navigation race: two quick `Navigate`s → the final pane shows the second path's entries only.
6. `Upload`/`Download` of a mixed selection (files + dir) enqueue the right items, complete, and refresh the target pane.
7. A transfer conflict surfaces as a `Conflict` prompt, and the answer reaches the queue.
8. `Delete` with `confirm_delete` asks first; `Confirm(false)` deletes nothing; `Confirm(true)` deletes recursively and refreshes.
9. Sorting and hiding: a table-driven test covers dirs-first, natural order, size/date order and dotfile hiding.
10. `Quit` with an active transfer prompts. Confirming persists the queue and the handle shuts down within the timeout.
11. Snapshots are published at most 30×/s during a 1,000-file transfer, and `AppState` clone cost doesn't grow with the entry count (the entries `Arc` is shared, asserted by pointer equality).

## Out of scope (v1)
Multiple simultaneous sessions or tabs, local trash/recycle bin, bookmarks, quick connect,
directory comparison or sync, search/filter in panes, remote file editing, persisting UI layout.

## v1.1 acceptance (tests: `local_ops`, `logging`, `transfers`, `queue_commands`)
- Local mkdir, rename, recursive delete (with confirm) and chmod change the disk and refresh the local pane.
- A log line inside a site's session or a transfer worker carries the `SiteId`; `lines_for` filters.
- Connected to A with items for A and B, `site_queue` holds A's only; disconnected, it is empty.
- Enqueue of 3 files leaves 3 held items and nothing transfers; `QueueStartHeld` transfers them.
- Pausing A leaves B running. Clear asks first: "no" keeps everything, "yes" empties A only.
- Queue commands while disconnected raise a notice and change nothing.
