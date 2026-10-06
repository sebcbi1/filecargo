# Spec: v1.1 (local file operations, per-connection bottom panel, staged queue)

> Increment id: `v1.1` · Source: [TODO.md](TODO.md) · Status: **implemented**
> Builds on the v1 modules in [SPEC.md](SPEC.md). Module ids don't change. Each feature below
> names the modules it touches, and the contract changes go in the provider's spec
> (`transfer`, `app-core`) when the feature is implemented.

## Objective

Four improvements from TODO.md for the developer using filecargo every day:

| Feature id | What | Touches |
|---|---|---|
| `tui-backtab` | Shift-Tab moves focus backwards in the TUI (bug: it does nothing today) | `tui` |
| `local-ops` | New folder, rename, delete and permissions on the **local** pane, like the remote pane | `app-core`, `tui`, `gui` |
| `site-scoped-panel` | Queue, Completed, Failed and Log show only the **current connection's** items; the lists show both local and remote paths | `transfer`, `app-core`, `tui`, `gui` |
| `staged-queue` | "Add to queue" without starting, "Start all queued", per-site pause/resume and clear; the queue survives restarts as held items | `transfer`, `app-core`, `tui`, `gui` |

**Build order:** `tui-backtab` → `local-ops` → `site-scoped-panel` → `staged-queue`
(`staged-queue` relies on the per-site views from `site-scoped-panel`; the first two stand alone).

### tui-backtab

- Root cause, verified: crossterm reports Shift-Tab as `KeyCode::BackTab` **with** `SHIFT`, but the
  binding in `crates/filecargo-tui/src/keymap.rs` only accepts `BackTab` with no modifiers or
  `Tab` with `SHIFT`. The existing test sends `BackTab` without modifiers, so it never saw this.
- Fix: `BackTab` matches with or without `SHIFT`, in the global binding and in dialog forms.

### local-ops

- `Command::Mkdir`, `Rename`, `Delete` and `Chmod` gain `pane: PaneId`. For `PaneId::Local`
  they act on entries of the local pane's directory through the `remote-fs` local backend, then
  refresh the local pane. Errors are shown the same way as remote errors.
- **Delete is permanent** (no OS trash), recursive for directories, and asks first when
  `settings.ui.confirm_delete` is on, exactly like remote delete.
- **Permissions on Unix only.** On Windows the local pane doesn't offer them (no menu entry, key
  ignored with a status hint).
- TUI: F7, F2, F8/Del and `c` act on whichever pane has the focus.
- GUI: the local pane's context menu gets New folder, Rename, Delete and Permissions…, and the
  toolbar buttons act on the focused pane.

### site-scoped-panel

- **Scope** = the site of the current session.
  - Connected: the panel shows that site's queue, completed, failed and log lines, plus app-wide
    log lines (lines that belong to no site).
  - **Not connected:** Queue, Completed and Failed are **empty**; the Log shows only app-wide lines.
- Other sites' transfers **keep running in the background**:
  - The status bar (TUI) and the toolbar (GUI) show "N transfers on other sites" while there are
    any.
  - Quit confirmation still counts every site's active transfers.
- **Log tagging:**
  - Lines emitted while handling a site's session or transfers carry that `SiteId`.
  - Lines get it from a `site` tracing span field that the log layer records into `LogLine.site`.
  - Lines outside any such span are app-wide.
- **Paths:** each row of Queue, Completed and Failed shows the direction, the local path, the
  remote path, the size and the progress or error. Long paths are truncated from the left (`…/www/index.php`).
- The tab counts (`Queue (3)`, `Failed (1)`) count the scoped items only.

### staged-queue

- New item state **`Held`**: queued but not started. The scheduler never starts a held item.
- **Add to queue:** for the selected entries, the same expansion and conflict handling as
  Upload/Download, but every resulting item is `Held`.
  - TUI: `a` in a file pane.
  - GUI: an "Add to queue" button next to Upload/Download, plus a pane context-menu entry.
- **Start all queued:** turns every `Held` item of the current site into `Pending`, and resumes
  the site if it is paused.
  - TUI: `S` in the Queue tab.
  - GUI: a "Start queue" button in the Queue tab.
- **Pause / resume per site:**
  - A paused site starts no new items. Active items finish, as the global pause does today.
  - Other sites are not affected.
  - TUI: `p` in the Queue tab. GUI: a pause/resume toggle in the Queue tab.
  - The global `QueueSetProcessing` goes away from the UIs and is replaced by the per-site
    toggle.
- **Clear queue:** asks first, then cancels the current site's active items (partial files stay,
  so resume can pick them up) and removes its pending and held items.
  - TUI: `X` in the Queue tab. GUI: a "Clear queue" button.
  - "Clear completed" and a new "Clear failed" are also scoped to the site.
- Normal Upload/Download (F5, double-click, toolbar) still start right away.
- **Persistence:** `queue.json` keeps pending, held and failed items, as today.
  - On load, every restored pending or interrupted item becomes **Held**, so nothing starts until
    the user presses Start.
  - Completed items and the per-site pause flags are not persisted.
  - The `queue.json` format gains a version bump; files written by v1 still load.

## Tech Stack

No changes: the v1 stack in [SPEC.md](SPEC.md) (Rust 2024, tokio, ratatui/crossterm, gpui-kit
=0.7.1). **No new dependencies.** A trash/recycle-bin crate was considered for local delete and
rejected for v1.1 (see Boundaries).

## Commands

```
Format:        cargo fmt --all --check
Lint:          cargo clippy --workspace --exclude filecargo-gui --all-targets -- -D warnings
Lint (GUI):    nix-shell --run "cargo clippy -p filecargo-gui --all-targets -- -D warnings"
Test:          cargo test
Test (GUI):    nix-shell --run "cargo test -p filecargo-gui"
Integration:   docker compose -f tests/docker/compose.yml up --build --wait && cargo it
Coverage:      cargo llvm-cov --workspace --exclude filecargo-gui --summary-only
Run TUI:       cargo run -p filecargo-tui
Run GUI:       nix-shell --run "cargo run -p filecargo-gui"
```

## Project Structure

Existing layout, no new crates:
```
crates/filecargo-transfer/src/{model,queue,store}.rs  → Held state, per-site pause/clear, held-on-restore, format version
crates/filecargo-app-core/src/{command,pane,logging,session}.rs → pane-aware file ops, site scoping, log tagging
crates/filecargo-app-core/tests/                     → local-ops, scoping and staged-queue tests over RootedFs
crates/filecargo-tui/src/{keymap,reducer,bottom_view}.rs + snapshots → keys, scoped lists, path columns
crates/filecargo-gui/src/{pane,toolbar,bottom/*}.rs + tests/ → menus, buttons, scoped tables
tasks/v1.1/{plan,todo}.md                             → plan and task list for this increment
```

## Code Style

Same as v1: edition 2024, `unsafe_code = forbid`, clippy `-D warnings`, comments only for the
non-obvious, and doc comments that say what an item is for. Commands carry the data the core
needs and name panes explicitly:

```rust
// ---- file operations (names are entries of `pane`'s directory) ----
Mkdir { pane: PaneId, name: String },
Rename { pane: PaneId, from: String, to: String },
/// Asks first when `settings.ui.confirm_delete`.
Delete { pane: PaneId, names: Vec<String> },
/// Unix only for the local pane.
Chmod { pane: PaneId, names: Vec<String>, mode: u32 },

// ---- queue (scoped to the connected site) ----
/// Like `Upload`/`Download`, but every item waits as `Held`.
Enqueue { from: PaneId, names: Vec<String> },
QueueStartHeld,
QueueSetSitePaused(bool),
/// Cancels active items, removes pending and held ones.
QueueClear,
QueueClearFailed,
```

## Testing Strategy

TDD per task (failing test first), as in v1.

- **`transfer` unit tests:**
  - Held items are never scheduled.
  - Start-held makes them pending.
  - Per-site pause leaves other sites running.
  - Clear cancels and removes only that site's items.
  - A v1 `queue.json` loads, with pending items restored as held.
- **`app-core` integration tests** (RootedFs factory, `tests/`):
  - Local mkdir, rename, delete (with and without confirm) and chmod (`cfg(unix)`).
  - Scoped snapshots: connected to A, A's items only; disconnected, empty.
  - Log lines carry the site.
  - Enqueue produces held items.
  - Quit still counts every site.
- **TUI:**
  - Keymap tests that send `BackTab` both with and without `SHIFT`.
  - Reducer tests for the new keys.
  - insta snapshots of the Queue tab with path columns, empty when disconnected, and the
    "N transfers on other sites" hint.
- **GUI** (`#[gpui_kit::test]`):
  - The local pane context menu sends pane-aware commands.
  - The Add to queue and Start queue buttons work.
  - The tables show the scoped rows and both paths.
- **Integration (`cargo it`, Docker):** one SFTP round trip that enqueues, restarts the queue
  from disk and starts it.
- Coverage stays at or above v1 (lines ≥ 85% for app-core, transfer and tui).

## Boundaries

- **Always:**
  - Write a failing test first.
  - Run fmt, clippy and tests (GUI under nix-shell) before each commit.
  - Make one jj commit per task, with no AI attribution lines.
  - Update the provider module's spec when a contract changes.
  - Keep both front-ends on app-core only.
- **Ask first:**
  - Adding a dependency (e.g. a trash crate).
  - Changing the `queue.json` format beyond the versioned, backward-compatible bump above.
  - Changing CI.
  - Removing the global pause API from `transfer` instead of hiding it.
  - Anything that changes the "one session at a time" rule.
- **Never:**
  - Delete local files without the confirm setting being honoured.
  - Break loading of a v1 `queue.json`.
  - Put secrets in the queue file.
  - Remove or weaken failing tests to get green.
  - Push or tag without being asked.

## Success Criteria

1. In the TUI, Shift-Tab moves focus backwards in xterm, kitty and tmux. The keymap test covers
   `BackTab` both with and without `SHIFT`.
2. In both UIs, on the local pane: creating a folder, renaming, deleting (recursive, with confirm)
   and chmod (Unix) change the disk and refresh the pane.
3. Connected to site A while site B has active transfers:
   - The panel lists only A's items and log lines, plus app-wide lines.
   - B's transfers keep going, and "1 transfer on other sites" is shown.
4. When disconnected, Queue, Completed and Failed are empty, and the Log shows only app-wide lines.
5. Every row in Queue, Completed and Failed shows both the local and the remote path.
6. Add to queue on 3 selected files leaves 3 held items and nothing transfers. Start all queued
   transfers them.
7. Pause on A stops A from starting new items, and B is unaffected. Clear (after confirm) leaves
   A's queue empty.
8. After quitting with held and pending items and restarting, they come back as held. Failed
   items come back as failed. Nothing starts until Start is pressed.
9. A `queue.json` written by v1.0 loads without errors.
10. CI is green on all jobs.

## Open Questions

None blocking. Decided with the user on 2026-10-06:
- Shift-Tab is the broken focus cycling, not bottom-tab cycling.
- When disconnected the panel is empty (Log: app-wide lines only).
- Pause, start and clear are per site, with held items.
- Restored items come back held.

Assumptions to confirm at review:
- Local delete is permanent (no trash).
- Clear cancels the site's active items.
- The pause flags are not persisted.
- Key choices: `a` add to queue, `S` start, `X` clear.
