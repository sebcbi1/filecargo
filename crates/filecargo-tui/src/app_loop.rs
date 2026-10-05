//! The loop: input, new snapshots and a tick, redrawing only when something changed.

use std::io;
use std::time::Duration;

use filecargo_app_core::prelude::*;
use futures_util::StreamExt;
use ratatui::DefaultTerminal;
use ratatui::Frame;
use ratatui::crossterm::event::{Event as CrosstermEvent, EventStream};

use crate::reducer::{self, Event};
use crate::ui_state::UiState;

const TICK: Duration = Duration::from_millis(250);

/// What draws a frame. A parameter so tests can make it panic.
pub type Renderer = dyn FnMut(&mut Frame, &UiState, &AppState);

fn to_event(event: CrosstermEvent) -> Option<Event> {
    match event {
        CrosstermEvent::Key(key) if key.is_press() || key.is_repeat() => Some(Event::Key(key)),
        CrosstermEvent::Mouse(mouse) => Some(Event::Mouse(mouse)),
        CrosstermEvent::Resize(w, h) => Some(Event::Resize(w, h)),
        CrosstermEvent::Paste(text) => Some(Event::Paste(text)),
        _ => None,
    }
}

/// Runs until the user quits (the app ends and the snapshot channel closes) or the terminal
/// fails.
pub async fn run(
    app: &AppHandle,
    terminal: &mut DefaultTerminal,
    render: &mut Renderer,
) -> io::Result<()> {
    let mut snapshots = app.state();
    let mut input = EventStream::new();
    let mut tick = tokio::time::interval(TICK);
    let size = terminal.size()?;
    let mut ui = UiState::new(size.width, size.height);
    ui.log = app.log();
    let mut dirty = true;

    loop {
        if dirty {
            let state = snapshots.borrow().clone();
            reducer::sync(&mut ui, &state);
            terminal.draw(|frame| render(frame, &ui, &state))?;
            dirty = false;
        }
        tokio::select! {
            event = input.next() => {
                let Some(event) = event else { return Ok(()) };
                let Some(event) = to_event(event?) else { continue };
                let state = snapshots.borrow().clone();
                for command in reducer::on_event(&mut ui, &state, event) {
                    app.send(command);
                }
                dirty = true;
            }
            changed = snapshots.changed() => {
                if changed.is_err() {
                    return Ok(()); // the app has quit
                }
                dirty = true;
            }
            _ = tick.tick() => {
                // progress and spinners move while transfers or connections are active
                let state = snapshots.borrow();
                dirty |= !state.queue.pending.is_empty()
                    || matches!(state.session, SessionState::Connecting { .. });
            }
        }
    }
}
