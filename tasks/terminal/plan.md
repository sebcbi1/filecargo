# Implementation Plan: `terminal` module

> Spec: [SPEC-terminal.md](../../SPEC-terminal.md) · Tasks: [todo.md](todo.md) · Status: **complete; awaiting final human review** · 2026-10-05
> Starts after `remote-fs` Checkpoint B (`ShellOpener` / `ShellChannel` exist). Can run in parallel with `transfer`.

## Overview
`crates/filecargo-terminal`: a pure key encoder plus a `TerminalHandle` that owns
`Arc<Mutex<vt100::Parser>>` and a feeder task bridging a `ShellChannel`. Almost everything is
tested against an in-memory fake channel; one integration test uses the docker SSH server.

## Architecture Decisions
| Decision | Rationale |
|---|---|
| **`vt100` parser shared behind `std::sync::Mutex`**, UIs read via `with_screen` | Rendering is a short critical section; `tui-term` takes `&vt100::Screen` directly; the GUI element reads cells the same way. No per-frame grid copy. |
| **`encode` is a pure function of (Key, Mods, Modes)** | Exhaustively table-testable; both UIs share identical key semantics. |
| **Resize coalescing in the feeder task** (latest size wins, ≤ 10/s) | Window drags in the GUI fire many resizes; the SSH server only needs the last one. |
| **`generation` counter (AtomicU64)** bumped after each `process` | Cheap "did anything change?" check for both UIs. |

### Dependencies (new)
`vt100 = "0.16"`, `tokio` (sync, time, rt), `tracing`.

## Task List
- [x] T1: Crate + `Key` / `Mods` / `Modes` + `encode` with xterm table tests (S)
- [x] T2: `spawn` + `TerminalHandle` over a fake channel: feed, generation, resize coalescing, paste (bracketed or not), scrollback + return-to-live, exit status, close (M)
- [x] T3: Integration vs docker SSH: `printf`, resize + `stty size` (S)
### Checkpoint: all 7 AC green, coverage ≥ 80 %, human review

## AC Traceability
AC1 → T1 · AC2–6 → T2 · AC7 → T3

## Risks and Mitigations
| Risk | Impact | Mitigation |
|---|---|---|
| vt100 0.16 lacks something a UI needs (e.g. wide-char or underline-color info) | Low | Fidelity issues are accepted for v1 (spec decision); `alacritty_terminal` remains the fallback behind the same `TerminalHandle` API. |
| Mutex contention between the feeder and the UI during heavy output (`cat bigfile`) | Low | Feed in chunks ≤ 64 KiB per lock; the UI redraw rate is capped by app-core. |

## Open Questions
None.

## Hand-off Notes
_Appended per task during implementation._

### T1 Key encoding (done)
- `keys.rs`: `Key`, `Mods` (`NONE/CTRL/ALT/SHIFT` consts), `Modes`, pure `encode`. `vt100` is pinned `=0.16.2` in the workspace. The test table has ~65 rows (every variant × the modifier combinations that matter) asserted in **both** cursor modes; a compile-time exhaustiveness check forces new `Key` variants into it.
- xterm decisions made explicit: Shift+Tab = `CSI Z`; Ctrl+Backspace = `0x08`; Ctrl+Enter/Shift+Enter = `\r`; modified cursor keys use `CSI 1;<m>X` even in application mode; F1–F4 modified = `CSI 1;<m>P..S`; `F(0)` / `F(13+)` encode to nothing; `Modes::application_keypad` is carried for the API but no `Key` is a keypad key in v1.

### T2 TerminalHandle (done)
- `handle.rs`: `spawn(channel, size, scrollback)` (inside a tokio runtime) → `TerminalHandle` over `Arc<Shared { Mutex<vt100::Parser<TermCallbacks>>, AtomicU64 generation, Mutex<TermStatus> }>`. The feeder task `select!`s output, resize changes and a deferred-resize timer; output is fed in ≤ 64 KiB chunks, each followed by a generation bump.
- **Resize coalescing:** the parser is resized immediately; the window-change goes out at once if none was sent in the last 100 ms, otherwise one deferred send at +100 ms carries the latest size (10 resizes in 50 ms → 2 messages, the second with the final size).
- **API deviations (spec updated):** `send_key(key, mods)` (the spec sketch lost `Mods`), plus `bell_count()` for the GUI flash; `selection_text` takes `(row, col)` ends in either order. `scroll(n)` is relative (positive = back into history), `scroll(0)` = live; typing, text and paste return to live.
- **Paste hardening:** in bracketed mode any `ESC[201~` inside the pasted text is stripped, so pasted content cannot end the bracket early and have the rest executed as typed input. Unbracketed paste turns `\r\n` / `\n` into `\r`.
- After `Exit(code)` the status is `Exited(code)` (a later `Closed` does not overwrite it), the screen stays readable and all input is dropped; a channel that disappears without an exit gives `Closed`.
- 13 tests against an in-memory fake channel (AC2–AC6 plus title/bell/selection, split escape sequences, 300 KB of output, close, keys in application-cursor mode); resize tests use paused time.

### T3 Docker integration (done) → module complete (human review pending)
- `tests/integration.rs` (`--features integration`): real `remote_fs::connect` + `ShellOpener::open`, `spawn`, type `printf 'ok\n'` and wait for a row that is exactly `ok` (not the command echo), `resize(100×30)`, then `stty size` shows `30 100` and the parser is 30×100. ~0.5 s. `cargo it` now covers three crates.
- Coverage (`cargo llvm-cov -p filecargo-terminal --features integration`): **96.9 % lines**. fmt and clippy (`-D warnings`) clean for the workspace.

### AC → tests
| AC | Covered by |
|---|---|
| 1 key table | `keys::tests` (~65 rows × both cursor modes) |
| 2 output → screen, generation | `tests/handle.rs::output_reaches_the_screen…`, split escape sequences |
| 3 resize coalescing | `ten_resizes_in_fifty_milliseconds…` (paused time) |
| 4 bracketed paste | `paste_is_bracketed_only_when…`, `pasted_text_cannot_close_the_bracket_early` |
| 5 scrollback | `scrollback_shows_earlier_lines_and_typing_returns_to_live` |
| 6 exit | `exit_keeps_the_screen_and_drops_later_input` |
| 7 docker shell | `tests/integration.rs` |
