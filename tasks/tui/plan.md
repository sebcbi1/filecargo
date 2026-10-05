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
- [ ] T2: Layout, local pane `Table`, `UiState` focus/cursor/selection, reducer for navigation and selection, too-small screen (M)
### Checkpoint A: binary browses local files; AC3, AC5 green; first snapshots reviewed
### Phase 2: Servers and sessions
- [ ] T3: Server tree (`tui-tree-widget`), tree bindings, connect/disconnect, remote pane (M)
- [ ] T4: Form toolkit + site editor (new/edit) + new folder/rename/move/delete dialogs (M)
- [ ] T5: Prompt dialogs for every `PromptKind` + chmod, go-to-path, import dialogs (M)
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
