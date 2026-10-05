# Implementation Plan: `terminal` module

> Spec: [SPEC-terminal.md](../../SPEC-terminal.md) · Tasks: [todo.md](todo.md) · Status: **awaiting review** · 2026-10-05
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
- [ ] T1: Crate + `Key` / `Mods` / `Modes` + `encode` with xterm table tests (S)
- [ ] T2: `spawn` + `TerminalHandle` over a fake channel: feed, generation, resize coalescing, paste (bracketed or not), scrollback + return-to-live, exit status, close (M)
- [ ] T3: Integration vs docker SSH: `printf`, resize + `stty size` (S)
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
