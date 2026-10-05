//! Terminal setup and the promise that it is undone: on normal exit, on error and on panic.

use std::io::{self, Write};

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
};
use ratatui::crossterm::execute;

/// Enters raw mode and the alternate screen (`ratatui::init`, which also installs a panic hook
/// that restores them), then enables the mouse and bracketed paste, which `init` does not.
pub fn enter() -> io::Result<DefaultTerminal> {
    let terminal = ratatui::init();
    execute!(io::stdout(), EnableMouseCapture, EnableBracketedPaste)?;
    // our panic hook runs first, then ratatui's (which restores raw mode and the screen)
    install_panic_restore(disable_extras);
    Ok(terminal)
}

/// Undoes everything [`enter`] did. Safe to call twice and when no terminal is attached.
pub fn leave() {
    disable_extras();
    ratatui::restore();
}

fn disable_extras() {
    let mut out = io::stdout();
    let _ = execute!(out, DisableBracketedPaste, DisableMouseCapture);
    let _ = out.flush();
}

/// Runs `restore` before the previous panic hook, so a panic anywhere (the render code
/// included) leaves the user's terminal usable and the message readable.
pub fn install_panic_restore(restore: impl Fn() + Send + Sync + 'static) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        previous(info);
    }));
}
