# Implementation Plan: v1.1

> Spec: [SPEC-v1.1.md](../../SPEC-v1.1.md) · Tasks: [todo.md](todo.md) · Status: **implemented**

## Overview

This plan covers four features from TODO.md, built in the spec's order:
1. `tui-backtab`: fix Shift-Tab in the TUI.
2. `local-ops`: file operations on the local pane.
3. `site-scoped-panel`: a bottom panel per connection that shows both paths.
4. `staged-queue`: held items, start, pause and clear per site, and held items restored after a restart.

Every task is a vertical slice that leaves the workspace building and green. Each feature lands
in the core first (`transfer` / `app-core`, with tests), then in the TUI, then in the GUI.

## Dependency graph

```
tui-backtab (T1)                       standalone

app-core pane-aware ops (T2) ──┬── TUI local ops (T3)
                               └── GUI local ops (T4)

log site tagging (T5) ──┐
                        ├── app-core scoped view (T6) ──┬── TUI scoped panel (T7)
transfer QueueItem.site ┘   (already exists)            └── GUI scoped panel (T8)

transfer Held + start_held (T9) ── per-site pause/clear (T10) ── persistence (T11)
        └──────────────────────────────┬────────────────────────────┘
                     app-core queue commands (T12, needs T6)
                          ├── TUI queue keys (T13)
                          └── GUI queue buttons (T14)
                                    └── integration test + spec/doc updates (T15)
```

## Architecture decisions

### Local operations reuse the remote op path

- For `PaneId::Local`, the op runs on a `RootedFs::new(<local pane dir>)` handed to the
  existing `spawn_op`.
- Recursive delete, rename, mkdir and chmod then share code, error mapping and notices with the
  remote pane. Only the refresh target differs.
- `RootedFs` already maps errors per platform. On Windows its chmod result doesn't matter: the
  UIs never offer chmod there, and app-core rejects it with a notice.

### Commands get `pane: PaneId`

- `Mkdir`, `Rename`, `Delete` and `Chmod` gain a `pane` field. This changes them for both
  front-ends.
- T2 updates every TUI/GUI call site to pass `PaneId::Remote`, so behaviour doesn't change until
  T3 and T4. These edits are mechanical and don't count toward the task's file budget.

### Log scoping is done with tracing spans

- `LogLine` gains `site: Option<SiteId>`. The log layer takes it from the nearest span that has a
  `site` field.
- app-core wraps session handling in a `site` span. `transfer` wraps each worker job in a `site`
  span. That's the one cross-crate touch, and it needs no new API.

### Scoping lives in app-core, not in the front-ends

- `AppState` gains:
  - `scope: Option<SiteId>`: the connected site.
  - `site_queue: Arc<QueueSnapshot>`: the full snapshot filtered to the scope; empty when
    disconnected.
  - `other_sites_active: usize`.
- It's rebuilt only when the queue snapshot or the scope changes, comparing Arc pointers as the
  UIs already do.
- `AppState.queue` stays the full snapshot, for quit counting.
- The log is filtered at read time with `LogBuffer::lines_for(scope)`.

### Held is a new `ItemState`

- The scheduler skips held items exactly as it skips pending items when processing is off.
- Per-site pause is a `HashSet<SiteId>` in the scheduler. The global `set_processing` stays in
  the `transfer` API, but the UIs no longer use it.

### queue.json

- The format gains `"version": 2` and a `Held` state. A file without a version is treated as v1.
- On load, both versions turn Pending and interrupted items into Held. A fixture of a real v1
  file is checked in.

### Clear queue

- Clear cancels the site's active items through the existing remove path (partial files are
  kept) and removes its pending and held items.
- app-core asks first with a new `ConfirmClearQueue` prompt kind. Both UIs render it the same
  way they render `ConfirmDelete` (a small addition in T13 and T14).

## Task list

See [todo.md](todo.md) for acceptance criteria, verification and files.

### Phase 1: Shift-Tab and local operations
- [x] T1: TUI Shift-Tab moves focus backwards
- [x] T2: app-core file ops take a pane; local mkdir/rename/delete/chmod
- [x] T3: TUI file-op keys act on the focused pane
- [x] T4: GUI local pane menu and toolbar act on the focused pane

**Checkpoint A:**
- fmt, clippy (incl. GUI) and the full test suite pass.
- Manual: in both UIs, mkdir/rename/delete/chmod on the local pane.

### Phase 2: Bottom panel per connection
- [x] T5: Log lines carry the site they belong to
- [x] T6: app-core publishes the scoped queue and the count of other sites' active transfers
- [x] T7: TUI bottom panel is scoped, shows both paths and the other-sites hint
- [x] T8: GUI bottom panel is scoped, shows both paths and the other-sites hint

**Checkpoint B:**
- Full suite passes.
- Manual: connect to A with B transferring in the background. The panel shows A only, and the
  hint shows B's count. When disconnected the lists are empty.

### Phase 3: Staged queue
- [x] T9: transfer: Held state, enqueue-held and start-held
- [x] T10: transfer: per-site pause, clear and clear-failed
- [x] T11: transfer: held items persist; restored items come back held; v1 files load
- [x] T12: app-core queue commands (Enqueue, StartHeld, SetSitePaused, Clear with confirm, ClearFailed)
- [x] T13: TUI `a` / `S` / `p` / `X` and Clear failed
- [x] T14: GUI Add to queue, Start queue, pause toggle and Clear queue
- [x] T15: Integration round trip (enqueue → restart → start) and spec/doc updates

**Checkpoint C (complete):**
- Every success criterion in SPEC-v1.1.md is met.
- `cargo it` passes on fresh containers.
- Coverage is no lower than v1.
- CI is green after the user pushes.

## Parallelization

- T1 is independent.
- After T2, T3 and T4 can run in parallel.
- After T6, T7 and T8 can run in parallel.
- T9–T11 are sequential, since they're in the same files.
- T13 and T14 can run in parallel after T12.
- A single agent should still go in order: the front-end tasks share the help, snapshot and
  test-support files.

## Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Worker log events miss the `site` span (spawned tasks don't inherit spans) | Med: B's lines show up as app-wide | Instrument the worker future with `.instrument(span)`; T5 tests a line emitted inside a spawned worker |
| Adding fields to `Command` breaks both front-ends in one task | Med | T2 does the call-site updates mechanically, with `PaneId::Remote`, and builds the GUI under nix-shell before the commit |
| Filtering a large queue on every snapshot | Low | Rebuild only on Arc pointer change; at most a few thousand items |
| v1 `queue.json` compatibility | High if broken: the user's queue would be lost | Checked-in v1 fixture test; `serde(default)` on the new fields; never rewrite the file before it parses |
| Per-site pause vs. the global processing flag interact badly | Med | The global flag stays at `true` from the UIs; scheduler tests cover pause A while B runs |
| Clear cancelling active transfers loses data | Med | Use the existing remove/cancel path (partial files kept); confirm first; app-core test |
| The known flaky FTP 1000-file integration test | Low (CI noise) | Out of scope; re-run once and report, as before |
| GUI tests on macOS/Windows compare temp paths | Low | Use `support::canonical` in every new GUI test |

## Open questions

None blocking. The spec's review assumptions are taken as defaults unless you change them:
- Local delete is permanent.
- Clear cancels the site's active items.
- The pause flags are not persisted.
- Keys: `a`, `S`, `X`.
