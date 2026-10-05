# Tasks: `app-core`

> Plan: [plan.md](plan.md) · Spec: [SPEC-app-core.md](../../SPEC-app-core.md)
> Definition of Done: as in [tasks/transfer/todo.md](../transfer/todo.md). Every test uses temp dirs, `MemoryStore`, and the `RootedFs` `SessionFactory` from `tests/support`.

### T1: Actor, runtime, snapshots (M)
`StartOptions`, `App::start`, `AppHandle { send, state, log, shutdown }`, actor loop with internal `Msg`, coalesced publishing (≤ 30/s), startup with config errors → `startup_error`, `ResetConfig`.
- **Accept:** AC1, AC2; `shutdown` is idempotent and returns within its timeout even if a task hangs (test with a never-ending fake).
- **Verify:** `cargo test -p filecargo-app-core --test startup`
- **Files:** `crates/filecargo-app-core/{Cargo.toml,src/lib.rs,src/app.rs,src/state.rs,src/command.rs}`, `tests/support/mod.rs`, `tests/startup.rs`
- **Deps:** transfer Checkpoint B, terminal complete

### T2: Logging (S)
`LogBuffer` (ring 5,000, generation counter), `LogLayer`, `logging::init(level, file)`, level changes on `UpdateSettings`.
- **Accept:** lines from `filecargo::protocol` appear with target and level; the ring drops oldest; the file sink appends when the env var is set.
- **Verify:** `cargo test -p filecargo-app-core logging`
- **Files:** `src/logging.rs`, `tests/logging.rs`

### T3: Tree and import commands (S)
`Command::Tree(op)` → `ConfigStore::apply`; errors → `Message` prompt; `ImportFileZilla` builds the dated folder name and shows the `ImportReport` as a `Message`; `SetSitePassword` → secret store.
- **Accept:** add/rename/delete reflected in the next snapshot; an invalid op leaves the tree unchanged and shows one `Message`; import of the config fixture shows a report with 6 imported, 1 skipped.
- **Verify:** `cargo test -p filecargo-app-core --test tree`
- **Files:** `src/app.rs`, `src/tree.rs`, `tests/tree.rs`

### Checkpoint A: AC1–2 · review `AppState` / `Command` with the human before UI work depends on them

### T4: Prompts (M)
Prompt FIFO, `PromptId`, `ActorPrompter: remote_fs::Prompter`, `Answer` routing with stale/unknown ids ignored, `Dismiss`.
- **Accept:** two concurrent prompt requests are shown one at a time in order; answering the first publishes the second; a cancelled credential prompt resolves to `None`.
- **Verify:** `cargo test -p filecargo-app-core --test prompts`
- **Files:** `src/prompt.rs`, `src/app.rs`, `tests/prompts.rs`

### T5: Local pane and sorting (M)
`Pane<PathBuf>`, `Navigate` (absolute/relative, `~`), `Up`, `Refresh`, `SetSort`, natural compare, dirs-first, hidden filter (dotfiles; Windows hidden attribute), generation-guarded listing tasks.
- **Accept:** AC5 (race), AC9 (table-driven ordering/hiding); a listing error keeps old entries and sets `error`.
- **Verify:** `cargo test -p filecargo-app-core --test panes`
- **Files:** `src/pane.rs`, `src/sort.rs`, `tests/panes.rs`

### T6: Connect + remote pane (M)
`SessionFactory` (default = `remote_fs::connect`), `ConnectContext` with shared `SessionTrust`, `SessionState` steps, remote pane at `remote_dir` or home, local dir switch, disconnect-before-connect, one automatic reconnect on `Disconnected`.
- **Accept:** AC3, AC4; `Failed` after a factory error shows a `Message` for auth/TLS errors only.
- **Verify:** `cargo test -p filecargo-app-core --test session`
- **Files:** `src/session.rs`, `src/app.rs`, `tests/session.rs`

### T7: Remote file operations (S)
`Mkdir`, `Rename`, `Delete` (+ `ConfirmDelete` when `confirm_delete`, recursive via `remove_all`), `Chmod`; refresh after each.
- **Accept:** AC8; errors surface as `Notice` plus a log line, without changing the pane.
- **Verify:** `cargo test -p filecargo-app-core --test remote_ops`
- **Files:** `src/ops.rs`, `tests/remote_ops.rs`

### Checkpoint B: AC3–5, 8, 9 green

### T8: Transfers wiring (M)
`QueueConnector` (adapts `SessionFactory` + shared trust to `transfer::Connector`), `Upload`/`Download` → `NewTransfer`s, queue commands, `ConflictAsked` → `Conflict` prompt → `resolve`, pane refresh debounce (1/s) on completions in the visible dir, snapshot rate check.
- **Accept:** AC6, AC7, AC11.
- **Verify:** `cargo test -p filecargo-app-core --test transfers`
- **Files:** `src/transfers.rs`, `src/app.rs`, `tests/transfers.rs`

### T9: Terminal, quit, re-exports (S)
`TerminalOpen/Input/Resize/Scroll/Close`, `TerminalState`, close on disconnect; `Quit` → `ConfirmQuit` when active → `Queue::shutdown` → stop; `pub mod prelude` with the spec's re-exports.
- **Accept:** AC10; `TerminalState::NotAvailable` for FTP sessions; the terminal opens over a fake shell channel in tests.
- **Verify:** `cargo test -p filecargo-app-core --test terminal --test quit`
- **Files:** `src/terminal.rs`, `src/app.rs`, `src/prelude.rs`, `tests/terminal.rs`, `tests/quit.rs`

### Checkpoint C: module complete
- [ ] All 11 AC green, coverage ≥ 80 %, CI green; hand-off notes; spec status → done; human review
