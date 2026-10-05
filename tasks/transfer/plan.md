# Implementation Plan: `transfer` module

> Spec: [SPEC-transfer.md](../../SPEC-transfer.md) · Tasks: [todo.md](todo.md) · Status: **awaiting review** · 2026-10-05
> Starts after `remote-fs` Checkpoint A (trait, `RootedFs` and contract suite exist). Can run in parallel with `terminal`.

## Overview
`crates/filecargo-transfer`: a tokio scheduler owning the queue state, N worker tasks, and a
debounced persister. Everything except the last task is tested against `RootedFs` through a
test `Connector`, so no network or docker is needed until the integration task.

## Architecture Decisions
| Decision | Rationale |
|---|---|
| **Single scheduler task owns all state**; workers report back through an mpsc | No locks around the queue; ordering and invariants are easy to reason about and test. `snapshot()` reads an `ArcSwap`-style `watch` value the scheduler publishes. |
| **Workers are tokio tasks holding one connection per site**, closed after 30 s idle | Mirrors FileZilla (one connection per transfer slot). FTP can't multiplex anyway. |
| **Cancellation = abort the worker's transfer future** (`AbortHandle`) | `remote-fs` defines dropping as cancellation; FTP marks itself broken, and the worker drops that connection. |
| **Directory expansion happens lazily when the dir item starts** | Enqueueing a huge tree returns immediately; children slot in right after their parent, preserving order. |
| **Fault injection = `FlakyFs` wrapper in `tests/support`** that fails after N bytes or on the k-th call | Exercises the real retry/resume code against `RootedFs`. No mocks of the scheduler. |
| **Progress throttling in the scheduler** (10 Hz `Changed`), rate = EMA (τ ≈ 5 s) | One place controls UI load; the worker callback stays a cheap atomic store. |
| **`queue.json` via serde_json + config's `atomic_write`** | Same durability as the config files; JSON because items nest paths and enums. |
| **Time comes from an injectable `Clock`** (real or `tokio::time` paused) | Back-off and debounce tests run instantly with `start_paused = true`. |

### Dependencies (new, versions pinned at scaffold)
`async-trait`, `serde_json`, `tokio` (rt, sync, time, fs, macros), `tracing`; dev: `tempfile`,
`sha2` (tree checksums), `proptest` (queue invariants).

## Dependency Graph
```
T1 model + persistence ── T2 scheduler + single file ── T3 directories ─┐
                                                                        ├─ T4 conflicts ─ T5 resume/retry/cancel ─ T6 progress + mtimes
                                                                        │
                                                       T7 restart/persist integration (needs T2, T5) ── T8 docker integration
```

## Task List
### Phase 1: Core path
- [x] T1: Crate scaffold, model types, `queue.json` load/save, `config::atomic_write` export (S)
- [x] T2: Scheduler + workers, single-file upload/download, snapshot + `Changed` (M)
- [x] T3: Directory items (lazy expansion, merge), 1,000-file round trip (M)
### Checkpoint A: files and trees move correctly (AC1) — reached
### Phase 2: Robustness
- [x] T4: Conflict rules + `Ask` / `resolve` / `apply_to_all` (M)
- [ ] T5: Resume, automatic retry with back-off, manual retry/remove/cancel, `FlakyFs` (M)
- [ ] T6: Progress rate/ETA, 10 Hz throttle, mtime preservation (S)
### Checkpoint B: AC2, AC3, AC6, AC7 green
### Phase 3: Durability and real servers
- [ ] T7: Debounced persistence, `shutdown`, restart restore, corrupt file backup (S)
- [ ] T8: Integration round trip vs docker SFTP and FTP, `max_concurrent = 4` (S)
### Checkpoint C: module complete (all 8 AC), coverage ≥ 80 %, human review

## AC Traceability
| AC | Task | AC | Task |
|---|---|---|---|
| 1 | T3 | 5 | T7 |
| 2 | T4 | 6 | T5 |
| 3 | T5 | 7 | T6 |
| 4 | T7 | 8 | T8 |

## Risks and Mitigations
| Risk | Impact | Mitigation |
|---|---|---|
| Flaky timing tests (throttle, back-off, cancel within 1 s) | Med | Paused tokio time + injectable clock; no real sleeps in unit tests. |
| FTP backends break after cancel and workers reuse them | Med | Worker drops the connection on any cancel or `Disconnected`; covered by a `FlakyFs` "broken after cancel" mode. |
| Partial-file resume on a target that changed underneath | Low | Resume only when current target size ≤ source size and `transferred > 0`; otherwise restart from 0 and log. |
| 1,000-file test slow on Windows CI (fsync, AV scanning) | Low | Files are tiny, there's no fsync in `RootedFs` writes, and the size is a const (CI may lower it via env). |

## Open Questions
None. Retry count (2) and back-off (2 s, 10 s) are constants in v1, not settings.

## Hand-off Notes
_Appended per task during implementation._

### T1 Scaffold, model, persistence (done)
- `config::atomic_write` is now exported (`fsio::write_atomic` made `pub` and re-exported), as the spec requested.
- `QueueItem` gained a pub field **`conflict: Option<ConflictRule>`** (the per-item rule from `NewTransfer`, needed after a restart); spec updated. `ItemState` carries `#[allow(clippy::large_enum_variant)]` (two `Entry` values in `AwaitingDecision`).
- `queue.json` (`store.rs`): `{ "version": 1, "next_id", "items": [...] }`; `RemotePath` is stored as a string and an item whose remote path no longer parses is dropped with the rest loaded; an item whose **local path is not UTF-8** is not saved (warning) rather than failing the whole save. `next_id` is raised above any loaded id. Corrupt / newer-version → `queue.json.bak-<unix-secs>`, warning, empty queue.
- 8 unit tests (every state shape, Active/Awaiting → Pending with offset, Completed dropped, corrupt, version 2, invalid remote path, id reuse).

### T2 Scheduler + single files (done)
- `queue.rs` (public API types + `Queue` handle), `scheduler.rs` (the one task that owns all state), `worker.rs` (the file I/O of one item). The handle allocates `TransferId`s itself (`AtomicU64`, seeded from `queue.json`) so `enqueue` can return them synchronously.
- Scheduler loop: `select!` over commands, finished jobs and one computed wake-up time (progress sampling 100 ms while anything runs, retry timers, idle-connection expiry, 1 s save debounce); no timer ticks while idle, so paused-time tests work. `Changed` and the published snapshot are rate-limited to 10 Hz; the snapshot is also published right before `Idle` so a test (or UI) that waits for `Idle` sees the final state.
- One job = one `tokio::spawn`ed task that takes a pooled connection for its site (or `Connector::connect`s one) and reports back with `Finished`. A connection goes back to the pool only after a **successful** job (anything else drops and closes it in the background); the pool closes connections idle > 30 s. Cancelling = aborting the task (the connection is dropped, never reused).
- `ProgressCell { offset, written }`: the worker stores bytes, the scheduler samples them; `item.transferred = offset + written`.
- A freshly started empty queue does not emit `Idle`.
- Directories (T3), conflicts (T4), retries (T5), rates (T6) and `shutdown` persistence details (T7) are not in yet; `Queue::resolve` and the conflict types arrive with T4. `tests/support`: `TestConnector` + `CountingFs` over `RootedFs` (counts in-flight transfers and connects, optional per-transfer delay).
- 6 tests: concurrency cap (peak == 3, never > 3), byte-identical uploads/downloads, connection reuse, pause/resume, failing item keeps the queue going, unknown site → "site was deleted".

### T3 Directory items (done)  → Checkpoint A reached
- A directory item creates the target (an existing directory merges; an existing *file* → `not a directory` failure), lists the source and inserts its children **right after itself, in listing order** (`Scheduler::expand` uses `Vec::splice`), then completes as `Outcome::Created`. Children inherit site, direction and conflict rule and get `parent = Some(dir id)`; ids come from the same shared counter.
- Skipped with a log line: symlinks, special files, and **unsafe names** (`safe_name`: exactly one normal path component, no `/`, `\`, NUL, `.`/`..`, no drive prefixes), so a hostile server cannot make a download write outside its target.
- AC1 verified for both directions: 1,000 files over 50 directories (5 × (1 + 9)), SHA-256 of every file and the directory list equal, peak concurrency ≤ 4. 1,051 `ItemFinished` events (files + 50 dirs + root); `completed` correctly stays capped at 1,000.
- `Harness` counts `ItemFinished` events (`finished_ok` / `finished_failed`) while waiting for `Idle`. 8 tree tests + 1 unit test.

### T4 Conflict rules (done)
- `conflict.rs`: `decide(rule, source, target) -> Decision` (pure, unit-tested incl. unknown times) and `numbered_name` (`report.txt` → `report (1).txt`, `archive.tar.gz` → `archive.tar (1).gz`, `.profile` → `.profile (1)`). The worker stats both ends first; a missing target is no conflict, a **partial file we wrote** (`transferred > 0`, target no larger than the source) is resumed without consulting the rule, a target that outgrew the source restarts from 0.
- `Ask` → `JobResult::NeedsDecision` → the item becomes `AwaitingDecision`, `ConflictAsked` is emitted and the worker slot is free for other items. `Queue::resolve` + `ConflictDecision { rule, apply_to_all }`. Rule precedence: the owner's answer for that item → the item's own rule → the queue default; an `Ask` falls back to the `apply_to_all` override, which lives until the queue is empty (`Idle`).
- **Race found by the tests and fixed:** an item already running with rule `Ask` when the owner answers "apply to all" would report its conflict afterwards and ask again; the scheduler now applies the override to such a late `NeedsDecision` instead of asking.
- `Rename` makes the item **follow its new name** (`ProgressCell::retarget`, applied by the scheduler on completion or failure), so a retry resumes the renamed partial instead of conflicting again. The existing file is never touched.
- The snapshot is published right before `ConflictAsked`, so event and snapshot agree (otherwise it would be up to 100 ms stale).
- 9 integration tests (each rule in both directions, newer/older/equal times, resume shorter/equal/longer, rename (1)/(2), ask with other items continuing, apply-to-all for the next and the already-waiting conflicts, `Ask` answers ignored) + 4 unit tests.
