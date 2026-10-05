# Spec: gui

> Module id: `gui` · Crate: `crates/filecargo-gui` → bin **`filecargo`** · Depends on: `app-core` only · Status: **draft, awaiting review**
> Project-wide rules: [SPEC.md](SPEC.md). Plan: [tasks/gui/plan.md](tasks/gui/plan.md).

## Objective

A native desktop front-end (macOS, Linux, Windows) with FileZilla's layout and mouse-driven
workflow, built on **gpui-kit 0.7.1** (GPUI via `gpui-pre =0.3.8`). It has the same
capabilities as the TUI, because both drive the same `app-core` API. The GUI adds
mouse-first interaction, resizable panes, context menus, notifications and a light/dark theme
that follows the system.

## Layout

```
┌──────────────────────────────────────────────────────────────────────────────┐
│ [⟳] [↑ Upload] [↓ Download] [✚ Folder] [✎ Rename] [🗑 Delete]   ● prod-web (SFTP) │  toolbar
├───────────────┬───────────────────────────────┬──────────────────────────────┤
│ SERVERS       │ ~/projects ▸ (path input)     │ /var/www ▸ (path input)      │
│ ▾ Work        │ Name ▲        Size  Modified  │ Name ▲      Size  Modified Perm│
│   ● prod-web  │ 📁 src             10-01 14:02│ 📁 html          09-30   drwx…│
│   staging     │ 📄 README.md  2.1K 10-01 13:40│ 📄 index.php 4.3K 09-30 -rw-…│
│ ▸ Personal    │                               │                              │
├───────────────┴───────────────────────────────┴──────────────────────────────┤
│ Queue (3) │ Completed │ Failed (1) │ Log │ Terminal                          │  TabBar
│ ↑ README.md → /var/www/README.md   ▓▓▓▓▓░░░ 42%  1.2 MB/s  0:03   [✕]        │
└──────────────────────────────────────────────────────────────────────────────┘
```
- `h_resizable` columns: tree (default 220 px, 160–480), local and remote panes.
  `v_resizable`: body and bottom panel (default 30% of height).
- **v1 doesn't persist** splitter sizes or window size.
- Theme follows the system light/dark appearance (`Theme::sync_system_appearance`).

## Interaction
| Area | Mouse | Keyboard (`secondary` = ⌘ on macOS, Ctrl elsewhere) |
|---|---|---|
| Server tree | click selects · double-click connects (site) / toggles (folder) · right-click: Connect, Edit, Rename, Duplicate, Move to…, Delete, New site/folder, Import FileZilla… | arrows, Enter, F2 rename, Del delete |
| File panes | click / secondary-click / shift-click select (multi) · double-click opens a dir or transfers a file to the other side · header click sorts · right-click: Upload/Download, Open, Rename, Delete, New folder, Permissions…, Refresh | arrows, Enter, Backspace parent, `secondary-a` all, F5 transfer, F7 mkdir, F2 rename, Del delete, `secondary-r` refresh, `secondary-l` focus path input |
| Path input | type a path + Enter | |
| Queue / Failed | right-click: Retry, Remove, Clear completed · toolbar toggle: pause processing | Del remove |
| Terminal | click focuses · selection with the mouse copies on `secondary-c` · `secondary-v` pastes | every key goes to the shell except `secondary-c/v`, which are copy/paste only when text is selected or the clipboard is non-empty |
| Global | | `secondary-q` quit · `secondary-,` settings · `secondary-1..5` bottom tabs · `secondary-k` connect to the selected site |

## Dialogs and notifications
- **Site editor**:
  - Fields: name, protocol (`Select`), host, port, user, auth method (`Select`), key path
    (with a file picker), password (`Input::masked` + remember `Checkbox`), FTP mode,
    remote dir, local dir, notes.
  - Saving sends `Command::Tree(AddSite | UpdateSite)` and, if a password was entered,
    `SetSitePassword`.
  - Validation errors arrive as `Message` prompts and show inline.
- **Prompt dialogs** for every `PromptKind`:
  - Credential, host key and certificate details, each with Trust once / Trust always / Reject.
  - Conflict: a source/target comparison, the five rules and "apply to all".
  - Confirm delete, confirm quit, and messages (the import report is shown as a list).
- **Settings dialog**: transfers (max concurrent, default conflict rule), connection
  (timeout, keepalive), UI (show hidden, confirm delete, local start dir), log level. Saving
  sends `UpdateSettings`.
- **Permissions dialog**: octal input ↔ rwx checkbox grid.
- `AppState.notices` are shown as gpui-kit notifications (autohide for info, sticky for errors).
  Transfer completion of a whole batch posts an OS notification when the window is unfocused.

## Architecture
- `main`:
  1. `App::start` (app-core owns its tokio runtime on its own threads)
  2. `gpui_kit::application().with_assets(gpui_kit::assets::Assets).run(..)`
  3. `gpui_kit::init(cx)`, theme sync, key bindings
  4. `gpui_kit::open_window` with the `Workspace` view; it wraps the view in `Root`, which
     dialogs and notifications need
- **`AppModel` entity** holds the latest `Arc<AppState>` and the `AppHandle`. A foreground task
  loops on `state.changed().await` (tokio's sync primitives are runtime-agnostic, verified),
  stores the snapshot and calls `cx.notify()`. Views read from `AppModel`. **No tokio inside gpui
  tasks**: all I/O happens in app-core.
- **Views** (entities): `Workspace` (layout, toolbar, actions), `ServerTreeView` (gpui-kit `Tree`),
  `FilePaneView` ×2 (gpui-kit `DataTable`), `BottomPanel` (`TabBar` + panels), `QueueView`
  (`DataTable`), `LogView` (`uniform_list`), `TerminalView` (custom element), dialog builders.
- **File panes**:
  - `DataTable` with a `TableDelegate` per pane: sortable columns, resizable columns,
    virtualization, row context menu, `DoubleClickedRow`.
  - **Multi-select lives in the delegate** as a `BTreeSet<String>` of names. It is drawn in
    `render_tr` and updated from mouse-down with the secondary/shift modifiers. This is the
    workaround the research verified for the single-select table.
  - Selection resets when the pane's `generation` changes, unless the names still exist.
- **Server tree**:
  - `Tree` items are rebuilt from `ServerTree` when its `Arc` pointer changes.
  - Expanded folder ids are kept in the view and re-applied after `set_items`, which resets
    selection and expansion.
  - Selection is tracked with `ListItem::on_click`, since the tree emits no selection event.
- **Terminal element**:
  - A `canvas` paints the cell backgrounds with `paint_quad`.
  - Each row is one `shape_line` with `force_width = cell width`, giving an exact monospace
    grid, and a new `TextRun` wherever the cell attributes change.
  - The cursor is painted as a quad.
  - Key events are mapped to `terminal::Key` + `Mods`, and the result of `terminal::encode`
    is sent as `TerminalInput`. Clipboard via `cx.write_to_clipboard` / `read_from_clipboard`.
  - The font is the platform monospace (Menlo / DejaVu Sans Mono / Consolas), 13 px.
- **Actions**: `gpui_kit::actions!(filecargo, [...])` with `cx.bind_keys`, scoped by
  `key_context` (`Workspace`, `FilePane`, `ServerTree`, `Terminal`).

## Build and platforms
- `cargo build -p filecargo-gui` (not a default member; plain `cargo build` skips it).
- **CI `gui` job** (matrix ubuntu/macos/windows): `cargo build -p filecargo-gui` +
  `cargo test -p filecargo-gui`.
  - Ubuntu installs `libfontconfig-dev libfreetype-dev libwayland-dev libxkbcommon-x11-dev
    libx11-xcb-dev libvulkan1 mesa-vulkan-drivers`. Extend the list from gpui-kit's own CI if
    linking fails.
  - Windows release builds need `fxc.exe` from the Windows SDK, which is present on GitHub
    runners.
- Packaging (app bundle, installers, code signing) is **out of v1**: the deliverable is the binary.

## Acceptance criteria
1. `#[gpui_kit::test]` tests (headless `TestAppContext` + `TestWindowExt`), all with the RootedFs `SessionFactory`:
   1. The window opens.
   2. A new `AppState` snapshot re-renders the panes.
   3. Double-clicking a site sends `Connect`.
   4. Double-clicking a dir sends `Navigate`.
   5. Secondary-click and shift-click build a multi-selection.
   6. Pressing F5 sends `Upload`/`Download` with the selected names.
   7. A `Credential` prompt opens a dialog, and submitting sends `Answer`.
2. Every `PromptKind` renders a dialog whose buttons send the matching `PromptAnswer` (one test per kind).
3. Terminal element:
   - A unit test on the cell → `TextRun` conversion: attribute boundaries, colors and wide chars.
   - A headless render of a fake screen doesn't panic.
   - Keys typed into the focused terminal reach `TerminalInput` with `terminal::encode` bytes.
4. The site editor round-trip: create → appears in the tree → edit → values persisted through `Command::Tree`.
5. Theme: switching the system appearance (or `Theme::change`) re-renders without restart.
6. The CI `gui` job builds and tests on all three OSes.
7. Manual smoke checklist (`crates/filecargo-gui/SMOKE.md`), run on macOS, Linux (X11 or Wayland) and Windows:
   - connect over SFTP and FTPS (with cert prompt)
   - multi-select upload/download, conflict dialog
   - delete and chmod
   - terminal with vim/htop, copy/paste, window resize
   - quit with active transfers

## Out of scope (v1)
Drag and drop between panes or from the OS file manager (stretch goal after v1, on gpui
`on_drag` / `on_drop`), persisting layout or window size, custom themes, app bundles/installers,
code signing and auto-update, multiple windows or session tabs, file previews.
