//! FileZilla site manager import: maps the parsed tree onto folders and sites.

mod filezilla;

use std::collections::HashSet;
use std::path::PathBuf;

use etcetera::BaseStrategy;

use crate::model::{Auth, Folder, FolderId, FtpMode, Protocol, Site};
use crate::secrets::{SecretKey, SecretString};
use filezilla::{FzLogon, FzNode, FzPassword, FzProtocol, FzSite};

pub(crate) use filezilla::parse;

/// Options for [`ConfigStore::import_filezilla`](crate::ConfigStore::import_filezilla).
#[derive(Debug, Clone)]
pub struct ImportOptions {
    /// Store passwords that FileZilla saved readably (base64) in the keychain.
    pub import_passwords: bool,
    /// Name of the new root folder everything is imported under; made unique if taken.
    pub folder_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedSite {
    /// Path of the site inside the FileZilla file, e.g. `Work/backup-bucket`.
    pub site: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswordNotImported {
    /// Path of the imported site (inside the new root folder).
    pub site: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportReport {
    /// Final name of the new root folder; `None` when nothing was imported.
    pub root_folder: Option<String>,
    /// Paths of imported sites, relative to the root folder.
    pub imported: Vec<String>,
    pub skipped: Vec<SkippedSite>,
    pub passwords_skipped: Vec<PasswordNotImported>,
}

/// Everything an import will add, computed before anything is written.
pub(crate) struct Plan {
    pub folders: Vec<Folder>,
    pub sites: Vec<Site>,
    /// Secrets to store after the file write: key, value, site path for the report.
    pub secrets: Vec<(SecretKey, SecretString, String)>,
    pub report: ImportReport,
}

/// FileZilla's own site manager file for the current platform, if a home directory exists.
/// Only the path is computed; nothing is read.
pub fn default_filezilla_path() -> Option<PathBuf> {
    let strategy = etcetera::choose_base_strategy().ok()?;
    let dir = if cfg!(windows) {
        "FileZilla"
    } else {
        "filezilla"
    };
    Some(strategy.config_dir().join(dir).join("sitemanager.xml"))
}

pub(crate) fn plan(
    nodes: &[FzNode],
    opts: &ImportOptions,
    root_name_taken: impl Fn(&str) -> bool,
) -> Plan {
    let mut plan = Plan {
        folders: Vec::new(),
        sites: Vec::new(),
        secrets: Vec::new(),
        report: ImportReport::default(),
    };
    let root = Folder {
        id: FolderId::new(),
        name: unique_root_name(opts, root_name_taken),
        parent: None,
    };
    let root_id = root.id;
    let root_name = root.name.clone();
    walk(nodes, Some(root_id), "", opts, &mut plan);

    if plan.sites.is_empty() && plan.folders.is_empty() {
        return plan;
    }
    plan.folders.insert(0, root);
    plan.report.root_folder = Some(root_name);
    plan
}

fn unique_root_name(opts: &ImportOptions, taken: impl Fn(&str) -> bool) -> String {
    let base = match opts.folder_name.trim() {
        "" => "Imported from FileZilla",
        name => name,
    };
    let mut candidate = base.to_owned();
    let mut n = 2;
    while taken(&candidate) {
        candidate = format!("{base} ({n})");
        n += 1;
    }
    candidate
}

fn walk(
    nodes: &[FzNode],
    parent: Option<FolderId>,
    path: &str,
    opts: &ImportOptions,
    plan: &mut Plan,
) {
    let mut used: HashSet<String> = HashSet::new();
    for node in nodes {
        match node {
            FzNode::Folder { name, children } => {
                let name = unique_name(or_default(name, "Folder"), &mut used);
                let folder = Folder {
                    id: FolderId::new(),
                    name: name.clone(),
                    parent,
                };
                let id = folder.id;
                plan.folders.push(folder);
                walk(children, Some(id), &join(path, &name), opts, plan);
            }
            FzNode::Site(fz) => {
                let display = join(path, &fz.name);
                match map_site(fz, opts) {
                    Err(reason) => plan.report.skipped.push(SkippedSite {
                        site: display,
                        reason,
                    }),
                    Ok(mapped) => {
                        let name = unique_name(or_default(&fz.name, &fz.host), &mut used);
                        let site_path = join(path, &name);
                        let mut site = mapped.site;
                        site.name = name;
                        site.folder = parent;
                        collect_password(plan, &site, &mapped.password, &site_path, opts);
                        plan.report.imported.push(site_path);
                        plan.sites.push(site);
                    }
                }
            }
        }
    }
}

struct Mapped {
    site: Site,
    password: FzPassword,
}

fn map_site(fz: &FzSite, opts: &ImportOptions) -> Result<Mapped, String> {
    if fz.host.trim().is_empty() {
        return Err("site has no host".into());
    }
    let protocol = match fz.protocol {
        FzProtocol::Ftp | FzProtocol::InsecureFtp => Protocol::Ftp,
        FzProtocol::Sftp => Protocol::Sftp,
        FzProtocol::FtpsImplicit => Protocol::FtpsImplicit,
        FzProtocol::FtpsExplicit => Protocol::FtpsExplicit,
        FzProtocol::Unsupported(code) => {
            return Err(format!(
                "{} is not supported",
                filezilla::unsupported_protocol_name(code)
            ));
        }
    };
    let auth = match (fz.logon, protocol) {
        (FzLogon::Anonymous, Protocol::Sftp) => {
            return Err("anonymous logon is not valid for SFTP".into());
        }
        (FzLogon::Anonymous, _) => Auth::Anonymous,
        (FzLogon::Normal | FzLogon::Account, _) => Auth::Password { remember: true },
        (FzLogon::Ask | FzLogon::Interactive, _) => Auth::Password { remember: false },
        (FzLogon::Key, Protocol::Sftp) => match &fz.keyfile {
            Some(path) => Auth::KeyFile {
                path: path.into(),
                remember_passphrase: opts.import_passwords
                    && matches!(fz.password, FzPassword::Plain(_)),
            },
            None => return Err("key file logon without a key file".into()),
        },
        (FzLogon::Key, _) => return Err("key file logon is only valid for SFTP".into()),
        (FzLogon::Unsupported(code), _) => {
            return Err(format!("logon type #{code} is not supported"));
        }
    };

    let mut site = Site::new(fz.name.clone(), protocol, fz.host.clone());
    site.port = fz.port.filter(|p| *p != protocol.default_port());
    site.user = fz.user.clone();
    site.auth = auth;
    if protocol.is_ftp_family() && fz.active_mode == Some(true) {
        site.ftp_mode = FtpMode::Active;
    }
    site.remote_dir = fz.remote_dir.clone();
    site.local_dir = fz.local_dir.as_ref().map(PathBuf::from);
    site.notes = fz.notes.clone();
    Ok(Mapped {
        site,
        password: fz.password.clone(),
    })
}

fn collect_password(
    plan: &mut Plan,
    site: &Site,
    password: &FzPassword,
    site_path: &str,
    opts: &ImportOptions,
) {
    let mut skip = |reason: &str| {
        plan.report.passwords_skipped.push(PasswordNotImported {
            site: site_path.to_owned(),
            reason: reason.to_owned(),
        });
    };
    match password {
        FzPassword::None => {}
        FzPassword::Unreadable => skip("protected by a FileZilla master password"),
        FzPassword::Plain(_) if !opts.import_passwords => skip("password import not requested"),
        FzPassword::Plain(pw) => {
            let key = match &site.auth {
                Auth::Password { remember: true } => SecretKey::Password(site.id),
                Auth::KeyFile {
                    remember_passphrase: true,
                    ..
                } => SecretKey::Passphrase(site.id),
                _ => {
                    skip("logon type does not use a saved password");
                    return;
                }
            };
            plan.secrets.push((
                key,
                SecretString::from(pw.expose().to_owned()),
                site_path.to_owned(),
            ));
        }
    }
}

fn or_default<'a>(name: &'a str, fallback: &'a str) -> &'a str {
    match name.trim() {
        "" => fallback,
        trimmed => trimmed,
    }
}

/// `name`, or `name (2)`, `name (3)`, ... when a sibling already uses it (case-insensitive).
fn unique_name(base: &str, used: &mut HashSet<String>) -> String {
    let mut candidate = base.to_owned();
    let mut n = 2;
    while !used.insert(candidate.to_lowercase()) {
        candidate = format!("{base} ({n})");
        n += 1;
    }
    candidate
}

fn join(path: &str, name: &str) -> String {
    if path.is_empty() {
        name.to_owned()
    } else {
        format!("{path}/{name}")
    }
}
