//! Snapshot tests of whole screens (100×30 and 80×24), plus checks of styling that text
//! snapshots cannot show.

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::{Color, Modifier};

use crate::test_support::{app, connected_app, draw, synced};
use crate::view;

#[test]
fn disconnected_at_100x30() {
    let app = app();
    let ui = synced(100, 30, &app);
    insta::assert_snapshot!(draw(&ui, &app));
}

#[test]
fn disconnected_at_80x24() {
    let app = app();
    let ui = synced(80, 24, &app);
    insta::assert_snapshot!(draw(&ui, &app));
}

#[test]
fn connected_with_both_panes_at_100x30() {
    let app = connected_app();
    let mut ui = synced(100, 30, &app);
    ui.local.cursor = 3;
    ui.local.selected = ["README.md".to_owned(), "notes.txt".to_owned()].into();
    insta::assert_snapshot!(draw(&ui, &app));
}

#[test]
fn too_small_shows_one_message() {
    let app = app();
    let ui = synced(79, 23, &app);
    insta::assert_snapshot!(draw(&ui, &app));
}

fn cell_style(
    ui: &crate::ui_state::UiState,
    app: &filecargo_app_core::prelude::AppState,
    x: u16,
    y: u16,
) -> ratatui::style::Style {
    let mut terminal = Terminal::new(TestBackend::new(ui.size.0, ui.size.1)).unwrap();
    terminal.draw(|frame| view::render(frame, ui, app)).unwrap();
    terminal.backend().buffer()[(x, y)].style()
}

#[test]
fn the_cursor_row_is_reversed_and_selected_rows_are_marked_and_tinted() {
    let app = connected_app();
    let mut ui = synced(100, 30, &app);
    ui.local.cursor = 3; // README.md
    ui.local.selected = ["notes.txt".to_owned()].into();
    // local pane starts at column 24 (tree) + border: name column at x = 24 + 2
    // rows: border 0, header 1, then `..` at y = 2, docs 3, src 4, README.md 5, notes.txt 6
    let cursor = cell_style(&ui, &app, 27, 5);
    assert!(
        cursor.add_modifier.contains(Modifier::REVERSED),
        "{cursor:?}"
    );
    let selected = cell_style(&ui, &app, 27, 6);
    assert_eq!(selected.fg, Some(Color::Yellow));
    assert!(selected.add_modifier.contains(Modifier::BOLD));
    let dir = cell_style(&ui, &app, 27, 3);
    assert_eq!(dir.fg, Some(Color::Blue));
}

#[test]
fn without_color_emphasis_comes_from_modifiers_only() {
    let app = connected_app();
    let mut ui = synced(100, 30, &app);
    ui.color = false;
    ui.local.cursor = 3;
    ui.local.selected = ["notes.txt".to_owned()].into();
    let plain = |fg| matches!(fg, None | Some(Color::Reset));
    assert!(
        plain(cell_style(&ui, &app, 27, 6).fg),
        "NO_COLOR: no foreground colors"
    );
    assert!(plain(cell_style(&ui, &app, 27, 3).fg));
    assert!(
        cell_style(&ui, &app, 27, 3)
            .add_modifier
            .contains(Modifier::BOLD),
        "directories stay bold"
    );
    assert!(
        cell_style(&ui, &app, 27, 5)
            .add_modifier
            .contains(Modifier::REVERSED)
    );
    let text = draw(&ui, &app);
    assert!(
        text.contains("* notes.txt"),
        "the selection marker still shows: {text}"
    );
}
