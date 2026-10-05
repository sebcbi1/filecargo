# Spec: terminal

> Module id: `terminal` · Crate: `crates/filecargo-terminal` · Depends on: `remote-fs` · Status: **draft, awaiting review**
> Project-wide rules: [SPEC.md](SPEC.md). Plan: [tasks/terminal/plan.md](tasks/terminal/plan.md).

## Objective

The interactive shell behind the **Terminal** tab of SFTP sessions. It:
- runs a VT emulator (`vt100`) fed by a `remote_fs::ShellChannel` on the session's own SSH connection
- turns UI-neutral key presses into the bytes a terminal would send
- exposes the screen so both UIs can draw it: the TUI with `tui-term`, the GUI with a custom element

Both UIs get identical behavior from this one crate: cursor-key modes, bracketed paste, resize
and scrollback.

## Public API

```rust
pub struct TermSize { pub cols: u16, pub rows: u16 }

/// Starts the emulator on an already-opened channel. Spawns one tokio task that feeds output
/// into the parser and forwards input; the task ends when the channel closes.
pub fn spawn(channel: ShellChannel, size: TermSize, scrollback: usize) -> TerminalHandle;

#[derive(Clone)]
pub struct TerminalHandle { /* Arc<Mutex<vt100::Parser>> + input sender + state */ }
impl TerminalHandle {
    /// Runs `f` with the current screen. Hold the lock only while drawing.
    pub fn with_screen<R>(&self, f: impl FnOnce(&vt100::Screen) -> R) -> R;
    pub fn send_key(&self, key: Key, mods: Mods);    // encoded with the screen's current modes
    pub fn send_text(&self, text: &str);              // typed text (UTF-8)
    pub fn paste(&self, text: &str);                  // bracketed when the app enabled it
    pub fn resize(&self, size: TermSize);             // updates the parser and sends window-change
    pub fn scroll(&self, lines: i32);                 // into scrollback; 0 = back to live view
    pub fn selection_text(&self, from: (u16, u16), to: (u16, u16)) -> String; // (row, col) ends, either order; copy
    pub fn status(&self) -> TermStatus;               // Running | Exited(Option<u32>) | Closed
    pub fn generation(&self) -> u64;                  // bumps when the screen changes
    pub fn title(&self) -> String;
    pub fn bell_count(&self) -> u64;                 // rings so far; a UI flashes when it changes
    pub fn close(&self);
}

/// UI-neutral key events. Each front-end maps its own key type to this.
pub enum Key {
    Char(char),                       // with modifiers below
    Enter, Tab, BackTab, Backspace, Escape, Delete, Insert,
    Up, Down, Left, Right, Home, End, PageUp, PageDown,
    F(u8),                            // F1..F12
}
pub struct Mods { pub ctrl: bool, pub alt: bool, pub shift: bool }
pub fn encode(key: Key, mods: Mods, modes: Modes) -> Vec<u8>; // pure; exposed for tests and the GUI
pub struct Modes { pub application_cursor: bool, pub application_keypad: bool }
```

`Screen` is re-exported as `vt100::Screen`. The UIs read `cell(row, col)`, `cursor_position()`,
`hide_cursor()` and colors directly. This avoids copying a snapshot of the grid every frame.
`filecargo-terminal` pins `vt100` to the version `tui-term` uses (0.16.2), so the TUI widget
accepts our `Screen` as is.

### Implementation notes (verified against vt100 0.16.2 / tui-term 0.3.4 sources)
- vt100 0.16 removed `Screen::title()` and the bell counters. The parser is created with
  `Parser::new_with_callbacks(rows, cols, scrollback, TermCallbacks)`, where `TermCallbacks`
  implements `vt100::Callbacks` (`set_window_title`, `audible_bell`, `visual_bell`) and stores
  the title and bell count for `title()` and the GUI's bell flash.
- Resizing is `screen_mut().set_size(rows, cols)`, and scrollback is
  `screen_mut().set_scrollback(n)`. The available scrollback depth is read by setting
  `usize::MAX` and then calling `scrollback()`.
- `Parser` is `Send + Sync` with `Send` callbacks, so `Arc<Mutex<Parser<TermCallbacks>>>` is fine.
- `contents_between(start_row, start_col, end_row, end_col)` backs `selection_text`.

## Behavior
- `TERM=xterm-256color`. Initial size comes from the UI (`TerminalOpen { cols, rows }`), and
  later resizes are coalesced to at most 10/s.
- Key encoding follows xterm:
  - **Arrows and Home/End:** `CSI A..D` / `CSI H, F`, or `SS3 A..D` / `SS3 H, F` in application cursor mode.
  - **Function keys:** `SS3 P..S` for F1–F4, `CSI 15~`… for F5–F12.
  - **Ctrl-letter:** C0 control bytes.
  - **Alt:** an ESC prefix.
  - **Modified arrows:** `CSI 1;<mod> A`.
- `paste` wraps text in `ESC[200~ … ESC[201~` when the remote app enabled bracketed paste.
  Otherwise newlines are sent as `\r`.
- Scrollback: 5,000 lines by default. Any key or input returns the view to live (`scroll(0)`).
- `generation()` lets UIs skip redraws: the TUI redraws only when it changed, and the GUI
  calls `cx.notify()` from the app-core snapshot tick.
- Channel `Exit(code)` → `status` becomes `Exited(code)`. The screen stays readable and input is
  ignored. The UI offers "reopen", which is a new `TerminalOpen`.

## Acceptance criteria
1. `encode` has a table-driven test covering every `Key` variant with no modifiers, Ctrl, Alt and Shift, in both cursor modes, against xterm's documented sequences.
2. A fake `ShellChannel` (in-memory mpsc): feeding `"\x1b[2J\x1b[1;1Hhello"` makes `with_screen(|s| s.contents())` start with `hello`, and `generation` increases.
3. `resize` sends `ShellInput::Resize` and the parser's size changes. Ten resizes in 50 ms send at most 2 window-change messages.
4. Bracketed paste: after `"\x1b[?2004h"` is fed, `paste("a\nb")` sends `ESC[200~a\nb ESC[201~`; without it, `"a\rb"`.
5. Scrollback: after 100 lines on a 24-row screen, `scroll(10)` shows earlier lines, and any `send_key` returns to live.
6. `Exit(Some(0))` → `status() == Exited(Some(0))`; later `send_key` calls are dropped without panicking.
7. Integration (`--features integration`, docker SFTP): open, run `printf 'ok\n'`, see `ok` on screen; resize to 100×30, then `stty size` prints `30 100`.

## Out of scope (v1)
Several terminals per session, local shells, mouse reporting to the remote app, sixel or
images, terminal search, persisting scrollback.
