# Tasks: v1.1

> Plan: [plan.md](plan.md) · Spec: [SPEC-v1.1.md](../../SPEC-v1.1.md)

**Every task:**
- Write the failing test first.
- Run `cargo fmt --all --check`, `cargo clippy --workspace --exclude filecargo-gui --all-targets -- -D warnings` and `cargo test`.
- For tasks that touch app-core or gui, also run
  `nix-shell --run "cargo clippy -p filecargo-gui --all-targets -- -D warnings && cargo test -p filecargo-gui"`.
- Make one `jj commit` per task, with no attribution lines.

---

## Phase 1: Shift-Tab and local operations

### T1: TUI Shift-Tab moves focus backwards
**Description:** crossterm reports Shift-Tab as `BackTab` + `SHIFT`, which no binding matches.
Make `BackTab` match with or without `SHIFT`, both in the keymap and in dialog forms.
**Acceptance:**
- [x] `lookup(Global, BackTab+SHIFT)` and `lookup(Global, BackTab)` both give `FocusPrev`.
- [x] Dialog forms move to the previous field on `BackTab+SHIFT`.
- [x] The terminal tab still sends `BackTab` to the shell.
**Verify:**
- [x] `cargo test -p filecargo-tui`
- [ ] (pending, manual) Manual: Shift-Tab in kitty/xterm/tmux cycles backwards.
**Dependencies:** none
**Files:** `crates/filecargo-tui/src/keymap.rs`, `crates/filecargo-tui/src/form.rs`, `crates/filecargo-tui/src/reducer.rs` (tests)
**Scope:** XS

### T2: app-core file ops take a pane; local mkdir/rename/delete/chmod
**Description:** Add `pane: PaneId` to `Mkdir`, `Rename`, `Delete` and `Chmod`.
- For `Local`, the op runs on `RootedFs::new(local dir)` through `spawn_op` and refreshes the
  local pane.
- Delete honours `confirm_delete`.
- Chmod on the local pane is rejected with a notice on Windows.
- Update every TUI/GUI call site to `PaneId::Remote` (behaviour unchanged).
**Acceptance:**
- [x] Local mkdir, rename and recursive delete change the disk, and the local pane refreshes.
- [x] Local delete asks first when `confirm_delete` is on, and a "no" leaves the files untouched.
- [x] Local chmod sets the mode (`cfg(unix)`).
- [x] Invalid names are rejected, as on the remote pane.
- [x] Remote ops behave exactly as before: the existing `remote_ops` tests pass unchanged except
  for the new field.
**Verify:**
- [x] `cargo test -p filecargo-app-core --test local_ops --test remote_ops`
- [x] GUI builds under nix-shell.
**Dependencies:** none
**Files:** `crates/filecargo-app-core/src/{command,ops,app}.rs`, `crates/filecargo-app-core/tests/local_ops.rs` (new), plus mechanical call-site edits in tui/gui
**Scope:** M

### T3: TUI file-op keys act on the focused pane
**Description:** F7, F2, F8/Del and `c` send the pane that has the focus.
- On Windows, `c` on the local pane shows a status hint instead.
- Help text says "current pane".
**Acceptance:**
- [x] On the local pane, F7, F2 and Del send `pane: Local` commands with the cursor or selection
  names.
- [x] `c` opens the permissions dialog for local entries on Unix.
- [x] The remote pane is unchanged.
**Verify:**
- [x] `cargo test -p filecargo-tui`
- [ ] (pending, manual) Manual: TUI local-pane ops work.
**Dependencies:** T2
**Files:** `crates/filecargo-tui/src/{reducer,dialog,help}.rs`, snapshots
**Scope:** S

### T4: GUI local pane menu and toolbar act on the focused pane
**Description:**
- The local pane's context menu gets New folder, Rename, Delete and Permissions… (Permissions on
  Unix only).
- The toolbar's Folder, Rename and Delete buttons act on the last-focused pane.
**Acceptance:**
- [x] A headless test of each local menu entry sends the matching `pane: Local` command, and the
  disk changes.
- [x] Toolbar Delete with the local pane focused deletes local files after a confirm.
- [x] Remote behaviour is unchanged.
**Verify:**
- [x] `nix-shell --run "cargo test -p filecargo-gui"`
**Dependencies:** T2
**Files:** `crates/filecargo-gui/src/{pane,toolbar,workspace}.rs`, `crates/filecargo-gui/tests/local_ops.rs` (new, using `support::canonical`)
**Scope:** M

### Checkpoint A
- [x] Full suite green (default, GUI, clippy, fmt).
- [ ] (pending, manual) local mkdir/rename/delete/chmod in both UIs.
- [ ] Human review: pending (auto mode continues).

---

## Phase 2: Bottom panel per connection

### T5: Log lines carry the site they belong to
**Description:**
- `LogLine.site: Option<SiteId>`, filled from the nearest span that has a `site` field.
- app-core wraps session work (connect, listing, remote ops) in `site` spans.
- `transfer` instruments each worker job with `info_span!("job", site = %item.site)`.
- `LogBuffer::lines_for(scope: Option<SiteId>)` returns the lines of that site plus app-wide
  lines. With `None`, it returns app-wide lines only.
**Acceptance:**
- [x] A line logged inside a connect is tagged with the site.
- [x] A line from a spawned transfer worker is tagged with the item's site.
- [x] A startup line is untagged.
- [x] `lines_for` filters correctly.
**Verify:**
- [x] `cargo test -p filecargo-app-core --test logging`
- [x] `cargo test -p filecargo-transfer`
**Dependencies:** none
**Files:** `crates/filecargo-app-core/src/{logging,session}.rs`, `crates/filecargo-transfer/src/worker.rs`, `crates/filecargo-app-core/tests/logging.rs`
**Scope:** M

### T6: app-core publishes the scoped queue and the other-sites count
**Description:** Add `AppState.scope`, `site_queue` and `other_sites_active`.
- They're recomputed when the queue snapshot (by Arc pointer) or the session changes.
- When disconnected, `site_queue` is empty.
**Acceptance:**
- [x] Connected to A with items for A and B, `site_queue` holds A's only, and
  `other_sites_active` counts B's active items.
- [x] After a disconnect, `site_queue` is empty and `other_sites_active` counts all active items.
- [x] Quit confirmation still counts every site.
**Verify:**
- [x] `cargo test -p filecargo-app-core --test transfers --test quit`
**Dependencies:** T5 (same scope notion; log filtering uses it)
**Files:** `crates/filecargo-app-core/src/{state,transfers,session}.rs`, `crates/filecargo-app-core/tests/transfers.rs`
**Scope:** M

### T7: TUI bottom panel scoped, both paths and the other-sites hint
**Description:**
- The Queue, Completed and Failed tabs render `site_queue`.
- Rows show direction, local path, remote path, size and progress/error. Long paths are
  truncated from the left.
- The Log tab uses `lines_for(scope)`.
- The status bar shows "N transfers on other sites" when N > 0.
- The tab counts are scoped.
**Acceptance:**
- [x] insta snapshots: a scoped queue with both paths, empty lists when disconnected, the hint.
- [x] Left truncation is unit-tested.
**Verify:**
- [x] `cargo test -p filecargo-tui`
- [x] `cargo insta review` (no pending snapshots).
**Dependencies:** T6
**Files:** `crates/filecargo-tui/src/{bottom_view,view,reducer}.rs`, `crates/filecargo-tui/src/test_support.rs`, snapshots
**Scope:** M

### T8: GUI bottom panel scoped, both paths and the other-sites hint
**Description:**
- The queue tables use `site_queue`, with Local and Remote path columns.
- The log view uses `lines_for(scope)`.
- The toolbar shows the other-sites hint.
- The tab labels count scoped items.
**Acceptance:**
- [x] A headless test: items for A and B, connected to A, show only A's rows, and both path
  columns render.
- [x] Disconnected shows empty tables.
- [x] The hint text is present when B is active.
**Verify:**
- [x] `nix-shell --run "cargo test -p filecargo-gui"`
**Dependencies:** T6
**Files:** `crates/filecargo-gui/src/bottom/{mod,queue,log}.rs`, `crates/filecargo-gui/src/toolbar.rs`, `crates/filecargo-gui/tests/transfers.rs`
**Scope:** M

### Checkpoint B
- [x] Full suite green.
- [ ] (pending, manual) B transfers in the background while connected to A, and the panel shows only A.
- [ ] Human review: pending.

---

## Phase 3: Staged queue

### T9: transfer: Held state, enqueue-held and start-held
**Description:**
- Add `ItemState::Held`.
- `Queue::enqueue_held(...)` expands and adds items the same way as `enqueue`, but every
  resulting item is Held.
- `Queue::start_held(site)` turns that site's Held items into Pending.
- The scheduler never starts a Held item.
**Acceptance:**
- [x] Held items stay held with free workers.
- [x] `start_held(A)` starts A's items only.
- [x] Directory items expand into Held children.
- [x] Snapshot lists include held items in `pending`.
**Verify:**
- [x] `cargo test -p filecargo-transfer --test scheduler`
**Dependencies:** none
**Files:** `crates/filecargo-transfer/src/{model,queue,scheduler}.rs`, `crates/filecargo-transfer/tests/scheduler.rs`
**Scope:** M

### T10: transfer: per-site pause, clear and clear-failed
**Description:**
- `set_site_paused(site, bool)`: a paused site starts nothing new, and active items finish.
- `clear(site)`: cancels the site's active items through the remove path, keeping partial files,
  and removes its pending and held items.
- `clear_failed(site)` and a site-scoped `clear_completed(site)`.
- The snapshot exposes `paused_sites`.
**Acceptance:**
- [ ] With A paused, B keeps scheduling.
- [ ] After `clear(A)`, A has no pending, held or active items, B is untouched, and A's partial
  file is still on disk.
- [ ] `clear_failed(A)` removes A's failed items only.
**Verify:**
- [ ] `cargo test -p filecargo-transfer --test scheduler --test resume`
**Dependencies:** T9
**Files:** `crates/filecargo-transfer/src/{queue,scheduler}.rs`, `crates/filecargo-transfer/tests/scheduler.rs`
**Scope:** M

### T11: transfer: held items persist, restored items come back held, v1 files load
**Description:**
- `queue.json` gains `version: 2` and a `Held` state.
- On load, Pending and interrupted items (v1 or v2) become Held. Failed items stay Failed.
- Check in a fixture of a v1 file.
**Acceptance:**
- [ ] Round trip: held, pending and failed items are saved, then loaded as held, held and failed.
- [ ] The v1 fixture loads, and its pending items become held.
- [ ] After a restart nothing starts until `start_held`.
**Verify:**
- [ ] `cargo test -p filecargo-transfer --test restart`
- [ ] Unit tests in `store.rs`.
**Dependencies:** T9
**Files:** `crates/filecargo-transfer/src/store.rs`, `crates/filecargo-transfer/tests/restart.rs`, `crates/filecargo-transfer/tests/fixtures/queue-v1.json` (new)
**Scope:** S

### T12: app-core queue commands
**Description:**
- New commands: `Enqueue { from, names }`, `QueueStartHeld`, `QueueSetSitePaused(bool)`,
  `QueueClear` (asks with the new `PromptKind::ConfirmClearQueue`) and `QueueClearFailed`.
- `QueueClearCompleted` becomes site-scoped.
- All of them act on the connected site and are ignored with a notice when disconnected.
- `AppState` exposes whether the current site is paused.
**Acceptance:**
- [ ] Enqueue on 3 files leaves 3 held items and nothing transfers.
- [ ] StartHeld transfers them.
- [ ] Pausing A leaves B running.
- [ ] Clear asks first: "no" keeps everything, "yes" empties A only.
- [ ] Commands while disconnected produce a notice and no change.
**Verify:**
- [ ] `cargo test -p filecargo-app-core --test transfers --test prompts`
**Dependencies:** T6, T10, T11
**Files:** `crates/filecargo-app-core/src/{command,transfers,state,prompt}.rs`, `crates/filecargo-app-core/tests/transfers.rs`
**Scope:** M

### T13: TUI `a` / `S` / `p` / `X` and Clear failed
**Description:**
- `a` in a file pane sends Enqueue.
- In the Queue tab: `S` starts held, `p` toggles the site pause (replacing the global toggle),
  and `X` clears (with the confirm prompt rendered as a confirm dialog).
- Clear failed goes in the Failed tab (`C`, like Clear completed in Completed).
- Held rows show "queued", and the tab header shows "paused".
**Acceptance:**
- [ ] Reducer tests for each key → command.
- [ ] Snapshots of held rows, the paused header and the clear-confirm dialog.
- [ ] Help lists the new keys.
**Verify:**
- [ ] `cargo test -p filecargo-tui`
**Dependencies:** T12
**Files:** `crates/filecargo-tui/src/{keymap,reducer,bottom_view,prompt_ui}.rs`, snapshots
**Scope:** M

### T14: GUI Add to queue, Start queue, pause toggle and Clear queue
**Description:**
- The toolbar gets an "Add to queue" button, and the pane context menu gets "Add to queue".
- The Queue tab gets Start queue, a Pause/Resume toggle (per site) and Clear queue (confirm
  dialog).
- The Failed tab menu gets Clear failed.
- Held rows show "Queued".
**Acceptance:**
- [ ] Headless tests: Add to queue sends Enqueue, and Start sends StartHeld.
- [ ] The toggle sends SetSitePaused.
- [ ] Clear opens the confirm, and confirming sends the answer.
- [ ] The ConfirmClearQueue prompt renders.
**Verify:**
- [ ] `nix-shell --run "cargo test -p filecargo-gui"`
**Dependencies:** T12
**Files:** `crates/filecargo-gui/src/{toolbar,pane,prompts}.rs`, `crates/filecargo-gui/src/bottom/{mod,queue}.rs`, `crates/filecargo-gui/tests/transfers.rs`
**Scope:** M

### T15: integration round trip and spec/doc updates
**Description:**
- Add a Docker SFTP test: enqueue held items, drop and reopen the queue from disk, check that
  they're held, start them, and check the files arrived.
- Update SPEC-transfer, SPEC-app-core, SPEC-tui and SPEC-gui for the new contracts, the two
  SMOKE.md files and the README key list.
- Mark SPEC-v1.1 implemented.
**Acceptance:**
- [ ] `cargo it` passes on fresh containers.
- [ ] The specs match the code (Command enum, ItemState, keys).
- [ ] SMOKE lists the v1.1 checks.
**Verify:**
- [ ] `docker compose -f tests/docker/compose.yml up --build --wait && cargo it`
- [ ] `cargo llvm-cov --workspace --exclude filecargo-gui --summary-only` shows lines at or
  above v1.
**Dependencies:** T13, T14
**Files:** `crates/filecargo-transfer/tests/integration.rs`, `SPEC-*.md`, `crates/*/SMOKE.md`, `README.md`
**Scope:** M (docs are mostly text)

### Checkpoint C (complete)
- [ ] Every SPEC-v1.1 success criterion is met.
- [ ] Full suite plus `cargo it` are green, and coverage is no lower than v1.
- [ ] Human review, then the user pushes and CI runs (no push or tag without being asked).
