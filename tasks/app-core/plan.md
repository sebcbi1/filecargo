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
- [ ] T2: Log layer, ring buffer, `FILECARGO_LOG_FILE` (S)
- [ ] T3: Tree commands, `ImportFileZilla` (→ `Message` with the report), `SetSitePassword` (S)
### Checkpoint A: AC1, AC2 green; snapshot API reviewed (UI authors depend on it)
### Phase 2: Browsing
- [ ] T4: Prompt queue, `Prompter` impl, `Answer` routing, stale-id handling (M)
- [ ] T5: Local pane: listing, navigation, sort (natural, dirs first), hidden filter, generation race (M)
- [ ] T6: Connect flow via `SessionFactory`, `SessionState` steps, remote pane, auto-reconnect once (M)
- [ ] T7: Remote ops: mkdir, rename, delete with confirm (recursive), chmod (S)
### Checkpoint B: AC3, AC4, AC5, AC8, AC9 green
### Phase 3: Transfers and terminal
- [ ] T8: Queue wiring: `Connector` adapter over `SessionFactory`, `Upload`/`Download`, conflict prompts, queue commands, pane refresh debounce, snapshot rate (M)
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
