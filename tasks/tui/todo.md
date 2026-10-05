# Tasks: `tui`

> Plan: [plan.md](plan.md) · Spec: [SPEC-tui.md](../../SPEC-tui.md) · DoD as in [tasks/transfer/todo.md](../transfer/todo.md), plus `cargo insta test` clean.
> Test fixtures: an `AppState` builder in `src/test_support.rs` (fixed timestamps, sizes, names), so snapshots are deterministic.

### T1: Skeleton binary (S)
Crate + `[[bin]] filecargo-tui`, add to `default-members`; args; `App::start`; `ratatui::init` / restore with panic hook; `tokio::select!` loop (input, `state.changed()`, tick); `q` quits via `Command::Quit`.
- **Accept:** AC5 (panic restores the terminal); `cargo run -p filecargo-tui` shows the empty layout and quits cleanly.
- **Verify:** `cargo test -p filecargo-tui`; manual run
- **Files:** `crates/filecargo-tui/{Cargo.toml,src/main.rs,src/app_loop.rs,src/ui_state.rs}`

### T2: Layout + local pane (M)
Layout areas per spec, local pane `Table` (name/size/modified, dir markers, selection marks), `UiState` focus, cursor, selection set keyed by name with reset on `generation` change, navigation and selection bindings, sort / hidden toggles, too-small screen.
- **Accept:** AC3; binding rows for file panes (navigation and selection); snapshots: disconnected and too-small.
- **Verify:** `cargo test -p filecargo-tui`, `cargo insta test -p filecargo-tui`
- **Files:** `src/layout.rs`, `src/pane.rs`, `src/reducer.rs`, `src/keymap.rs`, `src/test_support.rs`

### Checkpoint A: browse the local disk in the real binary; first snapshot review

### T3: Server tree + remote pane (M)
`tui-tree-widget` tree from `ServerTree` (expanded state in `UiState`), connection marker, tree bindings (move, expand, connect, disconnect), remote pane reusing the pane widget, remote pane title with session state.
- **Accept:** binding rows for the server tree; snapshots: connecting and connected listing.
- **Files:** `src/tree.rs`, `src/pane.rs`, `src/reducer.rs`, `src/keymap.rs`

### T4: Forms + site editor (M)
Form toolkit (text, masked, checkbox, select, buttons, focus order), site editor new/edit → `Command::Tree(AddSite/UpdateSite)` + `SetSitePassword`, new folder / rename / move / delete dialogs.
- **Accept:** snapshot of the site editor; reducer tests for field editing, validation messages, cancel.
- **Files:** `src/form.rs`, `src/dialogs/site_editor.rs`, `src/dialogs/simple.rs`, `src/reducer.rs`

### T5: Prompts and utility dialogs (M)
Render and answer every `PromptKind`; chmod dialog (octal ↔ rwx); go-to-path; import FileZilla (prefilled path, checkbox).
- **Accept:** one snapshot per prompt kind; reducer tests for each answer path.
- **Files:** `src/dialogs/prompt.rs`, `src/dialogs/chmod.rs`, `src/dialogs/goto.rs`, `src/dialogs/import.rs`

### Checkpoint B: manual run against docker SFTP: create a site, connect, accept the host key, browse

### T6: Bottom panel + transfers (M)
Tabs (Queue / Completed / Failed / Log) with counts, queue table with progress bars, speed and ETA, queue bindings, `F5` / `t` / `Enter`-on-file transfers, log view (follow / scroll).
- **Accept:** snapshot of a transfer in progress; binding rows for queue/log/transfer.
- **Files:** `src/bottom/{mod.rs,queue.rs,log.rs}`, `src/reducer.rs`

### T7: Terminal tab (S)
`tui-term` `PseudoTerminal` via `TerminalHandle::with_screen`, crossterm `KeyEvent` → `terminal::Key` + `Mods`, `Ctrl-\` / `F12` escape, `Shift-PgUp/PgDn` scrollback, `TerminalOpen` / `TerminalResize` from the tab size.
- **Accept:** AC4; snapshot of the terminal tab (fake screen content).
- **Files:** `src/bottom/terminal.rs`, `src/keys.rs`, `src/reducer.rs`

### T8: Polish and verification (M)
Help overlay generated from `keymap.rs`, status line hints, mouse (wheel, click to focus/select), `NO_COLOR`, remaining snapshots, `SMOKE.md`, manual smoke on macOS / Linux / Windows Terminal.
- **Accept:** AC1, AC2 complete; AC6 checklist executed and recorded.
- **Files:** `src/help.rs`, `src/status.rs`, `src/mouse.rs`, `SMOKE.md`

### Checkpoint C: module complete (6 AC, CI green, smoke recorded, hand-off notes, spec → done, human review)
