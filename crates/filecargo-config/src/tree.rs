use std::collections::HashSet;

use crate::error::ValidationError;
use crate::model::{Auth, Folder, FolderId, NodeId, Protocol, Site, SiteId};

/// A mutation of the server tree. Applied by `ConfigStore::apply`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeOp {
    AddSite(Site),
    UpdateSite(Site),
}

/// A child of a folder, as returned by [`ServerTree::children`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Node<'a> {
    Folder(&'a Folder),
    Site(&'a Site),
}

/// Folders and sites. Display order is derived (folders first, then alphabetical), not stored.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServerTree {
    folders: Vec<Folder>,
    sites: Vec<Site>,
}

impl ServerTree {
    pub(crate) fn from_parts(
        folders: Vec<Folder>,
        sites: Vec<Site>,
    ) -> Result<Self, ValidationError> {
        let tree = Self { folders, sites };
        tree.validate()?;
        Ok(tree)
    }

    pub fn folders(&self) -> &[Folder] {
        &self.folders
    }

    pub fn sites(&self) -> &[Site] {
        &self.sites
    }

    pub fn site(&self, id: SiteId) -> Option<&Site> {
        self.sites.iter().find(|s| s.id == id)
    }

    pub fn folder(&self, id: FolderId) -> Option<&Folder> {
        self.folders.iter().find(|f| f.id == id)
    }

    /// Children of `parent` (`None` = root): folders first, then sites, each sorted
    /// case-insensitively by name.
    pub fn children(&self, parent: Option<FolderId>) -> Vec<Node<'_>> {
        let mut folders: Vec<&Folder> =
            self.folders.iter().filter(|f| f.parent == parent).collect();
        let mut sites: Vec<&Site> = self.sites.iter().filter(|s| s.folder == parent).collect();
        folders.sort_by_key(|f| f.name.to_lowercase());
        sites.sort_by_key(|s| s.name.to_lowercase());
        folders
            .into_iter()
            .map(Node::Folder)
            .chain(sites.into_iter().map(Node::Site))
            .collect()
    }

    /// Applies `op` in place. The caller is expected to work on a clone and call
    /// [`validate`](Self::validate) before committing.
    pub(crate) fn apply(&mut self, op: TreeOp) -> Result<NodeId, ValidationError> {
        match op {
            TreeOp::AddSite(site) => {
                let id = site.id;
                self.sites.push(site);
                Ok(NodeId::Site(id))
            }
            TreeOp::UpdateSite(site) => {
                let id = site.id;
                let slot = self
                    .sites
                    .iter_mut()
                    .find(|s| s.id == id)
                    .ok_or(ValidationError::UnknownNode)?;
                *slot = site;
                Ok(NodeId::Site(id))
            }
        }
    }

    /// Checks every rule of the spec against the whole tree.
    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        let mut folder_ids = HashSet::new();
        for f in &self.folders {
            if !folder_ids.insert(f.id) {
                return Err(ValidationError::DuplicateId(f.id.to_string()));
            }
            if f.name.trim().is_empty() {
                return Err(ValidationError::EmptyName);
            }
        }
        let mut site_ids = HashSet::new();
        for s in &self.sites {
            if !site_ids.insert(s.id) {
                return Err(ValidationError::DuplicateId(s.id.to_string()));
            }
            validate_site(s)?;
        }

        for f in &self.folders {
            if let Some(p) = f.parent
                && !folder_ids.contains(&p)
            {
                return Err(ValidationError::UnknownFolder(p));
            }
        }
        for s in &self.sites {
            if let Some(p) = s.folder
                && !folder_ids.contains(&p)
            {
                return Err(ValidationError::UnknownFolder(p));
            }
        }

        for f in &self.folders {
            let mut cursor = f.parent;
            let mut steps = 0;
            while let Some(id) = cursor {
                if id == f.id || steps > self.folders.len() {
                    return Err(ValidationError::MoveIntoDescendant);
                }
                cursor = self.folder(id).and_then(|p| p.parent);
                steps += 1;
            }
        }

        let mut seen = HashSet::new();
        let siblings = self
            .folders
            .iter()
            .map(|f| (f.parent, &f.name))
            .chain(self.sites.iter().map(|s| (s.folder, &s.name)));
        for (parent, name) in siblings {
            if !seen.insert((parent, name.trim().to_lowercase())) {
                return Err(ValidationError::DuplicateName { name: name.clone() });
            }
        }
        Ok(())
    }
}

fn validate_site(site: &Site) -> Result<(), ValidationError> {
    if site.name.trim().is_empty() {
        return Err(ValidationError::EmptyName);
    }
    if site.host.trim().is_empty() {
        return Err(ValidationError::EmptyHost {
            site: site.name.clone(),
        });
    }
    if site.port == Some(0) {
        return Err(ValidationError::InvalidPort {
            site: site.name.clone(),
        });
    }
    let (auth_ok, auth_name) = match (&site.auth, site.protocol) {
        (Auth::Anonymous, p) => (p.is_ftp_family(), "anonymous"),
        (Auth::KeyFile { .. }, p) => (p == Protocol::Sftp, "key file"),
        (Auth::Agent, p) => (p == Protocol::Sftp, "ssh-agent"),
        (Auth::Password { .. }, _) => (true, "password"),
    };
    if !auth_ok {
        return Err(ValidationError::AuthProtocolMismatch {
            site: site.name.clone(),
            auth: auth_name,
            protocol: protocol_name(site.protocol),
        });
    }
    Ok(())
}

fn protocol_name(p: Protocol) -> &'static str {
    match p {
        Protocol::Sftp => "SFTP",
        Protocol::Ftp => "FTP",
        Protocol::FtpsExplicit => "explicit FTPS",
        Protocol::FtpsImplicit => "implicit FTPS",
    }
}
