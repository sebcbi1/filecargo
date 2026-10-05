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
    pub queue: Arc<QueueSnapshot>,
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
    // remote file operations
    Mkdir { name: String }, Rename { from: String, to: String },
    Delete { names: Vec<String> },               // confirm prompt when settings.ui.confirm_delete
    Chmod { names: Vec<String>, mode: u32 },
    // transfers (names are entries of the source pane's current dir)
    Upload { names: Vec<String> }, Download { names: Vec<String> },
    QueueRetry(TransferId), QueueRetryFailed, QueueRemove(TransferId),
    QueueClearCompleted, QueueSetProcessing(bool),
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
- Local panes are browse-only in v1. Mkdir, rename, delete and chmod apply to the remote pane.

### Transfers
- `Upload { names }`: for each local entry, a `NewTransfer` from `local.path/name` to
  `remote.path/name`, with `is_dir` from the entry. `Download` is the mirror image.
- The `Connector` given to the queue calls `remote_fs::connect` with the same `ConnectContext`
  (shared `SessionTrust`), so workers never re-prompt for what the user already answered.
- When an upload into the remote pane's current directory completes, that pane refreshes
  (debounced to 1/s). Same for downloads into the local pane.

### Terminal
- Available only for SFTP sessions (`TerminalState::NotAvailable` otherwise).
- Opened lazily on `TerminalOpen` (sent by the UI when the tab is first shown, with its size).
  Closed on disconnect. `TerminalView` exposes the `terminal::TerminalHandle` for rendering
  and key encoding.

### Log
- A `tracing` layer feeds a ring buffer of 5,000 `LogLine { time, level, target, message }`.
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
Multiple simultaneous sessions or tabs, local file operations, bookmarks, quick connect,
directory comparison or sync, search/filter in panes, remote file editing, persisting UI layout.
