# Spec: transfer

> Module id: `transfer` · Crate: `crates/filecargo-transfer` · Depends on: `remote-fs`, `config` · Status: **draft, awaiting review**
> Project-wide rules: [SPEC.md](SPEC.md). Plan: [tasks/transfer/plan.md](tasks/transfer/plan.md).

## Objective

The transfer queue behind the Queue / Completed / Failed tabs. It:
- schedules uploads and downloads, files and whole directories
- runs up to `max_concurrent` at a time, each on its own connection
- applies the conflict rules, resumes interrupted files and retries transient failures
- reports progress, speed and ETA
- persists unfinished work across restarts

It has no UI and no knowledge of prompts. It asks its owner (`app-core`) for connections and
conflict decisions through traits and events.

## Model

```rust
pub struct TransferId(u64);                 // monotonic, unique within queue.json
pub enum Direction { Upload, Download }

pub struct NewTransfer {
    pub site: SiteId,
    pub direction: Direction,
    pub local: PathBuf,                     // file or directory on this machine
    pub remote: RemotePath,                 // file or directory on the server
    pub is_dir: bool,
    pub size: Option<u64>,                  // known size of the source, for progress before stat
    pub conflict: Option<ConflictRule>,     // None = settings.transfers.default_conflict
}

pub struct QueueItem {
    pub id: TransferId,
    pub site: SiteId,
    pub direction: Direction,
    pub local: PathBuf,
    pub remote: RemotePath,
    pub is_dir: bool,
    pub size: Option<u64>,
    pub transferred: u64,                   // bytes on target written by us (resume offset)
    pub state: ItemState,
    pub attempts: u32,
    pub parent: Option<TransferId>,         // directory item that produced this one
}

pub enum ItemState {
    Pending,
    Active { started: SystemTime },
    AwaitingDecision { conflict: ConflictInfo },  // waiting for the owner to answer
    Completed { outcome: Outcome, finished: SystemTime },
    Failed { reason: String, retryable: bool, finished: SystemTime },
}
pub enum Outcome { Transferred, Skipped, Resumed, Renamed(String), Created /* dir */ }
pub struct ConflictInfo { pub source: Entry, pub target: Entry }
```

## Public API

```rust
/// Opens connections for workers. Implemented by app-core (real `remote_fs::connect` with the
/// shared `SessionTrust`) and by tests (a `RootedFs`, or a fault-injecting wrapper).
#[async_trait]
pub trait Connector: Send + Sync {
    async fn connect(&self, site: SiteId) -> Result<Arc<dyn RemoteFs>, TransferError>;
}

pub struct Queue { /* cheap-to-clone handle; the scheduler runs as a tokio task */ }

impl Queue {
    /// Loads `queue.json` (if any) and starts the scheduler (paused if `processing` is false).
    pub async fn start(connector: Arc<dyn Connector>, store: PathBuf, limits: QueueLimits)
        -> Result<(Queue, mpsc::UnboundedReceiver<QueueEvent>), TransferError>;
    pub fn enqueue(&self, items: Vec<NewTransfer>) -> Vec<TransferId>;
    pub fn resolve(&self, id: TransferId, decision: ConflictDecision);
    pub fn retry(&self, id: TransferId);
    pub fn retry_all_failed(&self);
    pub fn remove(&self, id: TransferId);    // cancels if active; a partial file is left as is
    pub fn clear_completed(&self);
    pub fn set_processing(&self, on: bool);  // pause/resume starting new items
    pub fn set_limits(&self, limits: QueueLimits);
    pub fn snapshot(&self) -> Arc<QueueSnapshot>;   // latest view, cheap
    /// Stops workers (active items return to Pending with their offset) and writes queue.json.
    pub async fn shutdown(self);
}

pub struct QueueLimits { pub max_concurrent: u8, pub default_conflict: ConflictRule }
pub struct ConflictDecision { pub rule: ConflictRule /* never Ask */, pub apply_to_all: bool }

pub enum QueueEvent {
    Changed,                                       // snapshot() has new content (coalesced)
    ConflictAsked { id: TransferId, conflict: ConflictInfo },
    ItemFinished { id: TransferId, ok: bool },     // for notifications / log lines
    Idle,                                          // nothing pending or active
}

pub struct QueueSnapshot {
    pub pending: Vec<QueueItemView>,               // incl. Active and AwaitingDecision, queue order
    pub completed: Vec<QueueItemView>,             // newest first, capped at 1,000
    pub failed: Vec<QueueItemView>,
    pub processing: bool,
    pub totals: Totals,                            // bytes done/total, rate, ETA for the whole queue
}
pub struct QueueItemView { pub item: QueueItem, pub rate: Option<f64> /* B/s */, pub eta: Option<Duration> }
```

## Behavior

### Scheduling
- FIFO. At most `max_concurrent` items are `Active`. Lowering the limit lets running items
  finish and doesn't start new ones until the count is under it.
- Each worker keeps one connection per site from `Connector` and reuses it for consecutive
  items of that site. An idle connection is closed after 30 s.
- A **directory item**, when started:
  1. creates the target directory (an existing one is fine)
  2. lists the source
  3. inserts one item per child **directly after itself**, keeping the source listing order
     (dirs and files)
  4. completes with `Outcome::Created`

  Symlinks are skipped with a log line; v1 doesn't follow them.
- Items for a site that no longer exists in config fail with `"site was deleted"`.

### Conflicts (target already exists)
| Rule | Behavior |
|---|---|
| `Ask` | Item → `AwaitingDecision`, `ConflictAsked` emitted, worker moves on to other items. The answer applies to this item; with `apply_to_all`, it also becomes the rule for every later conflict until the queue is empty. |
| `Overwrite` | Truncate and write from 0. |
| `OverwriteIfNewer` | Overwrite when source `modified` > target `modified` (both known); otherwise `Skipped`. Unknown times count as "not newer". |
| `Resume` | Target smaller than source → continue from target size (`Resumed`). Same size → `Skipped`. Larger → overwrite. |
| `Skip` | `Skipped`. |
| `Rename` | Write to the first free `name (1).ext`, `name (2).ext`, … (`Renamed(new_name)`). |

Directories never conflict; they merge.

### Resume and retry
- `transferred` counts bytes we wrote to the target. On retry, or after a restart, an item with
  `transferred > 0` resumes from the target's **current** size if that is ≤ the source size. A
  partial file we created is ours, so the conflict rule is **not** consulted again.
- Retryable failures (`FsError::is_retryable`, connect timeouts) retry automatically up to
  2 more times, with 2 s and 10 s back-off, on a fresh connection. Then the item is `Failed`.
  Non-retryable failures go straight to `Failed`.
- Manual `retry` resets `attempts` and makes the item `Pending` at the end of the queue.

### Timestamps
After a successful download the local file's mtime is set to the remote mtime. After an upload,
`set_modified` is called when the backend supports it (`Capabilities::set_mtime`). Failure to
set a time is logged, never fatal.

### Progress
- Backends report cumulative bytes; the queue coalesces them into `Changed` at most 10×/s.
- `rate` is an exponential moving average over about 5 s; `eta` = remaining ÷ rate, hidden
  until 1 s of data exists.

### Persistence (`<data>/queue.json`)
- Saved: items that are `Pending`, `Active` (saved as `Pending` with `transferred`),
  `AwaitingDecision` (saved as `Pending`) and `Failed`. **Completed items are not persisted.**
- Format: `{ "version": 1, "next_id": n, "items": [...] }`, written atomically (config's
  `atomic_write`), debounced to at most 1 write/s, plus a final write on `shutdown`.
- A corrupt or newer-version file is renamed to `queue.json.bak-<unix-secs>`; the queue starts
  empty and logs a warning. Losing the queue must not stop the app.

### Errors
```rust
pub enum TransferError { Connect(String), Fs(FsError), LocalIo(String), SiteDeleted, Persist(String) }
```

## Changes requested in other modules
- `config`: export `pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), ConfigError>`
  (today's private `fsio::write_atomic`), so the queue file gets the same guarantees.

## Acceptance criteria

1. With `RootedFs` as the "server": uploading and downloading a tree of 1,000 files over 50 directories gives byte-identical trees (SHA-256 of each file). `max_concurrent` is never exceeded (instrumented connector).
2. Each conflict rule has a test showing the resulting target content and `Outcome`. `Ask` + `apply_to_all` answers the remaining conflicts without new `ConflictAsked` events.
3. Fault injection: a connection drop at byte N of a 10 MB file retries automatically, resumes at the target's current size (not 0), and the final SHA-256 matches. After 3 consecutive failures the item is `Failed` with the reason.
4. Restart: `shutdown` with pending, active (partial) and failed items, then `start` again, restores them. The partial one resumes from its offset.
5. A corrupt `queue.json` is backed up, the queue starts empty, and a warning is logged.
6. `remove` on an active item cancels it within 1 s and frees its worker slot.
7. `Changed` events are emitted at most 10×/s under a 1,000-small-file transfer (counted in test).
8. Integration (`--features integration`): the 1,000-file round trip passes against the docker SFTP and FTP servers with `max_concurrent = 4`.

## Out of scope (v1)
Speed limits, per-item priorities and drag-to-reorder, scheduled transfers, verifying with
remote checksums, following symlinks, FXP, persisting the Completed list.
