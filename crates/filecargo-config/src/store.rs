use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::{ConfigError, ImportError};
use crate::fsio;
use crate::import::{self, ImportOptions, ImportReport};
use crate::model::{Auth, Folder, NodeId, Site};
use crate::paths::Paths;
use crate::secrets::{SecretKey, SecretStore};
use crate::settings::{ConnectionSettings, LogSettings, Settings, TransferSettings, UiSettings};
use crate::tree::{Outcome, ServerTree, TreeOp};

const FILE_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct ServersFile {
    version: u32,
    #[serde(default, rename = "folder", skip_serializing_if = "Vec::is_empty")]
    folders: Vec<Folder>,
    #[serde(default, rename = "site", skip_serializing_if = "Vec::is_empty")]
    sites: Vec<Site>,
}

const SETTINGS_VERSION: u32 = 1;

fn settings_version() -> u32 {
    SETTINGS_VERSION
}

/// Explicit fields instead of `#[serde(flatten)]`, which discards the error position that
/// [`parse_toml`] reports as a line number.
#[derive(Serialize, Deserialize)]
#[serde(default)]
struct SettingsFile {
    version: u32,
    transfers: TransferSettings,
    connection: ConnectionSettings,
    ui: UiSettings,
    log: LogSettings,
}

impl Default for SettingsFile {
    fn default() -> Self {
        Self::new(Settings::default())
    }
}

impl SettingsFile {
    fn new(settings: Settings) -> Self {
        Self {
            version: settings_version(),
            transfers: settings.transfers,
            connection: settings.connection,
            ui: settings.ui,
            log: settings.log,
        }
    }

    fn into_settings(self) -> Settings {
        Settings {
            transfers: self.transfers,
            connection: self.connection,
            ui: self.ui,
            log: self.log,
        }
    }
}

/// Owns the server tree and its on-disk representation.
///
/// Several processes (the GUI and the TUI) may share one config directory. Every write takes
/// an exclusive lock, re-reads the file if another instance changed it, applies the operation
/// on top of that state, and only then writes, so concurrent edits merge instead of clobbering.
#[derive(Debug)]
pub struct ConfigStore {
    paths: Paths,
    tree: ServerTree,
    settings: Settings,
    secrets: Arc<dyn SecretStore>,
    /// Bytes of `servers.toml` as last read or written (`None` = file did not exist).
    loaded: Option<Vec<u8>>,
    /// Same for `settings.toml`.
    settings_loaded: Option<Vec<u8>>,
}

impl ConfigStore {
    /// Loads the store. A missing `servers.toml` yields an empty tree and writes nothing.
    ///
    /// A corrupt or newer-versioned file is an error and is never touched; see [`Self::reset`].
    pub fn open(paths: Paths, secrets: Arc<dyn SecretStore>) -> Result<Self, ConfigError> {
        let loaded = fsio::read_optional(&paths.servers())?;
        let tree = parse_tree(&paths, loaded.as_deref())?;
        let settings_loaded = fsio::read_optional(&paths.settings())?;
        let settings = parse_settings(&paths, settings_loaded.as_deref())?;
        Ok(Self {
            paths,
            tree,
            settings,
            secrets,
            loaded,
            settings_loaded,
        })
    }

    pub fn tree(&self) -> &ServerTree {
        &self.tree
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn secrets(&self) -> &dyn SecretStore {
        &*self.secrets
    }

    /// Re-reads `servers.toml` and `settings.toml` if another instance changed them. On error
    /// the in-memory state is kept as it was.
    pub fn reload(&mut self) -> Result<(), ConfigError> {
        let current = fsio::read_optional(&self.paths.servers())?;
        let current_settings = fsio::read_optional(&self.paths.settings())?;
        let tree = (current != self.loaded)
            .then(|| parse_tree(&self.paths, current.as_deref()))
            .transpose()?;
        let settings = (current_settings != self.settings_loaded)
            .then(|| parse_settings(&self.paths, current_settings.as_deref()))
            .transpose()?;
        if let Some(tree) = tree {
            self.tree = tree;
            self.loaded = current;
        }
        if let Some(settings) = settings {
            self.settings = settings;
            self.settings_loaded = current_settings;
        }
        Ok(())
    }

    /// Changes the settings with `f`, validates them, and writes `settings.toml` atomically,
    /// on top of the latest on-disk state. On any error nothing is written and the in-memory
    /// settings are unchanged.
    pub fn update_settings(&mut self, f: impl FnOnce(&mut Settings)) -> Result<(), ConfigError> {
        let _lock = fsio::lock_exclusive(&self.paths.lock())?;
        self.reload()?;
        let mut next = self.settings.clone();
        f(&mut next);
        next.validate()?;
        let bytes = serialize_settings(&next)?.into_bytes();
        fsio::write_atomic(&self.paths.settings(), &bytes)?;
        self.settings = next;
        self.settings_loaded = Some(bytes);
        Ok(())
    }

    /// Applies `op` on top of the latest on-disk state: validates the resulting tree, then
    /// writes it atomically. On any error the file and the in-memory tree are unchanged.
    pub fn apply(&mut self, op: TreeOp) -> Result<NodeId, ConfigError> {
        let _lock = fsio::lock_exclusive(&self.paths.lock())?;
        self.reload()?;
        let previous = match &op {
            TreeOp::UpdateSite(site) => self.tree.site(site.id).cloned(),
            _ => None,
        };
        let mut next = self.tree.clone();
        let outcome = next.apply(op)?;
        next.validate()?;
        let bytes = serialize(&next)?.into_bytes();
        fsio::write_atomic(&self.paths.servers(), &bytes)?;
        self.tree = next;
        self.loaded = Some(bytes);
        self.sync_secrets(&outcome, previous.as_ref());
        Ok(outcome.node)
    }

    /// Keeps the keychain in step with a committed tree change. Runs after the file write, so a
    /// keychain failure is logged and never fails the operation: the file is the source of
    /// truth, and a leftover keychain entry is harmless.
    fn sync_secrets(&self, outcome: &Outcome, previous: Option<&Site>) {
        for &id in &outcome.removed_sites {
            self.forget(SecretKey::Password(id));
            self.forget(SecretKey::Passphrase(id));
        }
        if let (Some(from), NodeId::Site(to)) = (outcome.duplicated_from, outcome.node) {
            self.copy_secret(SecretKey::Password(from), SecretKey::Password(to));
            self.copy_secret(SecretKey::Passphrase(from), SecretKey::Passphrase(to));
        }
        if let (Some(old), NodeId::Site(id)) = (previous, outcome.node)
            && let Some(new) = self.tree.site(id)
        {
            if remembers_password(&old.auth) && !remembers_password(&new.auth) {
                self.forget(SecretKey::Password(id));
            }
            if remembers_passphrase(&old.auth) && !remembers_passphrase(&new.auth) {
                self.forget(SecretKey::Passphrase(id));
            }
        }
    }

    fn forget(&self, key: SecretKey) {
        if let Err(error) = self.secrets.delete(&key) {
            tracing::warn!(?key, %error, "could not delete keychain entry");
        }
    }

    fn copy_secret(&self, from: SecretKey, to: SecretKey) {
        match self.secrets.get(&from) {
            Ok(Some(value)) => {
                if let Err(error) = self.secrets.set(&to, &value) {
                    tracing::warn!(key = ?to, %error, "could not copy keychain entry");
                }
            }
            Ok(None) => {}
            Err(error) => tracing::warn!(key = ?from, %error, "could not read keychain entry"),
        }
    }

    /// Imports a FileZilla `sitemanager.xml` under a new root folder.
    ///
    /// The result is validated and written in a single atomic write; on any failure nothing is
    /// written and no secret is stored. Passwords go to the keychain only after the file write
    /// (a keychain failure is reported in [`ImportReport::passwords_skipped`], not an error).
    pub fn import_filezilla(
        &mut self,
        path: &Path,
        opts: ImportOptions,
    ) -> Result<ImportReport, ImportError> {
        let bytes = std::fs::read(path).map_err(|source| ImportError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let nodes = import::parse(path, &String::from_utf8_lossy(&bytes))?;

        let _lock = fsio::lock_exclusive(&self.paths.lock())?;
        self.reload()?;
        let mut plan = import::plan(&nodes, &opts, |name| {
            let wanted = name.trim().to_lowercase();
            self.tree
                .children(None)
                .iter()
                .any(|n| node_name(n).trim().to_lowercase() == wanted)
        });
        if plan.folders.is_empty() && plan.sites.is_empty() {
            return Ok(plan.report);
        }

        let next = self
            .tree
            .extended(plan.folders, plan.sites)
            .map_err(ConfigError::from)?;
        let tree_bytes = serialize(&next)?.into_bytes();
        fsio::write_atomic(&self.paths.servers(), &tree_bytes)?;
        self.tree = next;
        self.loaded = Some(tree_bytes);

        for (key, value, site) in plan.secrets {
            if let Err(error) = self.secrets.set(&key, &value) {
                tracing::warn!(?key, %error, "could not store imported password");
                plan.report
                    .passwords_skipped
                    .push(import::PasswordNotImported {
                        site,
                        reason: format!("keychain error: {error}"),
                    });
            }
        }
        Ok(plan.report)
    }

    /// Moves an unreadable `servers.toml` aside to `servers.toml.bak-<unix-seconds>` so a fresh
    /// store can be opened. Returns the backup path, or `None` when there was no file.
    pub fn reset(paths: &Paths) -> Result<Option<PathBuf>, ConfigError> {
        let _lock = fsio::lock_exclusive(&paths.lock())?;
        let path = paths.servers();
        if !path.exists() {
            return Ok(None);
        }
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let backup = (0..)
            .map(|n| match n {
                0 => path.with_extension(format!("toml.bak-{secs}")),
                n => path.with_extension(format!("toml.bak-{secs}-{n}")),
            })
            .find(|candidate| !candidate.exists())
            .unwrap_or_else(|| path.with_extension("toml.bak"));
        fsio::rename(&path, &backup)?;
        Ok(Some(backup))
    }
}

fn node_name<'a>(node: &crate::tree::Node<'a>) -> &'a str {
    match node {
        crate::tree::Node::Folder(f) => &f.name,
        crate::tree::Node::Site(s) => &s.name,
    }
}

fn remembers_password(auth: &Auth) -> bool {
    matches!(auth, Auth::Password { remember: true })
}

fn remembers_passphrase(auth: &Auth) -> bool {
    matches!(
        auth,
        Auth::KeyFile {
            remember_passphrase: true,
            ..
        }
    )
}

fn parse_tree(paths: &Paths, bytes: Option<&[u8]>) -> Result<ServerTree, ConfigError> {
    let path = paths.servers();
    let Some(bytes) = bytes else {
        return Ok(ServerTree::default());
    };
    let file: ServersFile = parse_toml(&path, bytes)?;
    if file.version > FILE_VERSION {
        return Err(ConfigError::UnsupportedVersion {
            path,
            found: file.version,
            supported: FILE_VERSION,
        });
    }
    Ok(ServerTree::from_parts(file.folders, file.sites)?)
}

fn parse_settings(paths: &Paths, bytes: Option<&[u8]>) -> Result<Settings, ConfigError> {
    let path = paths.settings();
    let Some(bytes) = bytes else {
        return Ok(Settings::default());
    };
    let file: SettingsFile = parse_toml(&path, bytes)?;
    if file.version > SETTINGS_VERSION {
        return Err(ConfigError::UnsupportedVersion {
            path,
            found: file.version,
            supported: SETTINGS_VERSION,
        });
    }
    let settings = file.into_settings();
    settings.validate()?;
    Ok(settings)
}

fn parse_toml<T: serde::de::DeserializeOwned>(
    path: &std::path::Path,
    bytes: &[u8],
) -> Result<T, ConfigError> {
    let text = String::from_utf8_lossy(bytes);
    toml::from_str(&text).map_err(|e| {
        let line = e
            .span()
            .map(|s| text[..s.start.min(text.len())].matches('\n').count() + 1)
            .unwrap_or(1);
        ConfigError::Parse {
            path: path.to_path_buf(),
            line,
            msg: e.message().to_string(),
        }
    })
}

fn serialize_settings(settings: &Settings) -> Result<String, ConfigError> {
    toml::to_string(&SettingsFile::new(settings.clone()))
        .map_err(|e| ConfigError::Serialize(e.to_string()))
}

fn serialize(tree: &ServerTree) -> Result<String, ConfigError> {
    let file = ServersFile {
        version: FILE_VERSION,
        folders: tree.folders().to_vec(),
        sites: tree.sites().to_vec(),
    };
    toml::to_string(&file).map_err(|e| ConfigError::Serialize(e.to_string()))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;
    use crate::error::ValidationError;
    use crate::model::{Auth, Protocol};

    fn open(paths: Paths) -> Result<ConfigStore, ConfigError> {
        ConfigStore::open(paths, Arc::new(crate::secrets::MemoryStore::new()))
    }

    fn paths(dir: &tempfile::TempDir) -> Paths {
        Paths::from_override(Some(dir.path().join("cfg")))
    }

    fn site(name: &str) -> Site {
        Site::new(name, Protocol::Sftp, "example.com")
    }

    #[test]
    fn fresh_dir_opens_empty_and_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(paths(&dir)).unwrap();
        assert!(store.tree().sites().is_empty());
        assert!(!dir.path().join("cfg").exists());
    }

    #[test]
    fn added_site_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = site("prod");
        s.user = "deploy".into();
        s.port = Some(2222);
        s.auth = Auth::KeyFile {
            path: PathBuf::from("~/.ssh/id_ed25519"),
            remember_passphrase: true,
        };
        s.remote_dir = Some("/var/www".into());
        let mut store = open(paths(&dir)).unwrap();
        store.apply(TreeOp::AddSite(s.clone())).unwrap();

        let reopened = open(paths(&dir)).unwrap();
        assert_eq!(reopened.tree().site(s.id), Some(&s));
    }

    #[test]
    fn update_site_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(paths(&dir)).unwrap();
        let mut s = site("prod");
        store.apply(TreeOp::AddSite(s.clone())).unwrap();
        s.host = "other.example.com".into();
        store.apply(TreeOp::UpdateSite(s.clone())).unwrap();

        let reopened = open(paths(&dir)).unwrap();
        assert_eq!(
            reopened.tree().site(s.id).unwrap().host,
            "other.example.com"
        );
    }

    #[test]
    fn serialized_file_matches_golden() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = site("prod-web");
        s.id = site_id("7c1e0000-0000-4000-8000-000000000001");
        s.user = "deploy".into();
        s.auth = Auth::KeyFile {
            path: PathBuf::from("/k"),
            remember_passphrase: true,
        };
        s.remote_dir = Some("/var/www".into());
        let mut store = open(paths(&dir)).unwrap();
        store.apply(TreeOp::AddSite(s)).unwrap();

        let text = fs::read_to_string(dir.path().join("cfg/servers.toml")).unwrap();
        assert_eq!(
            text,
            "version = 1\n\n[[site]]\nid = \"7c1e0000-0000-4000-8000-000000000001\"\nname = \"prod-web\"\nprotocol = \"sftp\"\nhost = \"example.com\"\nuser = \"deploy\"\nremote_dir = \"/var/www\"\n\n[site.auth]\nmethod = \"key_file\"\npath = \"/k\"\nremember_passphrase = true\n"
        );
    }

    fn site_id(s: &str) -> crate::model::SiteId {
        toml::Value::String(s.into()).try_into().unwrap()
    }

    #[test]
    fn invalid_sites_are_rejected_and_nothing_is_written() {
        let cases: Vec<(&str, Site)> = vec![
            ("empty name", site("  ")),
            ("empty host", Site::new("a", Protocol::Sftp, " ")),
            ("port 0", {
                let mut s = site("a");
                s.port = Some(0);
                s
            }),
            ("key file on ftp", {
                let mut s = Site::new("a", Protocol::Ftp, "h");
                s.auth = Auth::KeyFile {
                    path: "k".into(),
                    remember_passphrase: false,
                };
                s
            }),
            ("agent on ftps", {
                let mut s = Site::new("a", Protocol::FtpsExplicit, "h");
                s.auth = Auth::Agent;
                s
            }),
            ("anonymous on sftp", {
                let mut s = site("a");
                s.auth = Auth::Anonymous;
                s
            }),
        ];
        for (label, bad) in cases {
            let dir = tempfile::tempdir().unwrap();
            let mut store = open(paths(&dir)).unwrap();
            let err = store.apply(TreeOp::AddSite(bad)).unwrap_err();
            assert!(matches!(err, ConfigError::Invalid(_)), "{label}: {err}");
            assert!(!dir.path().join("cfg/servers.toml").exists(), "{label}");
            assert!(store.tree().sites().is_empty(), "{label}");
        }
    }

    #[test]
    fn rejected_update_leaves_file_bytes_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(paths(&dir)).unwrap();
        let mut s = site("a");
        store.apply(TreeOp::AddSite(s.clone())).unwrap();
        let before = fs::read(dir.path().join("cfg/servers.toml")).unwrap();

        s.port = Some(0);
        let err = store.apply(TreeOp::UpdateSite(s)).unwrap_err();
        assert!(matches!(
            err,
            ConfigError::Invalid(ValidationError::InvalidPort { .. })
        ));
        assert_eq!(
            fs::read(dir.path().join("cfg/servers.toml")).unwrap(),
            before
        );
    }

    #[test]
    fn update_of_unknown_site_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(paths(&dir)).unwrap();
        let err = store.apply(TreeOp::UpdateSite(site("ghost"))).unwrap_err();
        assert!(matches!(
            err,
            ConfigError::Invalid(ValidationError::UnknownNode)
        ));
    }

    #[test]
    fn duplicate_sibling_names_are_rejected_case_insensitively() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(paths(&dir)).unwrap();
        store.apply(TreeOp::AddSite(site("Prod"))).unwrap();
        let err = store.apply(TreeOp::AddSite(site("prod"))).unwrap_err();
        assert!(matches!(
            err,
            ConfigError::Invalid(ValidationError::DuplicateName { .. })
        ));
    }
}
