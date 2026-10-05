# Implementation Plan: `gui` module

> Spec: [SPEC-gui.md](../../SPEC-gui.md) · Tasks: [todo.md](todo.md) · Status: **awaiting review** · 2026-10-05
> Starts after `tui` Checkpoint B: by then the TUI has exercised the app-core API end to end, so the GUI builds on a settled contract.

## Overview
`crates/filecargo-gui`, binary `filecargo`, on gpui-kit `=0.7.1`.
- **First task:** a building, CI-verified window on all three OSes. The platform build is the
  biggest unknown, so it goes first.
- **Then the views in the TUI's order:** panes, tree, dialogs, bottom panel, terminal.
- **Tests:** headless gpui tests (`gpui-kit` `test-support`) cover the wiring; a manual smoke
  checklist covers rendering.

## Architecture Decisions
| Decision | Rationale |
|---|---|
| **No tokio in the GUI process's gpui side**; `AppModel` awaits `watch::changed()` on the foreground executor | Verified: tokio::sync is runtime-agnostic and gpui-pre has no tokio. app-core owns all I/O on its own runtime. |
| **gpui-kit `DataTable` for panes and queue, with delegate-owned multi-select** (`BTreeSet` drawn in `render_tr`) | Sorting, resizing, virtualization, context menu and double-click come for free; the multi-select workaround was compiled and tested in research. |
| **gpui-kit `Tree` for servers**, rebuilt on `Arc` change, expansion re-applied, selection via `ListItem::on_click` | The component lacks a selection event and resets on `set_items` (verified); this works around both. |
| **Terminal = `canvas` + per-row `shape_line(force_width = cell)`**, shaped rows cached by (row, terminal generation) | Exact monospace grid without a custom text engine; the cache avoids re-shaping unchanged rows. |
| **Dialogs via `WindowExt::open_dialog`**, form `InputState`s created before opening and moved in | The dialog builder re-runs every frame (verified); state must live outside it. |
| **Actions + key contexts** (`Workspace`, `FilePane`, `ServerTree`, `Terminal`) | Terminal focus captures keys; global shortcuts stay predictable. |
| **CI `gui` job from T1** | Catches platform build breaks (Linux deps, Windows `fxc.exe`) before any UI work piles up. |

### Dependencies (new)
`gpui-kit = "=0.7.1"` (dev: `features = ["test-support"]`), `anyhow`. Nothing else: GPUI comes
through gpui-kit, and must never be added directly.

## Task List
### Phase 1: Platform first
- [x] T1: Crate + bin, `App::start`, gpui-kit bootstrap (`with_assets`, `init`, `open_window` + Root), `AppModel` watch bridge, empty resizable layout, theme sync, quit; CI `gui` job on 3 OSes (M)
- [x] T2: File panes: `DataTable` delegate (columns, sort → `SetSort`, multi-select), local navigation (double-click, Enter, Backspace, path input), pane context menu (M)
### Checkpoint A: the binary browses the local disk; CI gui job green on ubuntu/macos/windows
### Phase 2: Sessions and editing
- [x] T3: Server tree view, connect/disconnect, remote pane, toolbar with session status (M)
- [x] T4: Dialog infrastructure, site editor, tree context-menu operations (new, rename, duplicate, move, delete, import) (M)
- [x] T5: Prompt dialogs for every `PromptKind`, permissions dialog, settings dialog, notices → notifications (M)
### Checkpoint B: connect over SFTP with host-key and password prompts, edit sites, change settings
### Phase 3: Transfers, terminal, polish
- [x] T6: Bottom panel (`TabBar`), Queue / Completed / Failed tables with progress, Log `uniform_list`, transfer actions (F5, double-click, menus), batch-complete OS notification (M)
- [x] T7: Terminal element (cells, runs, cursor, row cache), key mapping, clipboard, resize → `TerminalResize` (M)
- [ ] T8: Full key-binding set, `SMOKE.md`, manual smoke on macOS / Linux / Windows, remaining headless tests (M)
### Checkpoint C: all 7 AC green, smoke recorded, human review → v1 feature-complete

## AC Traceability
AC1 → T1–T6 (each subcase in the task that builds it) · AC2 → T5 · AC3 → T7 · AC4 → T4 · AC5 → T1 · AC6 → T1 · AC7 → T8

## Risks and Mitigations
| Risk | Impact | Mitigation |
|---|---|---|
| gpui-pre / gpui-kit breaking changes (weekly releases) | High | Exact pins; bumps are an ask-first change; all gpui-kit usage is confined to view modules. |
| Linux build deps or Wayland/X11 runtime issues | Med | CI job from T1 using gpui-kit's own dependency list; smoke test on both X11 and Wayland if available. |
| DataTable multi-select workaround breaks keyboard selection semantics | Med | The delegate handles shift/secondary + arrow keys itself; headless tests in T2 pin the behavior; fallback: a custom `uniform_list` table. |
| Terminal rendering cost at large sizes (200×60) | Med | Row-shape cache keyed by terminal generation; repaint only on generation change; measure in T7 (target: < 4 ms per frame on the dev Mac). |
| Headless tests cover wiring, not pixels | Med | Manual smoke checklist per release on all three OSes (AC7); visual issues logged against SPEC-gui. |
| Windows release build needs `fxc.exe` | Low | Present on GitHub runners via the Windows SDK; `GPUI_FXC_PATH` documented in `SMOKE.md`. |

## Open Questions
None. Drag and drop is explicitly post-v1.

## Hand-off Notes
_Appended per task during implementation._

### Build environment (all tasks)
- GPUI needs system libraries at build time. On NixOS use the repo's `shell.nix`: `nix-shell --run "cargo test -p filecargo-gui"` (other Linux: the apt list in the CI job; macOS / Windows need nothing extra). `cargo build` / `cargo test` without `-p filecargo-gui` never compile gpui (the crate is not a default member; the CI `check` job excludes it).
- Headless tests need no display: gpui-kit's `test-support` runs on gpui's test platform.

### T1 Platform skeleton + CI (done)
- Crate = lib `filecargo_gui` + bin `filecargo`. `main`: `App::start`, `gpui_kit::application().with_assets(AllAssets)` (the default `Assets` lacks the Upload / Download / Trash / Pencil icons we need), `gpui_kit::init`, **`Theme::sync_system_appearance(None, cx)` right after** (`init` forces the light theme), `secondary-q` → `Quit` action → `Command::Quit` (the window closes when the app has really quit, so a "transfers running" confirmation can intervene), `open_window` with `Workspace`.
- `AppModel` entity holds `Arc<AppState>` + the `AppHandle`; a foreground task refreshes it and `cx.notify()`s; `Workspace` observes it. **Deviation:** the task polls `watch::has_changed()` every 33 ms instead of awaiting `changed()`: gpui's deterministic test scheduler panics when a task is woken from another thread (the app-core thread), so awaiting would make the tests unable to run the production path. app-core publishes ≤ 30 snapshots/s anyway. The channel closing (app quit) ends the task and quits gpui.
- `Workspace`: toolbar strip, `h_resizable` (tree 220 px, 160–480) with three areas, `v_resizable` body / bottom panel (220 px), all placeholders for now; `renders` counter for tests.
- CI: new `gui` job (ubuntu with the apt libs / macos / windows: clippy, build, test); the `check` job now excludes `filecargo-gui`; `actionlint` clean, **not run on GitHub**. `shell.nix` for NixOS.
- Tests (`tests/window.rs`, harness in `tests/support/mod.rs`: real app-core on a temp dir, gpui test window with `Root`): window opens, a new snapshot re-renders (AC1 i, ii), theme switch re-renders (AC5).

### T2 File panes (done)
- `pane.rs`: `FilePaneView` (path bar `Input`, `DataTable`, error line, "Not connected" hint for the remote pane) over a `PaneDelegate`. Columns Name / Size / Modified / Permissions; a header click on the first three calls `perform_sort` → `Command::SetSort` (the table never sorts by itself: the new order arrives with the snapshot); a `..` row is added when the directory has a parent.
- **Multi-select in the delegate** (`BTreeSet<String>` of names, drawn in `render_tr` with the table's active-row colour): plain mouse-down selects only that row, `secondary` toggles, `shift` selects the range from the anchor (shift+secondary extends); `secondary-a` selects all; keyboard Up/Down (handled by the table) select the row they land on — the `SelectRow` event that follows a mouse press is ignored via a flag. **Not done:** `Shift+Arrow` range extension (the table owns the arrow keys; revisit if the human review wants it).
- Selection rule (AC3 of the TUI, same here): same path + new generation → names that still exist stay selected, another path → cleared.
- Navigation: double-click a directory → `Navigate`, the `..` row → `Up`; `Enter` / `Backspace` / `secondary-r` through actions in the `FilePane` key context (`workspace::bind_keys`); path bar `Enter` → `Navigate`. Files are not transferred yet (T6). Context menu has *Refresh* only for now; the other entries arrive with the dialogs (T4/T5) and transfers (T6).
- `format.rs`: size / UTC time / `drwxr-xr-x` helpers (a copy of the TUI's, since front-ends share only app-core).
- Things learned: `TableState::refresh()` resets user-resized column widths, so rows are replaced through `delegate_mut()` + `notify`; `refresh()` is only used when the sort indicator changes. Real double clicks work headless through `window.double_click(("row", n), cx)`.
- Tests (`tests/panes.rs`, 6): listing with parent row, multi-select (plain / secondary / shift / `..`), double-click directory and `..` (AC1 iv), selection across refresh and navigation, header sort → app (AC1 v for the multi-select part), remote hint.

### T3 Server tree + session (done)
- `tree.rs` (`ServerTreeView`): gpui-kit `Tree` rebuilt from `ServerTree` whenever its `Arc` pointer changes; item ids are `folder:<uuid>` / `site:<uuid>` with a `nodes` map back to `NodeId`; the open folders are remembered from `TreeEvent::{Expanded, Collapsed}` and re-applied after `set_items` (which also resets the selection, so a vanished node is dropped from `selected`). Selection is read from the tree state after it notifies (`selected_item`), exposed as `ServerTreeView::selected`. Rows: folder icons, a green check on the connected site, a red cross on a failed one, a spinner while connecting. Double-click on a site (`ListItem::on_click`, `click_count() == 2`) → `Command::Connect`; a click on a folder row toggles it (the component's own behaviour). Context menu: *Connect* (and *Disconnect* on the connected site); more entries arrive in T4.
- Toolbar (in `workspace.rs`, label text in `toolbar.rs`): *Connect* (enabled with a site selected and no connection in progress), *Disconnect*, *Refresh* (both panes) and the session indicator (`● name (SFTP)`, `Connecting to … (step)…`, `✗ name: error`). The workspace observes the tree entity so the buttons follow the selection. Upload / Download / New folder / Rename / Delete buttons come with their dialogs and transfers (T4–T6).
- Tests need an app whose sessions are served from disk: `tests/support/factory.rs` is a `SessionFactory` over `RootedFs` (dev-dependencies `filecargo-remote-fs`, `async-trait`). `tests/tree.rs` (4): tree rows and selection, double-click connects and the remote pane's table lists the server (AC1 iii), toolbar connect / disconnect, deleting the selected site clears the selection.
- Test gotcha: effects (observers) only run on `cx.run_until_parked()`, so a click that changes the selection and the click that depends on it must be in separate `update_window` calls.

### T4 Dialogs + site editor (done)
- **Dialog pattern** (`dialogs/mod.rs`): the dialog builder re-runs every frame, so each dialog is a small **view entity** that owns its `InputState`s / `SelectState`s / flags, and `open_form(window, cx, title, width, content, ok_label, on_ok)` wraps it in `window.open_dialog` with a *Cancel* / *OK* footer (a plain `Dialog` has no buttons of its own; the OK button has the id `ok`, which tests click with `window.within("dialog").click("ok", cx)`). `on_ok` returns whether to close, so validation errors keep the dialog open and are shown inside it. `confirm(...)` wraps `open_alert_dialog` for yes / no questions (used for delete now, quit and delete-in-pane in T5).
- **Site editor** (`site_editor.rs` + the pure `site_form.rs`): name, protocol and login `Select`s, host, port, user, masked password with a *Remember* checkbox, key file with a *Browse…* picker (`prompt_for_paths`), FTP mode, remote / local folders, notes; the fields that do not apply to the chosen login / protocol are hidden. `FormValues::{of, to_site, commands}` hold all the logic (validation messages are the TUI's; edits keep id and folder; the password is sent as `SetSitePassword` only when *Remember* is ticked and the field is not empty). The editor does its own validation, inline; app-core's tree errors still arrive as prompts (T5).
- **Tree operations** (`tree_ops.rs`): new folder, rename (name must be non-empty, no `/`), delete with confirmation, move (a `Select` of the top level and every folder except the node and what is below it), FileZilla import (path with picker, *import passwords* box). Reached from the row context menu (site: Connect / Disconnect / Edit… / Duplicate / Rename… / Move to… / Delete…; folder: New site here… / New folder here… / Rename… / Move to… / Delete…), the toolbar (*New site*, *New folder*, *Import…*) and keys in the `ServerTree` context (`F2` rename, `Delete`, `secondary-e` edit).
- Tests: 5 unit tests for `FormValues`, 1 for name checks, and `tests/dialogs.rs` (8, real dialogs in a headless window): the **AC4 round trip** (create → in the tree → edit keeps the id → values persisted), invalid form stays open with the message, remembered password reaches the keychain, rename (refuses `a/b`), new folder, delete only after confirming, move, import of the FileZilla fixture (6 sites).
- Test setup note: dialogs animate on the wall clock; the harness calls `cx.set_reduce_motion(true)`.
- Not covered by a test: the *Browse…* pickers (OS dialogs) — in the smoke list.

### T5 Prompts, permissions, settings, notices (done)
- `prompts.rs`: `PromptHost` (an entity of the workspace) opens a dialog when a **new prompt id** appears in the snapshot, one per `PromptKind`; a shared `Answerer` sends the `Command::Answer` exactly once, and **Esc gives the prompt's "no" answer** (credential → `None`, trust → `Reject`, confirm → `false`, conflict / message → `Dismiss`), so a prompt is never left unanswered with its dialog gone. Overlay clicks and the close button are disabled on prompt dialogs.
  - Credential (password / passphrase / keyboard-interactive): masked or echoed inputs per prompt, a *Remember* checkbox except for keyboard-interactive, a retry note. Host key and certificate: the details and *Reject* / *Trust once* / *Trust always* (no default button). Conflict: both files' sizes and times, the five rules as buttons plus *Apply to all remaining files*. Confirm delete / quit: `AlertDialog`-style Cancel / OK (red OK). Message: OK (the import report is its lines).
- `dialogs/permissions.rs`: octal field and the nine rwx checkboxes in step both ways (special bits typed in octal survive a box toggle), `Command::Chmod` on the selection; opened from the remote pane's context menu (*Permissions…*). `dialogs/settings.rs`: `SettingsForm` (pure, unit-tested: ranges 1–10 / > 0, empty start folder = `~`) + view; toolbar *Settings* button and `secondary-,`.
- `notices.rs`: `NoticeHost` turns each new `AppState.notices` entry into a notification (info / warning auto-hide, errors stay) and closing one sends `DismissNotice`. The batch-complete OS notification comes with the transfers (T6).
- Tests: `tests/prompts.rs` (10, **one per kind plus the "no" paths**: password typed into the real input, cancel, host key once / reject, certificate always, conflict overwrite / skip with a check that nothing is written before the answer, confirm delete (cancel then confirm), confirm quit with a slow transfer running, message), `tests/utility.rs` (3: permissions → file mode changes on disk, settings save + bad number refused, notice → notification → close → dismissed). The test factory (`tests/support/factory.rs`) is app-core's with host-key / certificate triggers; `terminal.rs` has the fake shell for T7.
- Gotchas: the notification's close button only exists while hovered (`window.hover(..)` first).

### T6 Bottom panel + transfers (done)
- `bottom/mod.rs` (`BottomPanel`): a `TabBar` (*Queue (n)* / *Completed (n)* / *Failed (n)* / *Log* / *Terminal*, counts only when non-zero) over the active content; `bottom/queue.rs`: one `QueueDelegate` for the three lists (queue: direction icon, name, a drawn progress bar with percent, `done/total`, speed, ETA, "waiting for you" for conflicts, "queued"; completed: result, size, finish time; failed: reason, retry / final) fed by the `Arc<QueueSnapshot>` pointer; row context menu *Retry* / *Retry all failed* (failed), *Remove*, *Clear completed* (completed); `Delete` in the `Queue` key context removes the row under the cursor. `bottom/log.rs`: `uniform_list` over the shared `LogBuffer`, following the newest line until the wheel scrolls up. The terminal tab is a placeholder until T7.
- Transfers: `F5` (`TransferSelection`, `FilePane` context) sends the selection or the cursor row to the other side and clears the selection; toolbar *Upload* / *Download* (enabled while connected), a double-click on a file, and a *Upload* / *Download* entry in the pane context menu; toolbar *Pause / Resume transfers* toggles `QueueSetProcessing`.
- Batch-complete OS notification (`notices.rs`): when the queue goes from busy to empty while the window is not active, a system notification says "N items transferred, M failed". Pure functions `batch_summary` / `batch_ended` are unit-tested; the OS delivery itself (notify-rust on Linux needs a notification daemon) is in the smoke list.
- Icons: the Upload / Download toolbar icons come from `gpui_kit::assets::IconName` (only in `AllAssets`, which `main` registers; headless tests have no assets and draw nothing there).
- Tests (`tests/transfers.rs`, 5): `F5` upload through a real key press with a focused table, download button, double-click a file, a running transfer in the queue + pause toggle + `Delete` removes it, tab switch and a log with 50 lines. `tests/tree.rs` now scopes its clicks with `window.within("server-tree")`: the tab bar's tabs and the tree rows both use plain integer ids.

### T7 Terminal element (done)
- **terminal / app-core additions (additive):** `filecargo_terminal` and the prelude re-export `vt100`'s `Cell`, `Color` and `Parser` (the GUI converts cells and its tests feed a parser).
- `terminal/runs.rs` (pure, 8 tests): `row_layout(screen, row, cols, default_fg, default_bg)` → `Segment`s (text + `(utf-8 bytes, Style)` runs, so runs always cover the text) and merged `Background` rectangles in cell units; xterm palette (16 named, 6×6×6 cube, 24 greys), true colour, inverse (uses the defaults), dim, bold / italic / underline; **a wide character is a segment of its own** so the monospace grid keeps its alignment.
- `terminal/mod.rs` (`TerminalView`): a `canvas` measures the cell (`em_advance` rounded to physical pixels, line height 1.25 × 13 px), paints background and selection quads, one `shape_line` per segment (`force_width` = the cell width, times the cells a wide glyph covers) and the cursor quad (dim when unfocused, hidden while scrolled back); the painted size is sent as `TerminalResize` when it changes; the first frame with the tab showing and `TerminalState::Closed` sends `TerminalOpen` (once per visit). **No own row cache:** gpui's line-layout cache already reuses shaped lines by text and runs, so unchanged rows are not re-shaped; the planned "< 4 ms at 200×60" **frame time is not measured** (the headless platform has no real renderer) — it stays on the smoke list.
- `terminal/keys.rs` (pure, 5 tests): gpui keystroke → `terminal::{Key, Mods}`: typed text wins (case, layout), ctrl / alt send the base key + modifier, function keys 1–12, `shift-tab` → back-tab, modifier-only keys dropped; bytes come from `TerminalHandle::encode_key` (the screen's modes apply). Paste uses `paste_bytes` (bracketed only when the remote asked, end marker stripped).
- **Key routing:** every key goes to the shell, so global shortcuts are bound with the context `!Terminal` (`secondary-q`, `secondary-,`), and `Tab` / `Shift-Tab`, which gpui binds to focus traversal, are re-bound in the `Terminal` context to actions that send the key. Copy / paste per the spec: with a mouse selection `secondary-c` (and `ctrl-shift-c`) copies, otherwise `ctrl-c` is a plain `^C`; `secondary-v` / `ctrl-shift-v` paste when the clipboard has text, otherwise `ctrl-v` goes to the shell. Mouse: click focuses and starts a selection, drag extends it (`selection_text`), the wheel → `TerminalScroll`. An exited shell shows its last screen and "Press Enter to reopen".
- Tests (`tests/terminal.rs`, 5, with the fake shell behind a real app-core session): the tab opens the shell and the painted grid size reaches it, typed keys arrive as `ESC[A`, `^C`, Tab, `x`, **`Ctrl-Q` reaches the shell instead of quitting**, coloured / wide-character output is drawn for several frames without trouble and lands in the emulator, an exited shell reopens on Enter.
- Not covered headlessly: pixels, the mouse selection and the clipboard (smoke list).
