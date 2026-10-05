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

fn dialog_ui(
    app: &filecargo_app_core::prelude::AppState,
    dialog: crate::dialog::Dialog,
) -> crate::ui_state::UiState {
    let mut ui = synced(100, 30, app);
    ui.dialog = Some(dialog);
    ui
}

#[test]
fn the_site_editor_shows_only_the_fields_of_the_chosen_login() {
    use crate::dialog::{Dialog, SiteEditor};
    let app = with_servers();
    let ui = dialog_ui(&app, Dialog::Site(Box::new(SiteEditor::new_site(None))));
    insta::assert_snapshot!(draw(&ui, &app));
}

#[test]
fn the_site_editor_shows_validation_errors_and_hides_a_typed_password() {
    use crate::dialog::{Dialog, SiteEditor};
    let app = with_servers();
    let mut editor = SiteEditor::new_site(None);
    editor.form.field_mut("name").unwrap().set_text("work");
    editor
        .form
        .field_mut("password")
        .unwrap()
        .set_text("hunter2");
    editor.form.focus = 6;
    editor.form.error = Some("Enter a host.".to_owned());
    let ui = dialog_ui(&app, Dialog::Site(Box::new(editor)));
    let screen = draw(&ui, &app);
    assert!(!screen.contains("hunter2"));
    insta::assert_snapshot!(screen);
}

#[test]
fn the_delete_confirmation_and_the_move_picker_are_centered_boxes() {
    use crate::dialog::{ConfirmDialog, Dialog, MovePicker};
    use filecargo_app_core::prelude::{NodeId, TreeOp};
    let app = with_servers();
    let id = crate::test_support::site_id(&app.servers, "staging");
    let confirm = Dialog::Confirm(ConfirmDialog {
        title: "Delete".to_owned(),
        body: "Delete the site \"staging\"?".to_owned(),
        op: TreeOp::Delete {
            node: NodeId::Site(id),
        },
    });
    insta::assert_snapshot!("confirm", draw(&dialog_ui(&app, confirm), &app));
    let picker = Dialog::Move(MovePicker::new(&app.servers, NodeId::Site(id)));
    insta::assert_snapshot!("move_picker", draw(&dialog_ui(&app, picker), &app));
}

#[test]
fn an_input_dialog_shows_the_cursor_in_the_text() {
    use crate::dialog::{Dialog, InputDialog, InputPurpose};
    let app = connected_app();
    let ui = dialog_ui(
        &app,
        Dialog::Input(InputDialog::new(
            "Rename",
            "New name",
            "index.php",
            InputPurpose::Mkdir,
        )),
    );
    insta::assert_snapshot!(draw(&ui, &app));
}

fn prompt_screen(kind: filecargo_app_core::prelude::PromptKind) -> String {
    use filecargo_app_core::prelude::{Prompt, PromptId};
    let mut app = connected_app();
    app.prompt = Some(Prompt {
        id: PromptId(1),
        kind,
    });
    let ui = synced(100, 30, &app);
    draw(&ui, &app)
}

#[test]
fn prompts_of_every_kind_are_drawn() {
    use filecargo_app_core::prelude::*;
    let host_key = PromptKind::HostKey(HostKeyPrompt {
        host: "example.org".into(),
        port: 22,
        algorithm: "ssh-ed25519".into(),
        fingerprint: "SHA256:uV3ZkP0xvQhmY1d1fGkTt6l4mqWb8rJwq0h5cG9nZ2A".into(),
    });
    insta::assert_snapshot!("prompt_host_key", prompt_screen(host_key));
    let cert = PromptKind::Certificate(CertificatePrompt {
        host: "ftp.example.org".into(),
        port: 21,
        problem: CertificateProblem::SelfSigned,
        subject: "CN=ftp.example.org".into(),
        issuer: "CN=ftp.example.org".into(),
        not_after: "2027-01-01 00:00:00 UTC".into(),
        sha256: "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08".into(),
    });
    insta::assert_snapshot!("prompt_certificate", prompt_screen(cert));
    let password = PromptKind::Credential(CredentialPrompt::Password {
        site: "prod-web".into(),
        user: "deploy".into(),
        retry: true,
    });
    insta::assert_snapshot!("prompt_password", prompt_screen(password));
    let kbd = PromptKind::Credential(CredentialPrompt::KeyboardInteractive {
        site: "prod-web".into(),
        name: "Two-factor".into(),
        instructions: "Enter the code from your authenticator.".into(),
        prompts: vec![("Verification code: ".into(), true)],
    });
    insta::assert_snapshot!("prompt_keyboard_interactive", prompt_screen(kbd));
    let conflict = PromptKind::Conflict {
        transfer: TransferId(1),
        conflict: ConflictInfo {
            source: crate::test_support::entry("index.php", false, 4400, 2),
            target: crate::test_support::entry("index.php", false, 3900, 40),
        },
    };
    insta::assert_snapshot!("prompt_conflict", prompt_screen(conflict));
    let delete = PromptKind::ConfirmDelete {
        pane: PaneId::Remote,
        names: vec!["html".into(), "index.php".into()],
        recursive: true,
    };
    insta::assert_snapshot!("prompt_delete", prompt_screen(delete));
    let quit = PromptKind::ConfirmQuit {
        active_transfers: 3,
    };
    insta::assert_snapshot!("prompt_quit", prompt_screen(quit));
    let message = PromptKind::Message {
        level: Level::Warning,
        title: "FileZilla import".into(),
        body: "Imported 6 sites into \"FileZilla import 2026-10-05\".\nSkipped 1: unsupported protocol.".into(),
    };
    insta::assert_snapshot!("prompt_message", prompt_screen(message));
}

#[test]
fn chmod_go_to_and_import_dialogs_are_drawn() {
    use crate::dialog::{Dialog, InputDialog, InputPurpose};
    use crate::dialog_util::{ChmodDialog, ImportDialog};
    use filecargo_app_core::prelude::PaneId;
    let app = connected_app();
    let chmod = Dialog::Chmod(ChmodDialog::new(vec!["index.php".into()], 0o644));
    insta::assert_snapshot!("chmod", draw(&dialog_ui(&app, chmod), &app));
    let goto = Dialog::Input(InputDialog::new(
        "Go to path",
        "Path",
        "/var/www",
        InputPurpose::GoTo {
            pane: PaneId::Remote,
        },
    ));
    insta::assert_snapshot!("goto", draw(&dialog_ui(&app, goto), &app));
    let import = Dialog::Import(ImportDialog::new(
        "/home/me/.config/filezilla/sitemanager.xml",
    ));
    insta::assert_snapshot!("import", draw(&dialog_ui(&app, import), &app));
}

fn bottom_screen(
    tab: crate::ui_state::BottomTab,
    focused: bool,
    tweak: impl FnOnce(&mut filecargo_app_core::prelude::AppState),
) -> String {
    let mut app = connected_app();
    app.queue = std::sync::Arc::new(crate::test_support::busy_queue());
    tweak(&mut app);
    let mut ui = synced(100, 30, &app);
    ui.bottom.tab = tab;
    if focused {
        ui.focus = crate::ui_state::Focus::Bottom;
    }
    draw(&ui, &app)
}

#[test]
fn the_queue_tab_shows_progress_speed_eta_and_totals() {
    use crate::ui_state::BottomTab;
    insta::assert_snapshot!(bottom_screen(BottomTab::Queue, true, |_| {}));
}

#[test]
fn a_paused_queue_says_so_in_the_footer() {
    use crate::ui_state::BottomTab;
    let screen = bottom_screen(BottomTab::Queue, false, |app| {
        let mut queue = (*app.queue).clone();
        queue.processing = false;
        app.queue = std::sync::Arc::new(queue);
    });
    assert!(screen.contains("PAUSED"));
}

#[test]
fn completed_and_failed_tabs_list_their_items() {
    use crate::ui_state::BottomTab;
    insta::assert_snapshot!(
        "completed",
        bottom_screen(BottomTab::Completed, true, |_| {})
    );
    insta::assert_snapshot!("failed", bottom_screen(BottomTab::Failed, true, |_| {}));
}

#[test]
fn empty_lists_say_what_they_are_for() {
    use crate::ui_state::BottomTab;
    let screen = bottom_screen(BottomTab::Queue, false, |app| {
        app.queue = std::sync::Arc::new(Default::default());
    });
    assert!(screen.contains("Nothing queued"));
}

#[test]
fn the_log_tab_shows_the_newest_lines_with_levels() {
    use crate::ui_state::BottomTab;
    use filecargo_app_core::prelude::{LogLevel, LogLine};
    let mut app = connected_app();
    app.queue = std::sync::Arc::new(Default::default());
    let mut ui = synced(100, 30, &app);
    ui.bottom.tab = BottomTab::Log;
    for (n, level) in [LogLevel::Info, LogLevel::Warn, LogLevel::Error]
        .into_iter()
        .enumerate()
    {
        ui.log.push(LogLine {
            time: std::time::UNIX_EPOCH + std::time::Duration::from_secs(50_000 + n as u64),
            level,
            target: "filecargo::session".into(),
            message: format!("event number {n}"),
        });
    }
    insta::assert_snapshot!(draw(&ui, &app));
}

fn terminal_screen(state: filecargo_app_core::prelude::TerminalState) -> String {
    let mut app = connected_app();
    app.terminal = state;
    let mut ui = synced(100, 30, &app);
    ui.bottom.tab = crate::ui_state::BottomTab::Terminal;
    ui.focus = crate::ui_state::Focus::Bottom;
    draw(&ui, &app)
}

#[tokio::test]
async fn the_terminal_tab_draws_the_shell_screen_with_colours_and_the_cursor() {
    use crate::test_support::FakeShell;
    use filecargo_app_core::prelude::TerminalState;
    let shell = FakeShell::open(
        98,
        4,
        "me@prod:~$ ls\r\n\x1b[1;34mhtml\x1b[0m  index.php  style.css\r\nme@prod:~$ ",
    )
    .await;
    insta::assert_snapshot!(terminal_screen(TerminalState::Open(shell.view.clone())));
}

#[tokio::test]
async fn an_exited_shell_keeps_its_screen_and_offers_to_reopen() {
    use crate::test_support::FakeShell;
    use filecargo_app_core::prelude::TerminalState;
    let shell = FakeShell::open(98, 3, "logout\r\n").await;
    insta::assert_snapshot!(terminal_screen(TerminalState::Exited {
        code: Some(0),
        view: shell.view.clone()
    }));
}

#[test]
fn the_terminal_tab_explains_why_there_is_no_shell() {
    use filecargo_app_core::prelude::TerminalState;
    assert!(terminal_screen(TerminalState::NotAvailable).contains("needs an SFTP connection"));
    assert!(terminal_screen(TerminalState::Closed).contains("Opening the shell"));
}
