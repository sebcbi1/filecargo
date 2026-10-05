# Tasks: `gui`

> Plan: [plan.md](plan.md) · Spec: [SPEC-gui.md](../../SPEC-gui.md) · DoD as in [tasks/transfer/todo.md](../transfer/todo.md).
> Headless tests: `#[gpui_kit::test]` with `TestAppContext` / `TestWindowExt`, an app-core `StartOptions` using `MemoryStore` + the RootedFs `SessionFactory`.

### T1: Platform skeleton + CI (M)
Crate + `[[bin]] filecargo` (not in `default-members`), `App::start`, `gpui_kit::application().with_assets(..)`, `gpui_kit::init`, `Theme::sync_system_appearance`, `open_window(Workspace)`, `AppModel` (foreground task on `state.changed()`), empty `h_resizable`/`v_resizable` layout, `Quit` action → `Command::Quit` + `shutdown`. CI `gui` job (ubuntu deps, macos, windows).
- **Accept:** AC5, AC6; AC1 subcases i (window opens) and ii (snapshot re-render).
- **Verify:** `cargo test -p filecargo-gui`; `cargo run -p filecargo-gui`; `actionlint`; CI green
- **Files:** `crates/filecargo-gui/{Cargo.toml,src/main.rs,src/model.rs,src/workspace.rs}`, `.github/workflows/ci.yml`

### T2: File panes (M)
`FilePaneView` + `PaneDelegate: TableDelegate` (name / size / modified / permissions columns, `perform_sort` → `SetSort`, multi-select set drawn in `render_tr`, shift/secondary handling, arrow keys), path input (`Input`), double-click / Enter / Backspace navigation, pane context menu (actions wired as they become available).
- **Accept:** AC1 subcases iv (double-click dir) and v (multi-select).
- **Verify:** `cargo test -p filecargo-gui panes`
- **Files:** `src/pane/{mod.rs,delegate.rs,path_bar.rs}`, `src/actions.rs`

### Checkpoint A: local browsing works on all three CI builds; review the look with the human

### T3: Server tree + session (M)
`ServerTreeView` (`Tree` rebuilt on `Arc` change, expansion re-applied, `on_click` selection, double-click connect/toggle), remote pane (second `FilePaneView`), toolbar (refresh, upload, download, new folder, rename, delete, session indicator).
- **Accept:** AC1 subcase iii (double-click connects).
- **Verify:** `cargo test -p filecargo-gui tree`
- **Files:** `src/tree.rs`, `src/toolbar.rs`, `src/workspace.rs`

### T4: Dialogs + site editor (M)
Dialog helper (pre-created `InputState`s, footer buttons, `on_ok` validation), site editor (protocol and auth `Select`s, masked password + remember, key-file picker via `cx.prompt_for_paths`), tree context menu: new site / folder, rename, duplicate, move to…, delete (confirm), import FileZilla (path picker + checkbox).
- **Accept:** AC4.
- **Verify:** `cargo test -p filecargo-gui site_editor`
- **Files:** `src/dialogs/{mod.rs,site_editor.rs,tree_ops.rs,import.rs}`

### T5: Prompts, permissions, settings, notices (M)
One dialog per `PromptKind` (credential, host key, certificate, conflict, confirm delete, confirm quit, message incl. import report), permissions dialog (octal ↔ rwx grid), settings dialog (`UpdateSettings`), `notices` → `push_notification`.
- **Accept:** AC2; AC1 subcase vii (credential dialog → `Answer`).
- **Verify:** `cargo test -p filecargo-gui prompts`
- **Files:** `src/dialogs/{prompt.rs,permissions.rs,settings.rs}`, `src/notices.rs`

### Checkpoint B: manual run against docker SFTP: create a site, accept the host key, enter a password, browse

### T6: Bottom panel + transfers (M)
`BottomPanel` with `TabBar` (counts in labels), `QueueView` / completed / failed `DataTable`s with a progress cell, speed and ETA, row menus (retry, remove, clear completed), pause toggle; `LogView` (`uniform_list`, follow mode); F5 / double-click file / context-menu transfers; OS notification on batch completion when unfocused.
- **Accept:** AC1 subcase vi (F5 with selection → `Upload`/`Download`).
- **Verify:** `cargo test -p filecargo-gui queue`
- **Files:** `src/bottom/{mod.rs,queue.rs,log.rs}`

### T7: Terminal element (M)
`TerminalView` + element: cell metrics from `em_advance`/ascent/descent, background quads, per-row `shape_line(force_width)` with attribute runs, cursor quad, row cache by generation, key → `terminal::Key` + `Mods` → `TerminalInput`, copy (mouse selection → `selection_text`) / paste (`paste`), resize from bounds → `TerminalResize`, `TerminalOpen` on first show.
- **Accept:** AC3; frame time measured at 200×60 (target < 4 ms) and recorded in the hand-off notes.
- **Verify:** `cargo test -p filecargo-gui terminal`
- **Files:** `src/terminal/{mod.rs,element.rs,runs.rs,keys.rs}`

### T8: Shortcuts, smoke, finish (M)
Full action and key-binding set from the spec table, `SMOKE.md`, manual smoke on macOS / Linux / Windows, remaining headless tests, hand-off notes.
- **Accept:** AC7 executed and recorded; all AC green.
- **Files:** `src/actions.rs`, `SMOKE.md`

### Checkpoint C: module complete → v1 feature-complete (both binaries); human review
