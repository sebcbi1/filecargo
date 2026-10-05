//! Modal dialogs of the server tree and the remote pane: the site editor, one-line inputs,
//! confirmations and the folder picker. Each turns keys into `Command`s; none does I/O.

use std::path::PathBuf;

use filecargo_app_core::prelude::*;
use ratatui::crossterm::event::{KeyCode, KeyEvent};

use crate::form::{Field, Form, FormOutcome};

/// What a dialog wants after a key.
#[derive(Debug)]
pub enum Outcome {
    Keep,
    Close,
    /// Close the dialog and run these.
    Run(Vec<Command>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputPurpose {
    NewFolder { parent: Option<FolderId> },
    RenameNode(NodeId),
    Mkdir,
    RenameRemote { from: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputDialog {
    pub title: String,
    pub form: Form,
    pub purpose: InputPurpose,
}

impl InputDialog {
    pub fn new(title: &str, label: &'static str, value: &str, purpose: InputPurpose) -> Self {
        Self {
            title: title.to_owned(),
            form: Form::new(vec![Field::text("value", label, value)]),
            purpose,
        }
    }

    fn on_key(&mut self, key: KeyEvent) -> Outcome {
        match self.form.on_key(key) {
            FormOutcome::Cancel => Outcome::Close,
            FormOutcome::Handled => Outcome::Keep,
            FormOutcome::Submit => {
                let value = self.form.text_of("value").to_owned();
                if value.is_empty() {
                    self.form.error = Some("The name cannot be empty.".to_owned());
                    return Outcome::Keep;
                }
                if value.contains('/') {
                    self.form.error = Some("The name cannot contain '/'.".to_owned());
                    return Outcome::Keep;
                }
                let command = match &self.purpose {
                    InputPurpose::NewFolder { parent } => Command::Tree(TreeOp::AddFolder {
                        name: value,
                        parent: *parent,
                    }),
                    InputPurpose::RenameNode(node) => Command::Tree(TreeOp::Rename {
                        node: *node,
                        name: value,
                    }),
                    InputPurpose::Mkdir => Command::Mkdir { name: value },
                    InputPurpose::RenameRemote { from } => Command::Rename {
                        from: from.clone(),
                        to: value,
                    },
                };
                Outcome::Run(vec![command])
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmDialog {
    pub title: String,
    pub body: String,
    pub op: TreeOp,
}

impl ConfirmDialog {
    fn on_key(&self, key: KeyEvent) -> Outcome {
        match key.code {
            KeyCode::Enter | KeyCode::Char('y' | 'Y') => {
                Outcome::Run(vec![Command::Tree(self.op.clone())])
            }
            KeyCode::Esc | KeyCode::Char('n' | 'N') => Outcome::Close,
            _ => Outcome::Keep,
        }
    }
}

/// Picks the folder a node moves to; the first choice is the root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MovePicker {
    pub node: NodeId,
    pub choices: Vec<(Option<FolderId>, String)>,
    pub cursor: usize,
}

impl MovePicker {
    /// Every folder except the node itself and its descendants.
    pub fn new(tree: &ServerTree, node: NodeId) -> Self {
        let mut choices = vec![(None, "/ (top level)".to_owned())];
        for folder in tree.folders() {
            if Self::inside(tree, folder.id, node) {
                continue;
            }
            choices.push((Some(folder.id), Self::path_of(tree, folder.id)));
        }
        choices[1..].sort_by(|a, b| a.1.cmp(&b.1));
        let current = match node {
            NodeId::Site(id) => tree.site(id).and_then(|s| s.folder),
            NodeId::Folder(id) => tree.folder(id).and_then(|f| f.parent),
        };
        let cursor = choices.iter().position(|c| c.0 == current).unwrap_or(0);
        Self {
            node,
            choices,
            cursor,
        }
    }

    fn path_of(tree: &ServerTree, id: FolderId) -> String {
        let mut names = Vec::new();
        let mut at = Some(id);
        while let Some(folder) = at.and_then(|i| tree.folder(i)) {
            names.push(folder.name.clone());
            at = folder.parent;
        }
        names.reverse();
        format!("/{}", names.join("/"))
    }

    /// Whether `folder` is `node` or sits below it.
    fn inside(tree: &ServerTree, folder: FolderId, node: NodeId) -> bool {
        let NodeId::Folder(root) = node else {
            return false;
        };
        let mut at = Some(folder);
        while let Some(id) = at {
            if id == root {
                return true;
            }
            at = tree.folder(id).and_then(|f| f.parent);
        }
        false
    }

    fn on_key(&mut self, key: KeyEvent) -> Outcome {
        let last = self.choices.len() - 1;
        match key.code {
            KeyCode::Esc => Outcome::Close,
            KeyCode::Up | KeyCode::Char('k') => {
                self.cursor = self.cursor.saturating_sub(1);
                Outcome::Keep
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.cursor = (self.cursor + 1).min(last);
                Outcome::Keep
            }
            KeyCode::Enter => Outcome::Run(vec![Command::Tree(TreeOp::Move {
                node: self.node,
                parent: self.choices[self.cursor].0,
            })]),
            _ => Outcome::Keep,
        }
    }
}

const PROTOCOLS: [&str; 4] = ["SFTP", "FTP", "FTPS (explicit)", "FTPS (implicit)"];
const AUTHS: [&str; 4] = ["Password", "Key file", "Agent", "Anonymous"];
const MODES: [&str; 2] = ["Passive", "Active"];

fn protocol_at(index: usize) -> Protocol {
    match index {
        0 => Protocol::Sftp,
        1 => Protocol::Ftp,
        2 => Protocol::FtpsExplicit,
        _ => Protocol::FtpsImplicit,
    }
}

fn protocol_index(protocol: Protocol) -> usize {
    match protocol {
        Protocol::Sftp => 0,
        Protocol::Ftp => 1,
        Protocol::FtpsExplicit => 2,
        Protocol::FtpsImplicit => 3,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteEditor {
    pub form: Form,
    /// The site being edited, `None` for a new one.
    pub original: Option<Site>,
    /// Where a new site goes.
    pub folder: Option<FolderId>,
}

impl SiteEditor {
    pub fn new_site(folder: Option<FolderId>) -> Self {
        Self::build(Site::new("", Protocol::Sftp, ""), None, folder)
    }

    pub fn edit(site: &Site) -> Self {
        Self::build(site.clone(), Some(site.clone()), site.folder)
    }

    fn build(site: Site, original: Option<Site>, folder: Option<FolderId>) -> Self {
        let (auth, remember, key) = match &site.auth {
            Auth::Password { remember } => (0, *remember, String::new()),
            Auth::KeyFile {
                path,
                remember_passphrase,
            } => (1, *remember_passphrase, path.display().to_string()),
            Auth::Agent => (2, false, String::new()),
            Auth::Anonymous => (3, false, String::new()),
        };
        let port = site.port.map(|p| p.to_string()).unwrap_or_default();
        let mut form = Form::new(vec![
            Field::text("name", "Name", &site.name),
            Field::select(
                "protocol",
                "Protocol",
                PROTOCOLS.to_vec(),
                protocol_index(site.protocol),
            ),
            Field::text("host", "Host", &site.host),
            Field::text("port", "Port", &port),
            Field::text("user", "User", &site.user),
            Field::select("auth", "Login", AUTHS.to_vec(), auth),
            Field::masked("password", "Password"),
            Field::text("key", "Key file", &key),
            Field::checkbox("remember", "Remember secret", remember),
            Field::select(
                "ftp_mode",
                "FTP mode",
                MODES.to_vec(),
                usize::from(site.ftp_mode == FtpMode::Active),
            ),
            Field::text(
                "remote_dir",
                "Remote dir",
                site.remote_dir.as_deref().unwrap_or(""),
            ),
            Field::text(
                "local_dir",
                "Local dir",
                &site
                    .local_dir
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
            ),
            Field::text("notes", "Notes", &site.notes),
        ]);
        Self::adapt(&mut form);
        Self {
            form,
            original,
            folder,
        }
    }

    /// Shows only the fields that apply to the chosen protocol and login.
    fn adapt(form: &mut Form) {
        let auth = form.choice("auth");
        let ftp = protocol_at(form.choice("protocol")).is_ftp_family();
        form.set_visible("password", auth == 0);
        form.set_visible("key", auth == 1);
        form.set_visible("remember", auth <= 1);
        form.set_visible("ftp_mode", ftp);
    }

    fn on_key(&mut self, key: KeyEvent) -> Outcome {
        match self.form.on_key(key) {
            FormOutcome::Cancel => Outcome::Close,
            FormOutcome::Handled => {
                Self::adapt(&mut self.form);
                Outcome::Keep
            }
            FormOutcome::Submit => match self.to_site() {
                Ok(site) => Outcome::Run(self.commands(site)),
                Err(message) => {
                    self.form.error = Some(message);
                    Outcome::Keep
                }
            },
        }
    }

    pub fn paste(&mut self, text: &str) {
        self.form.paste(text);
    }

    fn to_site(&self) -> Result<Site, String> {
        let form = &self.form;
        let name = form.text_of("name");
        if name.is_empty() {
            return Err("Give the site a name.".to_owned());
        }
        let host = form.text_of("host");
        if host.is_empty() {
            return Err("Enter a host.".to_owned());
        }
        let protocol = protocol_at(form.choice("protocol"));
        let port = match form.text_of("port") {
            "" => None,
            text => match text.parse::<u16>() {
                Ok(port) if port > 0 => Some(port),
                _ => return Err("The port must be a number from 1 to 65535.".to_owned()),
            },
        };
        let remember = form.checked("remember");
        let auth = match form.choice("auth") {
            0 => Auth::Password { remember },
            1 if protocol == Protocol::Sftp => {
                let path = form.text_of("key");
                if path.is_empty() {
                    return Err("Enter the path of the key file.".to_owned());
                }
                Auth::KeyFile {
                    path: PathBuf::from(path),
                    remember_passphrase: remember,
                }
            }
            2 if protocol == Protocol::Sftp => Auth::Agent,
            3 if protocol.is_ftp_family() => Auth::Anonymous,
            1 | 2 => return Err("Key file and agent login are for SFTP only.".to_owned()),
            _ => return Err("Anonymous login is for FTP and FTPS only.".to_owned()),
        };
        let mut site = self
            .original
            .clone()
            .unwrap_or_else(|| Site::new(name, protocol, host));
        site.name = name.to_owned();
        site.folder = self.folder;
        site.protocol = protocol;
        site.host = host.to_owned();
        site.port = port;
        site.user = form.text_of("user").to_owned();
        site.auth = auth;
        site.ftp_mode = if protocol.is_ftp_family() && form.choice("ftp_mode") == 1 {
            FtpMode::Active
        } else {
            FtpMode::Passive
        };
        site.remote_dir = Some(form.text_of("remote_dir"))
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        site.local_dir = Some(form.text_of("local_dir"))
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);
        site.notes = form.text_of("notes").to_owned();
        Ok(site)
    }

    fn commands(&self, site: Site) -> Vec<Command> {
        let id = site.id;
        let password = self
            .form
            .field("password")
            .map(|f| f.text.clone())
            .unwrap_or_default();
        let store = matches!(site.auth, Auth::Password { remember: true }) && !password.is_empty();
        let op = if self.original.is_some() {
            TreeOp::UpdateSite(site)
        } else {
            TreeOp::AddSite(site)
        };
        let mut commands = vec![Command::Tree(op)];
        if store {
            commands.push(Command::SetSitePassword {
                site: id,
                secret: SecretString::from(password),
            });
        }
        commands
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dialog {
    Site(Box<SiteEditor>),
    Input(InputDialog),
    Confirm(ConfirmDialog),
    Move(MovePicker),
}

impl Dialog {
    pub fn on_key(&mut self, key: KeyEvent) -> Outcome {
        match self {
            Self::Site(editor) => editor.on_key(key),
            Self::Input(input) => input.on_key(key),
            Self::Confirm(confirm) => confirm.on_key(key),
            Self::Move(picker) => picker.on_key(key),
        }
    }

    pub fn paste(&mut self, text: &str) {
        match self {
            Self::Site(editor) => editor.paste(text),
            Self::Input(input) => input.form.paste(text),
            Self::Confirm(_) | Self::Move(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use filecargo_config::ExposeSecret;
    use ratatui::crossterm::event::KeyModifiers;

    use super::*;

    macro_rules! same {
        ($left:expr, $right:expr) => {
            assert_eq!(format!("{:?}", $left), format!("{:?}", $right))
        };
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn type_text(dialog: &mut Dialog, text: &str) {
        for c in text.chars() {
            dialog.on_key(key(KeyCode::Char(c)));
        }
    }

    fn tab(dialog: &mut Dialog, times: usize) {
        for _ in 0..times {
            dialog.on_key(key(KeyCode::Tab));
        }
    }

    fn error_of(dialog: &Dialog) -> Option<&str> {
        match dialog {
            Dialog::Site(e) => e.form.error.as_deref(),
            Dialog::Input(i) => i.form.error.as_deref(),
            _ => None,
        }
    }

    #[test]
    fn a_new_site_needs_a_name_and_a_host() {
        let mut dialog = Dialog::Site(Box::new(SiteEditor::new_site(None)));
        same!(dialog.on_key(key(KeyCode::Enter)), Outcome::Keep);
        assert_eq!(error_of(&dialog), Some("Give the site a name."));
        type_text(&mut dialog, "work");
        dialog.on_key(key(KeyCode::Enter));
        assert_eq!(error_of(&dialog), Some("Enter a host."));
    }

    #[test]
    fn a_filled_in_form_adds_the_site_with_its_password_when_remembered() {
        let mut dialog = Dialog::Site(Box::new(SiteEditor::new_site(None)));
        type_text(&mut dialog, "work");
        tab(&mut dialog, 2); // host
        type_text(&mut dialog, "example.org");
        tab(&mut dialog, 1); // port
        type_text(&mut dialog, "2222");
        tab(&mut dialog, 1); // user
        type_text(&mut dialog, "me");
        tab(&mut dialog, 2); // password (login is Password)
        type_text(&mut dialog, "hunter2");
        tab(&mut dialog, 1); // remember
        dialog.on_key(key(KeyCode::Char(' ')));
        let Outcome::Run(commands) = dialog.on_key(key(KeyCode::Enter)) else {
            panic!("expected commands, error: {:?}", error_of(&dialog));
        };
        assert_eq!(commands.len(), 2);
        let Command::Tree(TreeOp::AddSite(site)) = &commands[0] else {
            panic!("{commands:?}")
        };
        assert_eq!(site.name, "work");
        assert_eq!(site.host, "example.org");
        assert_eq!(site.port, Some(2222));
        assert_eq!(site.user, "me");
        assert_eq!(site.auth, Auth::Password { remember: true });
        let Command::SetSitePassword { site: id, secret } = &commands[1] else {
            panic!("{commands:?}")
        };
        assert_eq!(*id, site.id);
        assert_eq!(secret.expose_secret(), "hunter2");
    }

    #[test]
    fn a_password_that_is_not_remembered_is_not_stored() {
        let mut dialog = Dialog::Site(Box::new(SiteEditor::new_site(None)));
        type_text(&mut dialog, "work");
        tab(&mut dialog, 2);
        type_text(&mut dialog, "example.org");
        tab(&mut dialog, 4);
        type_text(&mut dialog, "hunter2");
        let Outcome::Run(commands) = dialog.on_key(key(KeyCode::Enter)) else {
            panic!()
        };
        assert_eq!(commands.len(), 1, "{commands:?}");
    }

    #[test]
    fn a_bad_port_is_refused_with_a_message() {
        let mut dialog = Dialog::Site(Box::new(SiteEditor::new_site(None)));
        type_text(&mut dialog, "work");
        tab(&mut dialog, 2);
        type_text(&mut dialog, "example.org");
        tab(&mut dialog, 1);
        type_text(&mut dialog, "99999");
        same!(dialog.on_key(key(KeyCode::Enter)), Outcome::Keep);
        assert_eq!(
            error_of(&dialog),
            Some("The port must be a number from 1 to 65535.")
        );
    }

    #[test]
    fn login_choice_shows_the_matching_fields_and_rejects_the_wrong_protocol() {
        let mut editor = SiteEditor::new_site(None);
        let visible = |e: &SiteEditor, id: &str| e.form.field(id).unwrap().visible;
        assert!(
            visible(&editor, "password")
                && !visible(&editor, "key")
                && !visible(&editor, "ftp_mode")
        );
        editor.form.field_mut("name").unwrap().set_text("w");
        editor.form.field_mut("host").unwrap().set_text("h");
        editor.form.field_mut("auth").unwrap().choice = 3; // anonymous on SFTP
        SiteEditor::adapt(&mut editor.form);
        assert!(!visible(&editor, "password") && !visible(&editor, "remember"));
        same!(
            editor.to_site().unwrap_err(),
            "Anonymous login is for FTP and FTPS only."
        );
        editor.form.field_mut("protocol").unwrap().choice = 1; // FTP
        SiteEditor::adapt(&mut editor.form);
        assert!(visible(&editor, "ftp_mode"));
        assert_eq!(editor.to_site().unwrap().auth, Auth::Anonymous);
        editor.form.field_mut("auth").unwrap().choice = 1; // key file on FTP
        assert!(editor.to_site().is_err());
    }

    #[test]
    fn editing_keeps_the_site_identity_and_cancel_changes_nothing() {
        let mut site = Site::new("old", Protocol::Sftp, "h");
        site.auth = Auth::KeyFile {
            path: "/k".into(),
            remember_passphrase: true,
        };
        let mut dialog = Dialog::Site(Box::new(SiteEditor::edit(&site)));
        dialog.on_key(key(KeyCode::Char('!')));
        let Outcome::Run(commands) = dialog.on_key(key(KeyCode::Enter)) else {
            panic!()
        };
        let Command::Tree(TreeOp::UpdateSite(updated)) = &commands[0] else {
            panic!("{commands:?}")
        };
        assert_eq!(updated.id, site.id);
        assert_eq!(updated.name, "old!");
        assert_eq!(
            updated.auth, site.auth,
            "key path and remember survive a round trip"
        );
        assert_eq!(commands.len(), 1);

        let mut dialog = Dialog::Site(Box::new(SiteEditor::edit(&site)));
        same!(dialog.on_key(key(KeyCode::Esc)), Outcome::Close);
    }

    #[test]
    fn input_dialogs_validate_and_build_their_command() {
        let mut dialog = Dialog::Input(InputDialog::new(
            "New folder",
            "Name",
            "",
            InputPurpose::Mkdir,
        ));
        same!(dialog.on_key(key(KeyCode::Enter)), Outcome::Keep);
        assert_eq!(error_of(&dialog), Some("The name cannot be empty."));
        type_text(&mut dialog, "a/b");
        dialog.on_key(key(KeyCode::Enter));
        assert_eq!(error_of(&dialog), Some("The name cannot contain '/'."));
        dialog.on_key(key(KeyCode::Backspace));
        dialog.on_key(key(KeyCode::Backspace));
        dialog.on_key(key(KeyCode::Char('x')));
        same!(
            dialog.on_key(key(KeyCode::Enter)),
            Outcome::Run(vec![Command::Mkdir { name: "ax".into() }])
        );

        let mut rename = Dialog::Input(InputDialog::new(
            "Rename",
            "Name",
            "old.txt",
            InputPurpose::RenameRemote {
                from: "old.txt".into(),
            },
        ));
        type_text(&mut rename, "2");
        same!(
            rename.on_key(key(KeyCode::Enter)),
            Outcome::Run(vec![Command::Rename {
                from: "old.txt".into(),
                to: "old.txt2".into()
            }])
        );
    }

    #[test]
    fn confirm_runs_on_enter_or_y_and_closes_on_anything_negative() {
        let op = TreeOp::Delete {
            node: NodeId::Site(SiteId::new()),
        };
        let mut dialog = Dialog::Confirm(ConfirmDialog {
            title: "Delete".into(),
            body: "Delete work?".into(),
            op: op.clone(),
        });
        same!(dialog.on_key(key(KeyCode::Char('x'))), Outcome::Keep);
        same!(dialog.on_key(key(KeyCode::Char('n'))), Outcome::Close);
        same!(dialog.on_key(key(KeyCode::Esc)), Outcome::Close);
        same!(
            dialog.on_key(key(KeyCode::Char('y'))),
            Outcome::Run(vec![Command::Tree(op.clone())])
        );
        same!(
            dialog.on_key(key(KeyCode::Enter)),
            Outcome::Run(vec![Command::Tree(op)])
        );
    }
}
