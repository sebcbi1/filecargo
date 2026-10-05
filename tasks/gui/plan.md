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
- [ ] T3: Server tree view, connect/disconnect, remote pane, toolbar with session status (M)
- [ ] T4: Dialog infrastructure, site editor, tree context-menu operations (new, rename, duplicate, move, delete, import) (M)
- [ ] T5: Prompt dialogs for every `PromptKind`, permissions dialog, settings dialog, notices → notifications (M)
### Checkpoint B: connect over SFTP with host-key and password prompts, edit sites, change settings
### Phase 3: Transfers, terminal, polish
- [ ] T6: Bottom panel (`TabBar`), Queue / Completed / Failed tables with progress, Log `uniform_list`, transfer actions (F5, double-click, menus), batch-complete OS notification (M)
- [ ] T7: Terminal element (cells, runs, cursor, row cache), key mapping, clipboard, resize → `TerminalResize` (M)
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
