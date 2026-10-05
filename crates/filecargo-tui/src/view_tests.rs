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

fn with_servers() -> filecargo_app_core::prelude::AppState {
    let mut app = app();
    app.servers = std::sync::Arc::new(crate::test_support::sample_tree());
    app
}

fn session_info() -> filecargo_app_core::prelude::SessionInfo {
    filecargo_app_core::prelude::SessionInfo {
        protocol: filecargo_app_core::prelude::Protocol::Sftp,
        banner: None,
        tls: None,
        home: filecargo_app_core::prelude::RemotePath::root(),
    }
}

#[test]
fn disconnected_with_a_server_tree_at_100x30() {
    let app = with_servers();
    let mut ui = synced(120, 30, &app);
    ui.focus = crate::ui_state::Focus::Tree;
    ui.tree.expanded = app
        .servers
        .folders()
        .iter()
        .filter(|f| f.name == "Work")
        .map(|f| f.id)
        .collect();
    ui.tree.cursor = 3;
    insta::assert_snapshot!(draw(&ui, &app));
}

#[test]
fn connecting_shows_the_step_in_the_remote_title() {
    use filecargo_app_core::prelude::{ConnectStep, SessionState};
    let mut app = with_servers();
    let id = crate::test_support::site_id(&app.servers, "prod-web");
    app.session = SessionState::Connecting {
        site: id,
        step: ConnectStep::Authenticating,
    };
    let mut ui = synced(120, 30, &app);
    ui.tree.expanded = app.servers.folders().iter().map(|f| f.id).collect();
    insta::assert_snapshot!(draw(&ui, &app));
}

#[test]
fn connected_listing_shows_the_url_and_marks_the_connected_site() {
    use filecargo_app_core::prelude::SessionState;
    let mut app = connected_app();
    app.servers = std::sync::Arc::new(crate::test_support::sample_tree());
    let id = crate::test_support::site_id(&app.servers, "prod-web");
    app.session = SessionState::Connected {
        site: id,
        info: session_info(),
    };
    let mut ui = synced(120, 30, &app);
    ui.tree.expanded = app.servers.folders().iter().map(|f| f.id).collect();
    ui.focus = crate::ui_state::Focus::Remote;
    ui.remote.cursor = 2;
    insta::assert_snapshot!(draw(&ui, &app));
}

#[test]
fn a_failed_connection_shows_the_error_in_the_title() {
    use filecargo_app_core::prelude::SessionState;
    let mut app = with_servers();
    let id = crate::test_support::site_id(&app.servers, "staging");
    app.session = SessionState::Failed {
        site: id,
        error: "connection refused".to_owned(),
    };
    let ui = synced(100, 30, &app);
    insta::assert_snapshot!(draw(&ui, &app));
}
