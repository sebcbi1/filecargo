# Tasks: `transfer`

> Plan: [plan.md](plan.md) · Spec: [SPEC-transfer.md](../../SPEC-transfer.md)
> Definition of Done (every task): acceptance met; new behavior has failing-first tests; `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` clean; spec updated before any deviation; crate added to `default-members`.

### T1: Scaffold, model, persistence (S)
Create `crates/filecargo-transfer` with the spec's model types and serde for them.
`Store::load(path) -> (items, next_id)` and `Store::save(...)`. A corrupt or newer file → `.bak-<secs>` plus a warning, returning empty. Export `config::atomic_write` (rename of `fsio::write_atomic`, re-exported; spec note in SPEC-config).
- **Accept:** round-trip of every `ItemState` shape; `Active` saved as `Pending`; completed items dropped; corrupt and `version: 2` files backed up.
- **Verify:** `cargo test -p filecargo-transfer`; `cargo test -p filecargo-config`.
- **Files:** `Cargo.toml` (workspace), `crates/filecargo-transfer/{Cargo.toml,src/lib.rs,src/model.rs,src/store.rs}`, `crates/filecargo-config/src/{lib.rs,fsio.rs}`
- **Deps:** remote-fs Checkpoint A

### T2: Scheduler + single files (M)
Scheduler task (command mpsc → state), `Queue` handle API, worker pool honoring `max_concurrent`, per-site connection reuse plus 30 s idle close, upload and download of single files via `RemoteFs::upload/download`, `Completed{Transferred}` / `Failed`, snapshot publishing and coalesced `Changed`.
- **Accept:** 20 files with `max_concurrent = 3` → never more than 3 active (instrumented connector counts concurrent calls); all complete with identical bytes; `set_processing(false)` starts nothing new.
- **Verify:** `cargo test -p filecargo-transfer scheduler`
- **Files:** `src/{scheduler.rs,worker.rs,queue.rs}`, `tests/support/mod.rs` (test connector over `RootedFs`), `tests/scheduler.rs`
- **Deps:** T1

### T3: Directory items (M)
Lazy expansion: create the target dir, list the source, insert children right after the parent, `Outcome::Created`; skip symlinks with a log line; merge into existing dirs.
- **Accept:** AC1 (1,000 files / 50 dirs, both directions, SHA-256 equal); order of children follows the source listing; an existing target dir merges.
- **Verify:** `cargo test -p filecargo-transfer --test trees`
- **Files:** `src/scheduler.rs`, `src/worker.rs`, `tests/trees.rs`
- **Deps:** T2

### Checkpoint A: AC1 green (files and trees move correctly)

### T4: Conflict rules (M)
Stat the target before writing; apply the rule table; `Ask` → `AwaitingDecision` + `ConflictAsked`; `resolve`; `apply_to_all` override until idle; `Rename` naming `name (n).ext`.
- **Accept:** AC2: one test per rule (content + `Outcome`), plus the Ask/apply-to-all flow; directories never conflict.
- **Verify:** `cargo test -p filecargo-transfer --test conflicts`
- **Files:** `src/conflict.rs`, `src/worker.rs`, `src/scheduler.rs`, `tests/conflicts.rs`
- **Deps:** T3

### T5: Resume, retry, cancel (M)
`transferred` tracking; resume from the target's current size; automatic retries (2, back-off 2 s / 10 s, fresh connection); manual `retry` / `retry_all_failed`; `remove` aborts the active future within 1 s; `FlakyFs` test wrapper (fail at byte N, fail k-th call, broken-after-cancel).
- **Accept:** AC3 and AC6; a cancelled FTP-like (broken) connection is never reused.
- **Verify:** `cargo test -p filecargo-transfer --test resume` (paused time)
- **Files:** `src/worker.rs`, `src/scheduler.rs`, `tests/support/flaky.rs`, `tests/resume.rs`
- **Deps:** T4

### T6: Progress and timestamps (S)
Atomic progress counter per worker; scheduler samples it for rate EMA / ETA and `Totals`; `Changed` capped at 10 Hz; local mtime set after download (`File::set_modified`), remote `set_modified` after upload when supported.
- **Accept:** AC7; ETA hidden before 1 s; mtimes equal (±1 s) after a round trip on `RootedFs`.
- **Verify:** `cargo test -p filecargo-transfer --test progress`
- **Files:** `src/progress.rs`, `src/worker.rs`, `tests/progress.rs`
- **Deps:** T5

### Checkpoint B: AC1–3, 6, 7 green · human skim of the scheduler code

### T7: Persistence lifecycle (S)
Debounced saves (≤ 1/s) on every state change that affects persisted items; `shutdown` stops workers (active → pending with offset) and writes; `start` restores.
- **Accept:** AC4 and AC5.
- **Verify:** `cargo test -p filecargo-transfer --test restart`
- **Files:** `src/scheduler.rs`, `src/store.rs`, `tests/restart.rs`
- **Deps:** T5

### T8: Docker integration (S)
`--features integration` test: 1,000-file round trip against docker SFTP and FTP using a `Connector` over the real `remote_fs::connect` (test prompter auto-trusts the test host key/cert). Add the crate to the `cargo it` alias.
- **Accept:** AC8 green locally (OrbStack) and in the CI integration job.
- **Verify:** `docker compose -f tests/docker/compose.yml up -d --wait && cargo it`
- **Files:** `tests/integration.rs`, `.cargo/config.toml`
- **Deps:** T7, remote-fs Checkpoint C

### Checkpoint C: module complete
- [ ] All 8 AC green, coverage ≥ 80 % (`cargo llvm-cov -p filecargo-transfer`), CI green
- [ ] Hand-off notes in plan.md, SPEC-transfer status → done, SPEC.md map updated
- [ ] Human review
