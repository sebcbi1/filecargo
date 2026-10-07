# Manual smoke checklist: `filecargo` (GUI)

Run before a release on **macOS**, **Linux** (X11 and, if possible, Wayland) and **Windows**.
Use a fresh config directory so nothing depends on earlier runs:

```bash
docker compose -f tests/docker/compose.yml up -d --build --wait   # SFTP 2222, FTP 2121, FTPS 9990 / 2122 …
FILECARGO_CONFIG_DIR="$(mktemp -d)" cargo run -p filecargo-gui     # Linux build deps: see SPEC-gui.md / shell.nix
```

The test servers' logins are in `tests/docker/README.md` (SFTP `fcuser` / `fcpass`, FTP `ftpuser` /
`ftppass`; the FTPS servers use a self-signed certificate, which is what triggers the certificate
prompt). On Windows the release build needs `fxc.exe` from the Windows SDK (`GPUI_FXC_PATH`).

## Checklist

1. **Start**
   - [ ] The window opens: toolbar, server tree (with the "No servers yet" hint), two panes, bottom panel with five tabs. The system light / dark theme is followed (switch it while the app runs).
   - [ ] `secondary-q` (Ctrl-Q, ⌘Q on macOS) quits; the process ends.
2. **SFTP**
   - [ ] *New site*: SFTP, `127.0.0.1`, port `2222`, user `fcuser`, tick *Remember the password*, type `fcpass`, save. The site shows in the tree; the dialog's first field had the focus when it opened.
   - [ ] Double-click it: the *Unknown host key* dialog; *Trust once* connects, the remote pane lists the server, the toolbar says `● name (SFTP)`.
   - [ ] Right-click the site: Connect / Disconnect / Edit… / Duplicate / Rename… / Move to… / Delete… all work; `F2` and `Delete` work with the tree focused.
   - [ ] *Browse…* in the editor's key-file row opens the OS file picker and fills the field.
3. **FTPS**
   - [ ] A site on `127.0.0.1:9990` (FTPS implicit) or `2122` (explicit): the *Untrusted certificate* dialog shows subject, issuer, expiry and SHA-256; *Reject* fails the connection with a message, *Trust always* connects and is not asked again.
   - [ ] An FTP site without a remembered password asks for it in a masked dialog; a wrong one says "Login failed, try again."
4. **Transfers**
   - [ ] Select several local files (click, `shift`-click, `secondary`-click, `secondary-a`), `F5`: they run in the *Queue* tab with progress bars, speed and ETA, then appear in *Completed*; the remote pane refreshes by itself. The tab labels show the counts.
   - [ ] Upload a file that exists: the conflict dialog compares both; *Apply to all* works; Esc skips.
   - [ ] Download from the remote pane (`F5`, toolbar *Download*, double-click a file).
   - [ ] The Queue tab's *Pause* stops new transfers of this site only and says *Resume*; the row menu has Retry / Remove / Clear completed (Failed: Clear failed); `Delete` removes the row under the cursor.
   - [ ] With the window in the background, finish a batch: an OS notification appears (Linux needs a notification daemon).
5. **Remote operations**: `F7` new folder, `F2` rename, `Delete` (asks first), *Permissions…* (octal and the nine boxes follow each other), the path bar (`secondary-l`, type a path, Enter), double-click a folder and the `..` row, a header click sorts.
6. **Terminal (SFTP)**
   - [ ] `secondary-5` / the *Terminal* tab opens a shell at the size of the panel; typing, arrows, `Tab` completion and `Ctrl-C` work; `vim` and `htop` draw correctly with colours; the cursor shows.
   - [ ] Mouse selection copies with `secondary-c` (⌘C; Ctrl-Shift-C on Linux / Windows); `secondary-v` pastes (several lines are not run one by one at a bracketed-paste prompt); with nothing selected Ctrl-C is a plain interrupt.
   - [ ] Resize the window while `htop` runs: it follows. Wheel scrolls the history.
   - [ ] `Ctrl-Q` and the other global shortcuts reach the shell; `ctrl-shift-1..5` switches tabs from inside the terminal.
   - [ ] **Frame time at 200×60** (maximize on a big display, `yes | head -1000`, `htop`): the spec's target is under 4 ms per frame — note the observed feel / any profiler number here (not measurable headlessly).
7. **Notices and quit**: an info notice fades, an error stays until closed; quitting with a transfer running asks first (*Quit* / *Cancel*).
8. **Settings**: change *Simultaneous transfers* and *Show hidden files*, save; the panes react; bad numbers are refused inline.

9. **v1.1 checks**
   - [ ] The local pane's right-click menu has New folder, Rename, Delete (asks first) and Permissions (Unix); the toolbar's Folder / Rename / Delete act on the pane you last clicked.
   - [ ] Connected to A while B transfers in the background: the bottom panel shows A only with Local and Remote path columns, the toolbar says "N transfers on other sites"; disconnected, the lists are empty.
   - [ ] *Add to queue* (toolbar or right-click) holds files as "queued (held)"; *Start queue* starts them; *Pause* affects this site only; *Clear queue* asks first; the Failed tab menu has *Clear failed*.
   - [ ] Quit with held items, restart: they come back held until *Start queue*.

10. **Branding** (use the archives from a `workflow_dispatch` run of `release.yml`, not a `cargo run` build)
   - [ ] macOS: unpack the archive, move `FileCargo.app` to `/Applications` and allow it in *Privacy & Security*; the Dock, ⌘-Tab and Finder show the logo at the size of neighbouring icons, with no grey container.
   - [ ] macOS: the archive holds only `FileCargo.app`, README and licences; after the README's `ln -sf` step, `filecargo-tui --version` runs from a new terminal.
   - [ ] Windows: Explorer shows the logo on `filecargo.exe`; the title bar, taskbar and Alt-Tab show it when running.
   - [ ] Linux X11: the window and taskbar show the logo with nothing installed.
   - [ ] Linux Wayland: after the README's install steps (`~/.local/bin`, `~/.local/share`), the launcher entry and the running window show the logo.
   - [ ] The README shows the centered logo above the title on GitHub, in light and dark themes.

11. **About, menu bar and window lifecycle**
   - [ ] Windows / Linux: the ⓘ button at the right end of the toolbar (tooltip *About FileCargo*) opens a dialog with the logo, the app name, version, licence and a *Repository* button that opens the project page; *Close* dismisses it.
   - [ ] macOS: the menu bar shows *FileCargo* with *About FileCargo* (enabled with or without a window; opens the same dialog) and *Quit FileCargo* (⌘Q); the toolbar has no About button.
   - [ ] macOS: the red close button closes the window; the app stays in the Dock and a running transfer keeps going; clicking the Dock icon brings the window back with the same state.
   - [ ] macOS: ⌘Q with no window open and a transfer running reopens the window and asks "Quit anyway?"; without transfers it quits.
   - [ ] Windows / Linux: closing the window quits the app (unchanged).

12. **Import**
   - [ ] The toolbar has no Import button; *Settings* has a *Server list → Import from FileZilla…* button that opens the import dialog, and importing adds the sites to the tree.

## Automated coverage that stands in for part of this list

| What | Where |
|---|---|
| Every dialog, prompt kind, the site editor round trip, tree operations | `tests/dialogs.rs`, `tests/prompts.rs`, `tests/utility.rs` (headless, real dialogs) |
| Panes, multi-select, navigation, sorting | `tests/panes.rs` |
| Tree, connect / disconnect, toolbar | `tests/tree.rs` |
| Transfers, queue, per-site pause, log, tabs | `tests/transfers.rs` |
| Local ops, scoped panel, staged queue (add / start / pause / clear) | `tests/local_ops.rs`, `tests/scope.rs`, `tests/staged_queue.rs` |
| Shortcuts (F2 / F5 / F7 / Delete, `secondary-k/l/1..5`) | `tests/shortcuts.rs` |
| Terminal: keys → bytes, global shortcuts not stealing keys, resize, wide characters / colours, reopen | `tests/terminal.rs`, `src/terminal/{keys,runs}.rs` |
| Window opens, snapshots re-render, theme switch | `tests/window.rs` |

## Looking at the real window without a display (Linux, development aid)

The built binary can run under a virtual X server with a software Vulkan driver, and the screen can
be captured, e.g. on NixOS:

```bash
nix-shell -p xvfb-run mesa imagemagick xdotool …   # plus the libraries of shell.nix
VK_ICD_FILENAMES=<mesa>/share/vulkan/icd.d/lvp_icd.x86_64.json \
  xvfb-run -a -s '-screen 0 1400x900x24' bash -c 'target/debug/filecargo & sleep 12; import -window root shot.png'
```

`xdotool` clicks work (move the pointer first, wait, then click); text typed with `xdotool type` does
not reach inputs under Xvfb (no keyboard layout), key *names* such as `ctrl+a` do.

## Record

| Date | Environment | Items | Result | By |
|---|---|---|---|---|
| 2026-10-06 | NixOS, Xvfb + llvmpipe (lavapipe) Vulkan, 1400×900 | 1 (window opens, empty-tree hint, toolbar, panes, tabs), import dialog → app error → message dialog | ✅ (screenshots reviewed) | automated session |
| 2026-10-06 | headless gpui test platform | everything in the table above | ✅ | automated |
| | macOS | all | ⏳ pending | |
| | Linux (real display, X11 / Wayland) | all | ⏳ pending | |
| | Windows | all | ⏳ pending | |
