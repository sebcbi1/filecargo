//! Turning the site editor's fields into a `Site` and the commands to save it. Pure: the view
//! only collects the values.

use std::path::PathBuf;

use filecargo_app_core::prelude::*;

pub const PROTOCOLS: [&str; 4] = ["SFTP", "FTP", "FTPS (explicit)", "FTPS (implicit)"];
pub const AUTHS: [&str; 4] = ["Password", "Key file", "Agent", "Anonymous"];
pub const FTP_MODES: [&str; 2] = ["Passive", "Active"];

pub fn protocol_at(index: usize) -> Protocol {
    match index {
        0 => Protocol::Sftp,
        1 => Protocol::Ftp,
        2 => Protocol::FtpsExplicit,
        _ => Protocol::FtpsImplicit,
    }
}

pub fn protocol_index(protocol: Protocol) -> usize {
    match protocol {
        Protocol::Sftp => 0,
        Protocol::Ftp => 1,
        Protocol::FtpsExplicit => 2,
        Protocol::FtpsImplicit => 3,
    }
}

/// What the editor's controls hold.
#[derive(Debug, Clone, Default)]
pub struct FormValues {
    pub name: String,
    pub protocol: usize,
    pub host: String,
    pub port: String,
    pub user: String,
    pub auth: usize,
    pub password: String,
    pub key_path: String,
    pub remember: bool,
    pub ftp_active: bool,
    pub remote_dir: String,
    pub local_dir: String,
    pub notes: String,
}

impl FormValues {
    /// The values an existing site starts the editor with (the password is never shown).
    pub fn of(site: &Site) -> Self {
        let (auth, remember, key_path) = match &site.auth {
            Auth::Password { remember } => (0, *remember, String::new()),
            Auth::KeyFile {
                path,
                remember_passphrase,
            } => (1, *remember_passphrase, path.display().to_string()),
            Auth::Agent => (2, false, String::new()),
            Auth::Anonymous => (3, false, String::new()),
        };
        Self {
            name: site.name.clone(),
            protocol: protocol_index(site.protocol),
            host: site.host.clone(),
            port: site.port.map(|p| p.to_string()).unwrap_or_default(),
            user: site.user.clone(),
            auth,
            password: String::new(),
            key_path,
            remember,
            ftp_active: site.ftp_mode == FtpMode::Active,
            remote_dir: site.remote_dir.clone().unwrap_or_default(),
            local_dir: site
                .local_dir
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            notes: site.notes.clone(),
        }
    }

    /// The site these values describe: a copy of `original` with the fields replaced, or a new
    /// one in `folder`.
    pub fn to_site(
        &self,
        original: Option<&Site>,
        folder: Option<FolderId>,
    ) -> Result<Site, String> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err("Give the site a name.".to_owned());
        }
        let host = self.host.trim();
        if host.is_empty() {
            return Err("Enter a host.".to_owned());
        }
        let protocol = protocol_at(self.protocol);
        let port = match self.port.trim() {
            "" => None,
            text => match text.parse::<u16>() {
                Ok(port) if port > 0 => Some(port),
                _ => return Err("The port must be a number from 1 to 65535.".to_owned()),
            },
        };
        let auth = match self.auth {
            0 => Auth::Password {
                remember: self.remember,
            },
            1 if protocol == Protocol::Sftp => {
                let path = self.key_path.trim();
                if path.is_empty() {
                    return Err("Choose the key file.".to_owned());
                }
                Auth::KeyFile {
                    path: PathBuf::from(path),
                    remember_passphrase: self.remember,
                }
            }
            2 if protocol == Protocol::Sftp => Auth::Agent,
            3 if protocol.is_ftp_family() => Auth::Anonymous,
            1 | 2 => return Err("Key file and agent login are for SFTP only.".to_owned()),
            _ => return Err("Anonymous login is for FTP and FTPS only.".to_owned()),
        };
        let mut site = original
            .cloned()
            .unwrap_or_else(|| Site::new(name, protocol, host));
        site.name = name.to_owned();
        if original.is_none() {
            site.folder = folder;
        }
        site.protocol = protocol;
        site.host = host.to_owned();
        site.port = port;
        site.user = self.user.trim().to_owned();
        site.auth = auth;
        site.ftp_mode = if protocol.is_ftp_family() && self.ftp_active {
            FtpMode::Active
        } else {
            FtpMode::Passive
        };
        site.remote_dir = Some(self.remote_dir.trim())
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        site.local_dir = Some(self.local_dir.trim())
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);
        site.notes = self.notes.clone();
        Ok(site)
    }

    /// The commands that save the form: the tree operation, then the password when it is to be
    /// remembered and was typed (an empty field keeps the stored one).
    pub fn commands(
        &self,
        original: Option<&Site>,
        folder: Option<FolderId>,
    ) -> Result<Vec<Command>, String> {
        let site = self.to_site(original, folder)?;
        let id = site.id;
        let store =
            matches!(site.auth, Auth::Password { remember: true }) && !self.password.is_empty();
        let op = if original.is_some() {
            TreeOp::UpdateSite(site)
        } else {
            TreeOp::AddSite(site)
        };
        let mut commands = vec![Command::Tree(op)];
        if store {
            commands.push(Command::SetSitePassword {
                site: id,
                secret: SecretString::from(self.password.clone()),
            });
        }
        Ok(commands)
    }
}

#[cfg(test)]
mod tests {
    use filecargo_config::ExposeSecret;

    use super::*;

    fn valid() -> FormValues {
        FormValues {
            name: "work".into(),
            host: "example.org".into(),
            ..FormValues::default()
        }
    }

    #[test]
    fn name_and_host_are_required_and_the_port_is_checked() {
        assert_eq!(
            FormValues::default().to_site(None, None).unwrap_err(),
            "Give the site a name."
        );
        let v = FormValues {
            name: "w".into(),
            ..FormValues::default()
        };
        assert_eq!(v.to_site(None, None).unwrap_err(), "Enter a host.");
        for bad in ["0", "70000", "x"] {
            let v = FormValues {
                port: bad.into(),
                ..valid()
            };
            assert_eq!(
                v.to_site(None, None).unwrap_err(),
                "The port must be a number from 1 to 65535."
            );
        }
        let v = FormValues {
            port: " 2222 ".into(),
            ..valid()
        };
        assert_eq!(v.to_site(None, None).unwrap().port, Some(2222));
    }

    #[test]
    fn login_methods_must_fit_the_protocol() {
        let anon = FormValues { auth: 3, ..valid() };
        assert!(anon.to_site(None, None).is_err(), "anonymous on SFTP");
        assert_eq!(
            FormValues {
                protocol: 1,
                ..anon
            }
            .to_site(None, None)
            .unwrap()
            .auth,
            Auth::Anonymous
        );
        let key = FormValues {
            auth: 1,
            protocol: 1,
            key_path: "/k".into(),
            ..valid()
        };
        assert!(key.to_site(None, None).is_err(), "key file on FTP");
        let key = FormValues {
            auth: 1,
            key_path: String::new(),
            ..valid()
        };
        assert_eq!(key.to_site(None, None).unwrap_err(), "Choose the key file.");
        let key = FormValues {
            auth: 1,
            key_path: "/k".into(),
            remember: true,
            ..valid()
        };
        assert_eq!(
            key.to_site(None, None).unwrap().auth,
            Auth::KeyFile {
                path: "/k".into(),
                remember_passphrase: true
            }
        );
    }

    #[test]
    fn a_new_site_goes_in_the_folder_and_an_edit_keeps_identity_and_folder() {
        let folder = FolderId::new();
        let site = valid().to_site(None, Some(folder)).unwrap();
        assert_eq!(site.folder, Some(folder));
        let edited = FormValues {
            name: "renamed".into(),
            ..FormValues::of(&site)
        }
        .to_site(Some(&site), Some(FolderId::new()))
        .unwrap();
        assert_eq!((edited.id, edited.folder), (site.id, Some(folder)));
        assert_eq!(edited.name, "renamed");
    }

    #[test]
    fn the_password_is_stored_only_when_remembered_and_typed() {
        let typed = FormValues {
            password: "hunter2".into(),
            remember: true,
            ..valid()
        };
        let commands = typed.commands(None, None).unwrap();
        assert_eq!(commands.len(), 2);
        let Command::SetSitePassword { secret, .. } = &commands[1] else {
            panic!()
        };
        assert_eq!(secret.expose_secret(), "hunter2");
        let forgotten = FormValues {
            remember: false,
            ..typed.clone()
        };
        assert_eq!(forgotten.commands(None, None).unwrap().len(), 1);
        let empty = FormValues {
            password: String::new(),
            ..typed
        };
        assert_eq!(
            empty.commands(None, None).unwrap().len(),
            1,
            "an empty field keeps the stored password"
        );
    }

    #[test]
    fn the_form_round_trips_an_existing_site() {
        let mut site = Site::new("x", Protocol::FtpsExplicit, "h");
        site.port = Some(990);
        site.user = "u".into();
        site.ftp_mode = FtpMode::Active;
        site.remote_dir = Some("/srv".into());
        site.local_dir = Some("/tmp".into());
        site.notes = "n".into();
        site.auth = Auth::Password { remember: true };
        let back = FormValues::of(&site).to_site(Some(&site), None).unwrap();
        assert_eq!(back, site);
    }
}
