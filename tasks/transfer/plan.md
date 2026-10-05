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
- [ ] T1: Crate scaffold, model types, `queue.json` load/save, `config::atomic_write` export (S)
- [ ] T2: Scheduler + workers, single-file upload/download, snapshot + `Changed` (M)
- [ ] T3: Directory items (lazy expansion, merge), 1,000-file round trip (M)
### Checkpoint A: files and trees move correctly (AC1)
### Phase 2: Robustness
- [ ] T4: Conflict rules + `Ask` / `resolve` / `apply_to_all` (M)
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
