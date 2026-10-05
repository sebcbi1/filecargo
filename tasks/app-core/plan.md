# Implementation Plan: `app-core` module

> Spec: [SPEC-app-core.md](../../SPEC-app-core.md) · Tasks: [todo.md](todo.md) · Status: **awaiting review** · 2026-10-05
> Starts after `transfer` Checkpoint B and `terminal` complete (their public APIs are stable).

## Overview
`crates/filecargo-app-core`: an actor (one tokio task) that owns `ConfigStore`, the browsing
`Session`, the transfer `Queue`, the `TerminalHandle` and the prompt queue. It publishes
`Arc<AppState>` snapshots through a `watch` channel. Every behavior is tested headlessly with a
`SessionFactory` serving `RootedFs`, so both UIs can later be built on a verified core.

## Architecture Decisions
| Decision | Rationale |
|---|---|
| **Actor + `watch<Arc<AppState>>`** (not shared mutable state) | UIs never lock app state; a snapshot is always internally consistent; tokio::sync works on gpui's executor too, so no runtime bridge is needed. |
| **App owns its tokio runtime** (multi-thread, 2 workers) | Front-ends stay runtime-agnostic. The TUI may run its loop on `handle.block_on`, and the GUI never touches tokio. |
| **Background work = spawned tasks that report back as internal messages** (`Msg::Listed { generation, .. }`) | The actor never awaits network I/O, so it stays responsive. A stale `generation` drops late results (navigation race). |
| **Snapshot publishing is coalesced to ≤ 30/s** | Bounds UI redraw load during big transfers; queue snapshots are already ≤ 10 Hz. |
| **Prompts are a FIFO; `Prompter` impl = send `Msg::Prompt { kind, oneshot }` to the actor** | One mechanism for remote-fs prompts, conflicts and confirmations. Both UIs render a single `Option<Prompt>`. |
| **UI owns selection; commands carry names** | Each toolkit selects differently (mouse vs `Space`). app-core resolves names against the current pane entries and ignores unknown names. |
| **Natural sort** implemented locally (~40 lines) | Avoids a dependency for one comparator; table-driven tests pin the behavior. |
| **Log layer = custom `tracing_subscriber::Layer`** writing into `LogBuffer` (ring, `parking_lot`-free std `Mutex`) | Both UIs read the same lines; binaries install it with `app_core::logging::init`. |

### Dependencies (new)
`tokio` (rt-multi-thread, sync, time, macros), `async-trait`, `tracing`, `tracing-subscriber`
(registry, fmt for the optional file); dev: `tempfile`.

## Dependency Graph
```
T1 actor/runtime/snapshots ─┬─ T2 logging
                            ├─ T3 tree + import commands
                            ├─ T4 prompts ── T6 connect + remote pane ── T7 remote ops
                            └─ T5 local pane + sorting ──┘                    │
                                                         T8 transfers wiring ─┴─ T9 terminal + quit
```

## Task List
### Phase 1: Skeleton
- [x] T1: Crate, `App::start`, actor loop, `watch` snapshots, coalescing, `shutdown`; startup with config errors + `ResetConfig` (M)
- [x] T2: Log layer, ring buffer, `FILECARGO_LOG_FILE` (S)
- [x] T3: Tree commands, `ImportFileZilla` (→ `Message` with the report), `SetSitePassword` (S)
### Checkpoint A: AC1, AC2 green; snapshot API reviewed (UI authors depend on it)
### Phase 2: Browsing
- [x] T4: Prompt queue, `Prompter` impl, `Answer` routing, stale-id handling (M)
- [x] T5: Local pane: listing, navigation, sort (natural, dirs first), hidden filter, generation race (M)
- [x] T6: Connect flow via `SessionFactory`, `SessionState` steps, remote pane, auto-reconnect once (M)
- [x] T7: Remote ops: mkdir, rename, delete with confirm (recursive), chmod (S)
### Checkpoint B: AC3, AC4, AC5, AC8, AC9 green
### Phase 3: Transfers and terminal
- [x] T8: Queue wiring: `Connector` adapter over `SessionFactory`, `Upload`/`Download`, conflict prompts, queue commands, pane refresh debounce, snapshot rate (M)
- [ ] T9: Terminal commands, `TerminalState`, `Quit` + `ConfirmQuit` + shutdown timeout, re-exports module (S)
### Checkpoint C: all 11 AC green, coverage ≥ 80 %, human review

## AC Traceability
| AC | Task | AC | Task | AC | Task |
|---|---|---|---|---|---|
| 1 | T1 | 5 | T5 | 9 | T5 |
| 2 | T1 | 6 | T8 | 10 | T9 |
| 3 | T6 | 7 | T8 | 11 | T8 |
| 4 | T4, T6 | 8 | T7 | | |

## Risks and Mitigations
| Risk | Impact | Mitigation |
|---|---|---|
| Snapshot API churn once UIs start | High | Checkpoint A reviews `AppState`/`Command`; the TUI is built first and is the first consumer, and changes after that go through the spec. |
| Deadlocks between the actor and the prompter (connect awaits a prompt the actor must publish) | Med | Connect runs in a spawned task; the actor only forwards prompt messages; a test connects with a prompt pending and quits. |
| Re-exported types leak lower-crate churn into UIs | Med | Re-exports in one `pub mod prelude` file; reviewed at each checkpoint. |
| `tracing` global subscriber conflicts in tests | Low | `LogBuffer` layer is attachable per test via `with_default`; `logging::init` is only called by binaries. |

## Open Questions
None.

## Hand-off Notes
_Appended per task during implementation._

### T1 Actor, runtime, snapshots (done)
- `App::start(StartOptions) -> AppHandle` builds the 2-worker runtime, opens the `ConfigStore` **synchronously** (so `handle.state()` is complete the moment `start` returns), and spawns the actor. The actor `select!`s the message queue and a publish timer: state is mutated in place, `changed()` marks it dirty, and a snapshot (`Arc<AppState>` clone) is published at most every 33 ms (`SNAPSHOTS_PER_SECOND = 30`), plus once at shutdown.
- **The whole public vocabulary is defined now** (`AppState`, `SessionState`, `Pane`, `Sort`, `Prompt*`, `Notice`, `TerminalState`, `Command`, `LogBuffer`, `SessionFactory`) so UI authors can read it at Checkpoint A; commands not implemented yet are logged and ignored until their task lands. Config problems never fail `start`: `startup_error` is set and the tree is empty. `ResetConfig` backs up `servers.toml` (via `ConfigStore::reset`) **and** an unreadable `settings.toml` (`ConfigStore::reset` only handles the former), then reloads. A missing OS keychain yields a one-time warning notice (`KeyringStore::native()` is tried directly so the reason is known). `UpdateSettings` is already wired.
- `AppHandle::shutdown(timeout)`: idempotent (the runtime sits in a shared `Mutex<Option<_>>`), asks the actor to finish, then `Runtime::shutdown_timeout`, so a never-ending async task or a stuck blocking task cannot hold it past the timeout (tested). It must be called from outside the runtime.
- Tests are plain `#[test]`s: `Fixture::wait_for` blocks on the app's own runtime handle. 7 tests (fresh start, configured/bad start dir, corrupt `servers.toml` / `settings.toml` + reset, newer-version file untouched, shutdown with hung tasks, ≤ 35 snapshots/s under a burst of 200 changes).
- Fixed on the way: `tokio::time` values must be created inside the runtime (`runtime.enter()` in `start`, `block_on(async { timeout(..) })` in `shutdown`).

### T2 Logging (done)
- `logging.rs`: `LogBuffer` (ring of 5,000 `LogLine { time, level: config::LogLevel, target, message }`, a generation counter, and the **live level** as an atomic), `LogLayer` (a `tracing_subscriber::Layer`; formats `message` plus extra fields as `key=value`; optional file sink), `init_logging(&buffer)` for binaries (installs a registry and honors `FILECARGO_LOG_FILE`; returns `false` if a global subscriber already exists). `App::start` seeds the level from the settings and `UpdateSettings` changes it live; `AppState::log_generation` is refreshed at every publish.
- The `log` crate is **not** bridged, by design (see the module doc): the FTP library's `PASS` traces are compiled out and nothing from dependencies below our own `tracing` events is shown.
- 6 tests (target/level/fields, ring overflow, level filtering and live change through `UpdateSettings`, snapshot generation, file append).

### T3 Tree and import commands (done) → Checkpoint A reached (AC1, AC2)
- `tree.rs`: `Tree(op)` → `ConfigStore::apply` (a failure leaves the tree unchanged and queues **one** `Message` prompt with the config error), `ImportFileZilla` (folder `FileZilla import <UTC date>`, dated with a small civil-date function, unit-tested; result shown as a `Message` with "Imported N sites…", the skipped sites with reasons and the passwords not imported; `Warning` level when anything was skipped, `Error` for an unreadable file), `SetSitePassword` → keychain only (verified: not in any config file nor in the `Debug` output of the snapshot).
- The `ConfigStore` is synchronous and takes a cross-process lock per write; the actor calls it directly (a few KB of TOML).
- With the fixture: **6 imported, 1 skipped**, six sites in the tree. 5 tests.
- Review note for the UI authors (Checkpoint A): the state/command vocabulary is in `state.rs` / `command.rs`; `Pane.entries` excludes `..`; prompts are a FIFO (the answer path arrives with T4, until then a `Message` simply stays).

### T4 Prompts (done)
- `prompt.rs`: `ActorPrompter` implements `remote_fs::Prompter` by sending `Msg::Prompt(PromptRequest { kind, reply })` to the actor and awaiting the oneshot; `Core::request_prompt` queues it (FIFO) and remembers the reply channel; `Command::Answer` is accepted **only for the prompt that is showing (the front of the queue)** — unknown or stale ids (including an id answered twice) are ignored. A wrong kind of answer is a refusal (`None` for credentials, `Reject` for trust); a prompt still pending at shutdown resolves as cancelled because its reply channel is dropped.
- New public `AppHandle::prompter()` (not in the spec sketch): the prompter every session uses, for embedders and tests that open sessions themselves.
- Lesson for UI authors and tests: a snapshot can lag one publish behind an `Answer`, so after answering, wait for a prompt with a **different id** instead of "any prompt" (the first version of the test answered a stale prompt and hung; tests now join tasks with a 5 s timeout via `Fixture::join`).
- 5 tests: two concurrent requests shown one at a time in order, cancel → `None`, wrong answer kind → refusal, unknown/stale ids ignored, messages dismissed, shutdown with a pending prompt.

### T5 Local pane and sorting (done)
- `pane.rs`: a listing runs in a spawned task (`canonicalize` + `local::read_dir`, dotfiles and the Windows hidden attribute filtered according to `settings.ui.show_hidden`) and reports back as `Msg::LocalListed { seq, result }`. Every request bumps `local_seq`; **a result whose `seq` is no longer the newest is discarded** (navigation race, AC5: tested by navigating into a 20,000-entry directory and then a tiny one immediately, so the big listing finishes last). On success the pane gets the canonical path (Windows `\\?\` prefix stripped), sorted entries and `generation += 1`; on failure the **old path and entries stay**, `error` is set, `loading` is cleared. `Navigate` accepts absolute, relative (`docs/../src/.`) and `~` / `~/x`; `Up`, `Refresh`, `SetSort` (re-sorts the entries in hand, bumps `generation`, no new listing). The first listing starts when the actor starts; toggling `show_hidden` in the settings re-lists.
- `sort.rs`: `natural_cmp` (case-insensitive, digit runs compare as numbers, leading zeros as a final tie-break so the order is total) and `sort_entries`: directories first, then the key; **descending reverses only the primary key, ties are always by name ascending** (so directories, all of size 0, stay in name order under a size sort). Unit tests are the table-driven AC9 (dirs first for every key × direction, natural order, unknown times first, dotfile hiding).
- `AppState.local.entries` is an `Arc<[Entry]>` shared between snapshots (asserted by pointer equality after an unrelated change).
- 7 integration tests + 4 unit tests. The remote pane reuses this machinery in T6.

### T6 Connect and remote pane (done)
- `Connect(site)` (`session.rs`): drops any current session (closed in the background; transfers keep their own connections), bumps `connect_epoch`, publishes `Connecting { step }` and spawns **one task** that calls `SessionFactory::connect` with a clone of the shared `ConnectContext` (one `SessionTrust` for the whole app, timeouts from the settings, `ActorPrompter` as prompter), then picks the start directory (`site.remote_dir` if it is a directory, else the server's home) and lists it. The task reports `ConnectStep::Listing` and finally `Connected`. Steps visible to the UI: `Connecting`, `Authenticating` (as soon as a credential prompt is shown), `Listing`. A result whose epoch is stale (a newer connect or a disconnect won) is dropped and its session closed. The local pane also switches to `site.local_dir`.
- Failure: `Failed { error }`, a log line, and a `Message` prompt **only** for auth / key / host-key / certificate / TLS errors; refused or timed-out connections and a cancelled prompt just fail quietly.
- **Lost connection** (a remote listing fails with a retryable `FsError`): the session becomes `Failed`, the pane stays, and the **next remote command** (`Navigate` / `Up` / `Refresh`) reconnects once and then replays that command (`after_connect`). If that reconnect fails nothing retries automatically: the next command only raises a "Not connected" notice.
- Remote pane: same discipline as the local one (`remote_seq`, stale results dropped, old entries kept on error, `generation` bumps), paths normalised with `RemotePath::parse` (`..` above the root is an error shown in the pane), `~` = the server's home, `SetSort` re-sorts in hand.
- Test harness: `TestFactory` serves a directory as a "server" (optional password asked through the prompter, `remember` honoured, `fail_next`, per-connection kill switch, connect/close counters). 11 tests: states, remote_dir and home fallback, second site closes the first, password prompt + authenticating step, cancel (no keychain write, no popup), remember, message-vs-quiet failures, reconnect once, no automatic second retry, navigation/up/errors/sort, no-session notice.

### T7 Remote file operations (done) → Checkpoint B reached (AC3–5, 8, 9)
- `ops.rs`: `Mkdir`, `Rename`, `Delete`, `Chmod` act on **names of the remote pane's current directory**; a name must be one plain path component (`valid_name`), unknown names are ignored, an empty selection does nothing. Each runs in a spawned task and reports `Msg::OpDone`; success refreshes the pane, failure raises an error `Notice` ("Could not create the folder "sub": already exists: ...") plus a `warn` log line, never touches the pane's contents (it is refreshed anyway, since a partial failure may have changed the server), and a retryable error also marks the session lost (reconnect on the next command).
- `Delete` asks `ConfirmDelete { pane, names, recursive }` first when `settings.ui.confirm_delete`; `recursive` is true when any selected entry is a directory. App-raised prompts keep their follow-up in `PromptAction` (`Core.actions`); only `Confirm(true)` runs it, anything else drops it. Directories are removed with `remove_all`, files and symlinks with `remove_file`.
- 7 tests (mkdir/rename + refresh, decline deletes nothing, confirm deletes a directory with its contents, no-confirm setting, unknown names, chmod 0o600 on both files, error notices incl. invalid names).

### T8 Transfers wiring (done)
- `transfers.rs`: the queue starts with the actor (`Queue::start` on `paths.queue()`, so items left by the previous run reappear); its events are forwarded as `Msg::Queue`. `QueueConnector` adapts the app's `SessionFactory` to `transfer::Connector` with a clone of the **shared `ConnectContext`** (same `SessionTrust`, so workers never re-prompt for answered questions) and a `Shared { tree, timeouts }` view that the actor keeps in step (`sync_shared` after every tree/settings change; a deleted site fails the item with "site was deleted"). `UpdateSettings` also calls `queue.set_limits`.
- `Upload` / `Download { names }` build one `NewTransfer` per name found in the **source pane's entries** (unknown names, symlinks and special files are skipped; `is_dir` and size come from the entry) between `local.path/name` and `remote.path/name`; without a session they raise a notice.
- `QueueEvent`s: `Changed` / `ItemFinished` / `Idle` refresh `AppState.queue` (the queue's own 10 Hz cap carries through); `ConflictAsked` becomes a `PromptKind::Conflict` prompt whose answer (`PromptAnswer::Conflict(decision)`) is passed to `queue.resolve`; any other answer (dismiss) is read as **skip**, so a closed dialog never leaves an item waiting forever. Failed items log their reason.
- **Pane refresh debounce**: `RefreshSchedule` (due time = max(now, last refresh + 1 s)) per pane; a finished item refreshes the pane whose directory it landed in (remote for uploads into the shown directory, local for downloads), and `Idle` refreshes both so the last files of a batch always show up; the actor's wake-up includes the next due time.
- Queue commands (`QueueRetry`, `QueueRetryFailed`, `QueueRemove`, `QueueClearCompleted`, `QueueSetProcessing`) are thin calls.
- AC11 verified with a 1,000-file upload: ≤ 30 snapshots/s, and whenever the local pane's `generation` is unchanged between snapshots its entries are the **same `Arc`** (pointer equality). 8 tests (+ `Entry` re-exported). The test harness timeout was raised to 10 s after one run, taken while a compile was hogging the machine, missed a 5 s wait.
