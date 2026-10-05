use serde::{Deserialize, Serialize};

use crate::error::ConfigError;
use crate::fsio;
use crate::model::{Folder, NodeId, Site};
use crate::paths::Paths;
use crate::tree::{ServerTree, TreeOp};

const FILE_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct ServersFile {
    version: u32,
    #[serde(default, rename = "folder", skip_serializing_if = "Vec::is_empty")]
    folders: Vec<Folder>,
    #[serde(default, rename = "site", skip_serializing_if = "Vec::is_empty")]
    sites: Vec<Site>,
}

/// Owns the server tree and its on-disk representation.
#[derive(Debug)]
pub struct ConfigStore {
    paths: Paths,
    tree: ServerTree,
}

impl ConfigStore {
    /// Loads the store. A missing `servers.toml` yields an empty tree and writes nothing.
    pub fn open(paths: Paths) -> Result<Self, ConfigError> {
        let tree = load_tree(&paths)?;
        Ok(Self { paths, tree })
    }

    pub fn tree(&self) -> &ServerTree {
        &self.tree
    }

    /// Applies `op`: validates the resulting tree, then writes it atomically. On any error the
    /// file and the in-memory tree are unchanged.
    pub fn apply(&mut self, op: TreeOp) -> Result<NodeId, ConfigError> {
        let mut next = self.tree.clone();
        let outcome = next.apply(op)?;
        next.validate()?;
        fsio::write_atomic(&self.paths.servers(), serialize(&next)?.as_bytes())?;
        self.tree = next;
        Ok(outcome.node)
    }
}

fn load_tree(paths: &Paths) -> Result<ServerTree, ConfigError> {
    let path = paths.servers();
    let Some(bytes) = fsio::read_optional(&path)? else {
        return Ok(ServerTree::default());
    };
    let text = String::from_utf8_lossy(&bytes);
    let file: ServersFile = toml::from_str(&text).map_err(|e| {
        let line = e
            .span()
            .map(|s| text[..s.start.min(text.len())].matches('\n').count() + 1)
            .unwrap_or(1);
        ConfigError::Parse {
            path: path.clone(),
            line,
            msg: e.message().to_string(),
        }
    })?;
    Ok(ServerTree::from_parts(file.folders, file.sites)?)
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

    fn paths(dir: &tempfile::TempDir) -> Paths {
        Paths::from_override(Some(dir.path().join("cfg")))
    }

    fn site(name: &str) -> Site {
        Site::new(name, Protocol::Sftp, "example.com")
    }

    #[test]
    fn fresh_dir_opens_empty_and_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConfigStore::open(paths(&dir)).unwrap();
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
        let mut store = ConfigStore::open(paths(&dir)).unwrap();
        store.apply(TreeOp::AddSite(s.clone())).unwrap();

        let reopened = ConfigStore::open(paths(&dir)).unwrap();
        assert_eq!(reopened.tree().site(s.id), Some(&s));
    }

    #[test]
    fn update_site_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = ConfigStore::open(paths(&dir)).unwrap();
        let mut s = site("prod");
        store.apply(TreeOp::AddSite(s.clone())).unwrap();
        s.host = "other.example.com".into();
        store.apply(TreeOp::UpdateSite(s.clone())).unwrap();

        let reopened = ConfigStore::open(paths(&dir)).unwrap();
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
        let mut store = ConfigStore::open(paths(&dir)).unwrap();
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
            let mut store = ConfigStore::open(paths(&dir)).unwrap();
            let err = store.apply(TreeOp::AddSite(bad)).unwrap_err();
            assert!(matches!(err, ConfigError::Invalid(_)), "{label}: {err}");
            assert!(!dir.path().join("cfg/servers.toml").exists(), "{label}");
            assert!(store.tree().sites().is_empty(), "{label}");
        }
    }

    #[test]
    fn rejected_update_leaves_file_bytes_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = ConfigStore::open(paths(&dir)).unwrap();
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
        let mut store = ConfigStore::open(paths(&dir)).unwrap();
        let err = store.apply(TreeOp::UpdateSite(site("ghost"))).unwrap_err();
        assert!(matches!(
            err,
            ConfigError::Invalid(ValidationError::UnknownNode)
        ));
    }

    #[test]
    fn duplicate_sibling_names_are_rejected_case_insensitively() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = ConfigStore::open(paths(&dir)).unwrap();
        store.apply(TreeOp::AddSite(site("Prod"))).unwrap();
        let err = store.apply(TreeOp::AddSite(site("prod"))).unwrap_err();
        assert!(matches!(
            err,
            ConfigError::Invalid(ValidationError::DuplicateName { .. })
        ));
    }
}
