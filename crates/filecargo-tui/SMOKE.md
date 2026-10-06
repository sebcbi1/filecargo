# Manual smoke checklist: `filecargo-tui`

Run before a release, on each of: **macOS** (Terminal.app or iTerm2), a **Linux** terminal, and
**Windows Terminal**. Use a fresh config directory so nothing depends on earlier runs:

```bash
docker compose -f tests/docker/compose.yml up -d --build --wait   # test servers (SFTP 2222, FTP 2121)
cargo run -p filecargo-tui -- --config-dir "$(mktemp -d)"
```

Credentials of the test servers are in `tests/docker/README.md` (SFTP `fcuser` / `fcpass`,
FTP `ftpuser` / `ftppass`).

## Checklist

For each item, note ✅ / ❌ and the terminal in the table below.

1. **Start and quit**
   - [ ] The screen draws (three panes, bottom panel, status line); `q` quits and the shell prompt
     is back with the cursor visible and no mouse / paste garbage on the next keypress.
   - [ ] `filecargo-tui --version` and `--help` print and exit.
2. **SFTP**
   - [ ] `n` opens the site editor; create `docker-sftp` (SFTP, `127.0.0.1`, port `2222`, user
     `fcuser`, tick *Remember secret*, type `fcpass`). The password shows as bullets.
   - [ ] `Enter` on the site: the *Unknown host key* prompt appears; `Enter` does nothing, `y`
     trusts it; the remote pane shows `sftp://127.0.0.1/…` and takes the focus.
   - [ ] Reconnect (`x`, then `Enter`): no host-key prompt the second time (trusted for the rest of the run).
3. **FTP**
   - [ ] Create `docker-ftp` (FTP, `127.0.0.1`, port `2121`, user `ftpuser`, *do not* remember the
     password). Connecting asks for the password in a masked prompt; a wrong one shows
     "Login failed, try again."; `Esc` cancels cleanly.
   - [ ] The terminal tab (`Alt-5`) says it needs an SFTP connection.
4. **Auth prompt**
   - [ ] Edit the SFTP site (`e`), untick *Remember secret*, reconnect: the password prompt shows
     and *Remember* in it works.
5. **Upload and download**
   - [ ] Select two local files (`Space`), `F5`: both appear in the queue with a progress bar,
     speed and ETA, then in *Completed*; the remote pane refreshes by itself.
   - [ ] Upload a file that exists remotely: the conflict prompt shows both sizes and times;
     `a` toggles *apply to all*; `o` overwrites; `Esc` skips.
   - [ ] Focus the remote pane, `F5` on a file: it lands in the local folder.
   - [ ] `F7` new folder, `F2` rename, `c` chmod (octal and the boxes follow each other), `F8`
     delete asks first; `g` go to path.
6. **Terminal tab (SFTP only)**
   - [ ] `Alt-5`: a shell opens at the size of the panel. Type `ls`, arrows and `Tab` completion work.
   - [ ] Run `htop` (or `top`) and `vim`: they draw correctly, `q` / `:q` exit them, colours show.
   - [ ] `Ctrl-\` (or `F12`) leaves the terminal without sending anything; `Alt-5` comes back to the
     same shell.
   - [ ] `Shift-PgUp` / `Shift-PgDn` scroll the history; typing returns to the live screen.
   - [ ] Paste a few lines (terminal paste shortcut): they are not executed one by one when the
     shell is at a bracketed-paste prompt.
7. **Resizing**
   - [ ] Shrink the window to 80×24: still usable. Below that: "terminal too small (needs 80x24)",
     and growing it again restores the screen.
   - [ ] Below 100 columns the server tree hides; `F9` brings it back.
   - [ ] With the shell open, resize the window and run `stty size` or `htop`: it follows.
8. **Mouse**
   - [ ] The wheel scrolls the pane / list under the pointer; a click focuses an area and selects
     the row; a click on a bottom-panel tab title switches the tab.
   - [ ] (Shift-drag selects text in most terminals while mouse capture is on.)
9. **Colour**
   - [ ] `NO_COLOR=1 cargo run -p filecargo-tui …`: no colours anywhere; the cursor row, the
     selection and the focused area are still distinguishable.
10. **Terminal restore**
    - [ ] Kill the process with `kill -TERM` while it runs: the terminal may stay in the alternate
      screen (signals are not handled in v1); `reset` fixes it. A *panic* must restore it (covered
      by `tests/panic.rs`).

11. **v1.1 checks**
    - [ ] `Shift-Tab` cycles the focus backwards in kitty, xterm and tmux; in the terminal tab it still reaches the shell.
    - [ ] On the local pane: `F7` mkdir, `F2` rename, `Del` (asks first), `c` chmod (Unix) change the disk and the listing.
    - [ ] Connected to site A while site B transfers in the background: the Queue / Completed / Failed / Log tabs show A only, rows show both paths, the status bar says "N transfers on other sites"; disconnected, the lists are empty.
    - [ ] `a` on selected files queues them as "queued (held)" and nothing transfers; `S` in the Queue tab starts them; `p` pauses only this site (the tab header says "paused"); `X` asks, then clears this site's queue; `C` in the Failed tab clears failed items.
    - [ ] Quit with held items, restart: they come back held and nothing starts until `S`.

## Automated coverage that stands in for part of this list

| What | Where |
|---|---|
| Items 2, 5 (upload, rename, delete, download), 6 (shell) against the Docker SFTP server, through the real reducer / view / app core | `tests/e2e_sftp.rs` (`cargo it`) |
| Every prompt kind, dialogs, 80×24 and 100×30 screens | `src/view_tests.rs` snapshots |
| Key handling for every binding | `src/reducer.rs` tests, `keymap` table tests |
| Panic restores the terminal | `tests/panic.rs` |
| The real binary starts, draws, opens the help and quits on a pseudo-terminal (Linux) | run by hand during development, see the record below |

## Record

| Date | Terminal | OS | Items | Result | By |
|---|---|---|---|---|---|
| 2026-10-05 | `script` pseudo-terminal 100×30 | Linux | 1 (start, help overlay, quit, exit code 0) | ✅ | automated run during the build |
| 2026-10-05 | in-process driver (`tests/e2e_sftp.rs`) | Linux | 2, 5, 6 against the Docker servers | ✅ | automated |
| | macOS Terminal / iTerm2 | | all | ⏳ pending | |
| | Linux terminal (real) | | all | ⏳ pending | |
| | Windows Terminal | | all | ⏳ pending | |
