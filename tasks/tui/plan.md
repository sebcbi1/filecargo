# Implementation Plan: `tui` module

> Spec: [SPEC-tui.md](../../SPEC-tui.md) · Tasks: [todo.md](todo.md) · Status: **awaiting review** · 2026-10-05
> Starts after `app-core` Checkpoint C. It is the first consumer of the app-core API, so API friction found here is fixed in app-core (spec first) before the GUI starts.

## Overview
`crates/filecargo-tui`, binary `filecargo-tui`. Three layers:
1. **`UiState` + reducer** (`on_event`): pure, holds all key handling
2. **view** (`render`): pure, snapshot-tested
3. **thin `main` loop**: terminal setup, the `select!` over input, state and tick

Work is sliced by screen area, so every task ends with a runnable binary that does something more.

## Architecture Decisions
| Decision | Rationale |
|---|---|
| **Pure reducer + pure view; I/O only in `main`** | Key handling and layout are tested without a terminal (`TestBackend`, insta). |
| **Loop runs on app-core's runtime via `handle.block_on(ui_loop)`** with crossterm `EventStream` | One runtime in the process; `select!` over input, `state.changed()` and a 4 Hz tick. |
| **Redraw only on change** (input, new snapshot generation, tick while transfers are active) | Low CPU over SSH; no flicker. |
| **Widgets:** ratatui `Table` for panes and queue, `tui-tree-widget` for servers, `tui-term` for the terminal, `Clear` + centered `Block` for dialogs | Uses maintained widgets; no custom low-level drawing except the progress bar cells. |
| **A small form toolkit in-crate** (text field, masked field, checkbox, select, button row) | ratatui has no form widgets; the site editor and prompts reuse the same ~300 lines. |
| **Key bindings live in one table** (`keymap.rs`) driving both the reducer and the help overlay | The help overlay can't drift from the real bindings. |

### Dependencies (new)
`ratatui = "0.30"`, `crossterm = "0.29"` (feature `event-stream`), `futures-util` (StreamExt), `tui-tree-widget = "0.24"`,
`tui-term = "0.3"`, `anyhow`; dev: `insta`. Verified API notes are in SPEC-tui "Implementation notes".

## Task List
### Phase 1: Walking skeleton
- [x] T1: Crate + bin, args (`--version`, `--config-dir`), `App::start`, terminal init/restore + panic hook (plus our own mouse-capture/bracketed-paste enable and disable), loop skeleton rendering an empty layout (S)
- [x] T2: Layout, local pane `Table`, `UiState` focus/cursor/selection, reducer for navigation and selection, too-small screen (M)
### Checkpoint A: binary browses local files; AC3, AC5 green; first snapshots reviewed — reached
### Phase 2: Servers and sessions
- [x] T3: Server tree (`tui-tree-widget`), tree bindings, connect/disconnect, remote pane (M)
- [x] T4: Form toolkit + site editor (new/edit) + new folder/rename/move/delete dialogs (M)
- [x] T5: Prompt dialogs for every `PromptKind` + chmod, go-to-path, import dialogs (M)
### Checkpoint B: connect to a site with password + host-key prompt, browse, and edit sites, all in the binary
### Phase 3: Transfers, log, terminal, polish
- [ ] T6: Bottom panel: Queue / Completed / Failed / Log tabs, progress bars, queue bindings, transfer bindings in panes (M)
- [ ] T7: Terminal tab (`tui-term`), key mapping to `terminal::Key`, focus escape, scrollback keys (S)
- [ ] T8: Help overlay from keymap, status line, mouse (wheel + click focus), `NO_COLOR`, complete snapshot suite, `SMOKE.md` + manual smoke on 3 OSes (M)
### Checkpoint C: all 6 AC green, CI green, smoke checklist done, human review

## AC Traceability
AC1 → T2–T8 (each task adds its rows of the binding table) · AC2 → T2–T8 (snapshots per screen) · AC3 → T2 · AC4 → T7 · AC5 → T1 · AC6 → T8

## Risks and Mitigations
| Risk | Impact | Mitigation |
|---|---|---|
| app-core API gaps found late | High | Use the API from T2 on; log gaps in hand-off notes; fix in app-core with a spec update before continuing. |
| Key chords unavailable in some terminals (`Alt-1`, `Shift-Tab`, F-keys over SSH, `Ctrl-\`) | Med | Every action also has a plain-letter binding or menu path; the smoke checklist covers macOS Terminal, iTerm2, a Linux terminal and Windows Terminal. |
| Snapshot churn makes tests noisy | Med | Snapshots at 100×30 and 80×24 only; deterministic fixture state (fixed times, sizes); review diffs with `cargo insta review`. |
| Terminal-in-terminal escape and redraw glitches | Med | `tui-term` handles rendering; focus escape uses a chord rarely used by shells; manual vim/htop check. |

## Open Questions
None.

## Hand-off Notes
_Appended per task during implementation._

### T1 Skeleton binary (done)
- Crate = lib `filecargo_tui` + bin `filecargo-tui` (all logic in the lib so it is testable). `main`: `--version`, `--help`, `--config-dir <path>` (→ `StartOptions.paths = Paths::from_override`, **no `set_var`**: the workspace forbids `unsafe`; `Paths` is now in app-core's prelude), `App::start`, `init_logging`, `guard::enter`, the loop on the app's runtime (`runtime().block_on`), `guard::leave`, `app.shutdown(3 s)`.
- `guard.rs`: `enter()` = `ratatui::init()` (raw mode, alternate screen, its own panic hook) **plus** mouse capture and bracketed paste, which `init` does not enable; our panic hook disables them first and chains to ratatui's. `leave()` undoes everything and is safe to call twice or without a terminal. `install_panic_restore(restore)` is the injectable piece AC5 tests.
- `app_loop.rs`: `tokio::select!` over crossterm's `EventStream`, `snapshots.changed()` (an `Err` means the app has quit → return) and a 250 ms tick that only marks the screen dirty while transfers or a connection are active. Key release events are dropped (`is_press() || is_repeat()`, needed on Windows). The renderer is a `&mut dyn FnMut` so a test can make it panic.
- Verified: `tests/panic.rs` (restore runs **before** the previous hook prints the message; `leave()` without a terminal); and by driving the real binary through a pseudo-terminal (`script`): it enters the alternate screen with mouse + paste, draws, exits 0 on `q`, and ends with the disable / leave / show-cursor sequences.

### T2 Layout + local pane (done) → Checkpoint A reached (AC3, AC5)
- `keymap.rs`: **one table** (`BINDINGS`: context, keys, help label, action); `lookup(context, key)` checks the focused context, then `Global`. Letters ignore the SHIFT modifier (terminals disagree on whether `N` carries it); `Shift-Tab` is accepted as `BackTab` or `Tab+SHIFT`. The help overlay (T8) will draw from the same table.
- `layout.rs` (pure): ≥ 80×24 or "terminal too small (needs 80x24)"; status line 1 row; bottom panel 30 % of the body (≥ 6 rows, `F10` gives it the whole body); tree 20 % with ≥ 18 columns, shown by default from 100 columns (`F9` overrides); panes split the rest evenly.
- `UiState`: focus, size, `PaneUi { cursor, offset, selected: BTreeSet<String>, seen: (path, generation) }` per pane, `now` and `home` injected (drawing stays deterministic), `color` from `NO_COLOR`. Row 0 of a pane is the `..` row when the directory has a parent (app-core's entries never contain it).
- **Selection rule (AC3)** in `reducer::sync`: same path + new generation (refresh, re-sort) → selection pruned to names that still exist and the cursor follows its name; different path → cursor 0, nothing selected; a vanished remote pane resets its state and moves the focus to Local.
- Reducer (file panes): cursor keys (Up/Down/PgUp/PgDn/Home/End + `j`/`k`), `Enter` (dir → `Navigate`, `..` → `Up`, **file → Upload/Download of that file**), `Backspace`, `Space`/`Ins`, `*`, `Ctrl-a`, `F5`/`t` (selection or cursor row, clears the selection), `F8`/`Del` (remote only), `Ctrl-r`, `.` (flips `show_hidden` through `UpdateSettings`), `s` (cycles Name↑↓ → Size↑↓ → Modified↑↓), `Tab`/`Shift-Tab` over what exists, `F9`, `F10`, `q`/`Ctrl-q`. Dialog-based keys (`F7`, `F2`, `c`, `g`, `F1`, `Alt-n`) arrive with their tasks.
- View: bordered panes with `Name / Size / Modified` tables (dirs `name/` bold blue, selected rows `*` + bold yellow, cursor row reversed when focused and underlined otherwise, `NO_COLOR` falls back to bold/reverse/markers), local title with `~`, remote placeholder, a pane error in red on its last row. **Times are shown in UTC** (no local-time conversion without a time-zone dependency; revisit when the GUI needs it).
- Tests: 27 (`layout` 5, `pane` formatting 3, `reducer` 13 incl. AC3 cases, 6 view tests with 4 reviewed `insta` snapshots: disconnected 100×30 / 80×24, connected, too small; style checks for cursor/selection/NO_COLOR). Driven through a pseudo-terminal: exits cleanly after cursor keys + `q`.

### T3 Server tree + remote pane (done)
- **Deviation:** the tree is **not** drawn with `tui-tree-widget`. `tree.rs` flattens `ServerTree` into `TreeRow`s (depth, name, folder/site kind) over a `HashSet<FolderId>` of open folders, and the view renders them as a plain list. That keeps cursor, expansion and "go to parent" in a pure, unit-tested model and avoids the widget's borrowed-text rebuild per frame; the dependency was removed. (The spec's note about node ids as `Vec<String>` paths no longer applies.)
- Keymap gained the whole **Tree** context (Up/Down/PgUp/PgDn/Home/End, Right/Left, Enter, `x`, `n`, `N`, `e`, `r`, `D`, `m`, `Del`, `i`, plus `h j k l`). The table is `#[rustfmt::skip]` so it stays one binding per line. Implemented now: movement, expand / collapse (`Left` on a site or closed folder jumps to the parent), `Enter` (site → `Connect`, folder → toggle), `x` → `Disconnect`, `D` → `Tree(Duplicate)`; `n N e r m Del i` open dialogs in T4/T5.
- The focus moves from the tree to the remote pane when a connection comes up (once; it does not keep stealing it). Deleting folders elsewhere prunes the expansion set and clamps the cursor.
- View: `Servers` list (`▾`/`▸` folders in bold, `●` connected in green, `◌` connecting, `✗` failed, empty-tree hint), remote title carries the state (`Remote sftp://host/path`, `Remote (authenticating prod-web…)`, `Remote (failed: …)`, `Remote (not connected)`), placeholder text per state. The `site_id` / `sample_tree` fixtures live in `test_support`.
- 39 unit tests (12 new reducer tests for the tree and 4 new snapshots: disconnected with tree, connecting, connected listing, failed).

### T4 Forms + site editor + simple dialogs (done)
- `form.rs`: `Form` of `Field`s (text, masked, checkbox, select) with one focus order over the *visible* fields (`Tab`/`Down`, `BackTab`/`Up`), cursor editing by character (Left/Right/Home/End/Backspace/Delete, `Ctrl-u` clears), `Space` toggles / cycles, `Enter` submits, `Esc` cancels, paste inserts printable text. An error is shown under the fields and cleared by the next key.
- `dialog.rs`: `Dialog::{Site, Input, Confirm, Move}`, each turning keys into `Outcome::{Keep, Close, Run(Vec<Command>)}` (pure). `UiState.dialog` takes **every** key and paste while open (so `q` types a `q`).
  - Site editor: name, protocol, host, port (blank = default), user, login (Password / Key file / Agent / Anonymous), password (masked), key file, "remember secret", FTP mode, remote dir, local dir, notes. Fields that do not apply to the chosen login / protocol are hidden. Validation: name, host, port 1–65535, key path, login vs protocol. Editing keeps the site id. A typed password is sent as `SetSitePassword` **only** when "remember" is on; an empty password keeps the stored one. The key passphrase is not asked in the editor: it is prompted at connect time (T5).
  - Input: new tree folder, rename node, new remote folder (`F7`), rename remote (`F2`, pre-filled); name must be non-empty and contain no `/`. Confirm: tree delete (`y`/`Enter`, `n`/`Esc`). Move picker: the root plus every folder except the node and its descendants, starting on the current parent.
  - New sites / folders go into the folder under the cursor (or the site's folder).
- `dialog_view.rs`: centered boxed overlay (`Clear` + bordered block), reversed cursor cell in text fields, bullets for masked text.
- Remote `F8` delete still goes straight to `Command::Delete`: app-core already asks for confirmation (`confirm_delete`), shown by the T5 prompt dialogs.
- 27 new tests (form 7, dialog 8, reducer 8, snapshots 5 → site editor, validation error with masked password, confirm, move picker, input).

### T5 Prompts and utility dialogs (done)
- **App prompts** (`prompt_ui.rs`, `prompt_view.rs`): `UiState.prompt: Option<PromptSlot { id, ui, answered }>` is synced to `AppState.prompt` (new id → fresh slot; none → cleared). While a prompt shows it receives *every* key and paste (above any open dialog); the answer goes out as `Command::Answer` and the slot is marked `answered`, so the still-unprocessed snapshot neither redraws the prompt nor accepts a second answer.
  - Credential (password / passphrase / keyboard-interactive): masked fields (echoed when the server says so), `Remember` checkbox except for keyboard-interactive, `Enter` answers, `Esc` → `Credential(None)`; a retry note is shown. Up to 16 keyboard-interactive prompts.
  - Host key / certificate: `y` once, `a` always, `n`/`Esc` reject. **No default key** — `Enter` does nothing, so a trust decision is never one stray key away.
  - Conflict: `o` overwrite, `n` if newer, `r` resume, `s` skip, `k` keep both (rename), `a` toggles "apply to all", `Esc` skips (`Dismiss`).
  - Confirm delete / quit: `y`/`Enter`, `n`/`Esc`. Message: `Enter`/`Esc`/`Space` dismiss; level picks the colour.
- **Utility dialogs** (`dialog_util.rs`, plus `InputPurpose::GoTo`): `g` go to path (either pane, pre-filled with the current path, slashes allowed), `c` chmod (remote only; octal text and the nine rwx boxes stay in step both ways, special bits typed in octal survive a box toggle, `Enter` → `Command::Chmod`), `i` import (path pre-filled with `~/.config/filezilla/sitemanager.xml`, "Import passwords" box → `Command::ImportFileZilla`).
- `Field.id` / `label` became `String` (prompt texts are dynamic).
- 18 new tests (prompt_ui 6, dialog_util 5, reducer 5, snapshots: one per prompt kind ×8 plus chmod, go-to, import).
