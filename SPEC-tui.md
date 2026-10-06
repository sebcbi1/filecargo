# Spec: tui

> Module id: `tui` · Crate: `crates/filecargo-tui` → bin **`filecargo-tui`** · Depends on: `app-core` only · Status: **implemented, awaiting final review**
> Project-wide rules: [SPEC.md](SPEC.md). Plan: [tasks/tui/plan.md](tasks/tui/plan.md).

## Objective

A keyboard-first terminal front-end that exposes everything `app-core` offers. It must be usable
over SSH on an 80×24 terminal, and pleasant on a large one. It holds no business logic: every
action is an `app_core::Command`, and every visible fact comes from `AppState`.

## Layout

```
┌ Servers ─────┬ Local ~/projects ──────────────┬ Remote sftp://prod-web/var/www ─┐
│ ▾ Work       │   Name         Size  Modified  │   Name        Size  Modified    │
│   ● prod-web │ ▸ src/               10-01 14:02│ ▸ html/             09-30 11:15 │
│   staging    │ * README.md    2.1K  10-01 13:40│   index.php   4.3K  09-30 11:20 │
│ ▸ Personal   │                                │                                 │
├──────────────┴────────────────────────────────┴─────────────────────────────────┤
│ Queue 3 │ Completed │ Failed 1 │ Log │ Terminal                                  │
│ ↑ README.md → /var/www/README.md   ███████░░░  42%  1.2 MB/s  00:03             │
└─ F1 help  Tab focus  F5 transfer  F7 mkdir  F8 delete  q quit ──────────────────┘
```
- Column widths: server tree 20% (min 18 cols, hidden below 100 cols until toggled with `F9`).
  Panes split the rest evenly. The bottom panel is 30% of the height (min 6 rows) and can be
  maximized with `F10`.
- Below 80×24: a single centered message, "terminal too small (needs 80×24)".
- Status line: key hints for the focused area. Connection state appears in the remote pane title.
- Colors from the terminal palette only (16 colors), honoring `NO_COLOR`.

## Key bindings

| Context | Keys |
|---|---|
| Global | `Tab` / `Shift-Tab` focus next/prev area (`BackTab` matches with or without `SHIFT`) · `F1` / `?` help overlay · `Ctrl-r` refresh focused pane · `Alt-1..5` bottom tabs · `F9` toggle server tree · `F10` maximize bottom panel · `q` / `Ctrl-q` quit |
| Server tree | `↑↓` move · `→` / `←` expand/collapse · `Enter` connect (site) or toggle (folder) · `n` new site · `N` new folder · `e` edit · `r` rename · `D` duplicate · `m` move to folder · `Del` delete · `i` import FileZilla · `x` disconnect |
| File panes | `↑↓ PgUp PgDn Home End` move · `Enter` open dir (or transfer a file to the other side) · `Backspace` parent · `Space` / `Ins` toggle select · `*` invert · `Ctrl-a` select all · `F5` / `t` transfer selection (or cursor) to the other side · `a` add to the queue without starting · `F7` mkdir · `F2` rename · `F8` / `Del` delete · `c` chmod (F7/F2/F8/`c` act on the focused pane; local chmod not on Windows) · `.` toggle hidden · `s` cycle sort · `g` go to path |
| Queue / Completed / Failed | `↑↓` move · `r` retry / `R` retry all (Failed) · `Del` remove · `C` clear completed (Failed tab: clear failed) · `p` pause/resume this site (tab header shows "paused") · `S` start the held items (Queue tab) · `X` clear this site's queue after a confirm (Queue tab) |
| Log | `↑↓ PgUp PgDn` scroll · `End` follow |
| Terminal | All keys go to the remote shell. **`Ctrl-\`** or **`F12`** leaves terminal focus. `Shift-PgUp` / `Shift-PgDn` scroll back. |
| Dialogs | `Enter` confirm · `Esc` cancel · `Tab` next field · access keys shown underlined |

`Enter` on a file opens nothing locally: there's no editing in v1. It queues the file for the
other side, like FileZilla's double-click.

## Dialogs
- **Site editor** (new/edit): name, protocol, host, port, user, auth method, key path, password
  (masked) with a remember checkbox, FTP mode, remote dir, local dir, notes. Validation errors
  come from `app-core`'s `Message` prompt and are shown inline.
- **Prompts** render every `PromptKind`:
  - credential: masked input with a remember checkbox
  - host key: algorithm, fingerprint, and Trust once / Trust always / Reject
  - certificate: subject, issuer, expiry, fingerprint, problem
  - conflict: source vs target size and time, the five rules, and "apply to all"
  - confirm, and messages
- **chmod**: octal input plus rwx checkboxes; **go to path**; **import FileZilla**: path
  prefilled with `default_filezilla_path()`, plus an import-passwords checkbox.

## Architecture
- `main`: parse args (`--version`, `--config-dir <path>` → `FILECARGO_CONFIG_DIR`), then
  `App::start`, set up the terminal (`ratatui::init`, panic-safe restore), and run the loop.
- **Loop**: `tokio::select!` over crossterm `EventStream`, `state.changed()` and a 4 Hz tick
  (progress and spinners). It redraws only when something changed.
- **`UiState`** (TUI-only): focus, cursor and selection per pane (a set of names, reset when
  `Pane.generation` changes), scroll offsets, tree expansion, open dialog and form contents,
  help visibility.
- **Reducer**: `fn on_event(&mut UiState, &AppState, Event) -> Vec<Command>` is pure: no I/O and
  no terminal access. All key handling is unit-tested through it.
- **View**: `fn render(&UiState, &AppState, &mut Frame)` is pure and snapshot-tested with
  `TestBackend`.
- Mouse (v1): wheel scrolls the area under the cursor; a click focuses an area and moves the
  cursor to the clicked row. No drag and drop.

### Implementation notes (verified against ratatui 0.30.2 / crossterm 0.29 / tui-tree-widget 0.24.1)
- Mouse capture and bracketed paste:
  - `ratatui::init()` sets raw mode, the alternate screen and a panic hook, but enables
    neither.
  - The app enables both itself after `init`, and disables them in its own restore path,
    which the panic hook chains to.
- **Input:** crossterm feature `event-stream` (`EventStream` is runtime-agnostic). On Windows
  keep only `Event::is_key_press()`, since releases are reported too.
- **Tables:** ratatui `Table` selection is single-row, so multi-select is drawn by styling
  `Row`s from `UiState`. Use `row_highlight_style`, not the deprecated `highlight_style`.
- **Tree:** `tui-tree-widget` items borrow their text and are rebuilt each frame from
  `ServerTree`. Node ids are `Vec<String>` paths of folder and site ids.
  `TreeItem::new` errors on duplicate sibling ids, which config already prevents.
- **Snapshots:** `TestBackend` implements `Display`, so `insta::assert_snapshot!(terminal.backend())`
  works.

## Acceptance criteria
1. Reducer tests: every binding in the table above produces the expected `Command`s / `UiState` change (table-driven, ≥ 1 case per row).
2. Snapshot tests (`insta`, 100×30 and 80×24) for these screens:
   - disconnected with a tree
   - connecting
   - connected listing
   - transfer in progress
   - each prompt kind
   - site editor
   - help overlay
   - terminal tab
   - too-small terminal
3. Selection survives refreshes when the names still exist and resets on navigation (`generation` change).
4. Terminal focus: typed keys reach `TerminalInput` with the right bytes (via `terminal::encode`), and `Ctrl-\` leaves focus without sending anything.
5. Panics restore the terminal (cooked mode, cursor shown, alternate screen left), checked with a test that triggers a panic inside the loop's render hook.
6. Manual smoke checklist (macOS Terminal/iTerm2, a Linux terminal, Windows Terminal) in `crates/filecargo-tui/SMOKE.md`, covering:
   - connecting via SFTP and FTP
   - an upload and download
   - an auth prompt
   - htop/vim in the terminal tab
   - resizing the window

## Out of scope (v1)
Themes or config of key bindings, mouse drag and drop, split or extra tabs, inline file preview,
image rendering.

## v1.1 changes
- Shift-Tab moves focus backwards. File-op keys act on the focused (local or remote) pane.
- The Queue, Completed, Failed and Log tabs show the connected site only (empty when not
  connected); rows show direction, local path, remote path (long paths cut from the left), size
  and progress. The status bar says "N transfers on other sites" while other sites transfer.
- Held items show as "queued (held)". `p` toggles the site's pause (the queue-wide toggle is gone).
