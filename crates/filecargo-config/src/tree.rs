use std::collections::HashSet;

use crate::error::ValidationError;
use crate::model::{Auth, Folder, FolderId, NodeId, Protocol, Site, SiteId};

/// A mutation of the server tree. Applied by `ConfigStore::apply`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeOp {
    AddFolder {
        name: String,
        parent: Option<FolderId>,
    },
    AddSite(Site),
    UpdateSite(Site),
    Rename {
        node: NodeId,
        name: String,
    },
    Move {
        node: NodeId,
        parent: Option<FolderId>,
    },
    /// Copies a site under a new id and name `"<name> (copy)"`, `"(copy 2)"`, ...
    Duplicate {
        site: SiteId,
    },
    /// Deleting a folder is recursive.
    Delete {
        node: NodeId,
    },
}

/// What an applied op did beyond returning the affected node; used for secret bookkeeping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Outcome {
    pub node: NodeId,
    pub removed_sites: Vec<SiteId>,
    /// For `Duplicate`: the site that was copied (`node` is the copy).
    pub duplicated_from: Option<SiteId>,
}

impl Outcome {
    fn node(node: NodeId) -> Self {
        Self {
            node,
            removed_sites: Vec::new(),
            duplicated_from: None,
        }
    }
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

    /// A copy of this tree with `folders` and `sites` added, validated as a whole.
    pub(crate) fn extended(
        &self,
        folders: Vec<Folder>,
        sites: Vec<Site>,
    ) -> Result<Self, ValidationError> {
        let mut next = self.clone();
        next.folders.extend(folders);
        next.sites.extend(sites);
        next.validate()?;
        Ok(next)
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
    pub(crate) fn apply(&mut self, op: TreeOp) -> Result<Outcome, ValidationError> {
        match op {
            TreeOp::AddFolder { name, parent } => {
                let folder = Folder {
                    id: FolderId::new(),
                    name,
                    parent,
                };
                let id = folder.id;
                self.folders.push(folder);
                Ok(Outcome::node(NodeId::Folder(id)))
            }
            TreeOp::AddSite(site) => {
                let id = site.id;
                self.sites.push(site);
                Ok(Outcome::node(NodeId::Site(id)))
            }
            TreeOp::UpdateSite(site) => {
                let id = site.id;
                let slot = self
                    .sites
                    .iter_mut()
                    .find(|s| s.id == id)
                    .ok_or(ValidationError::UnknownNode)?;
                *slot = site;
                Ok(Outcome::node(NodeId::Site(id)))
            }
            TreeOp::Rename { node, name } => {
                match node {
                    NodeId::Site(id) => self.site_mut(id)?.name = name,
                    NodeId::Folder(id) => self.folder_mut(id)?.name = name,
                }
                Ok(Outcome::node(node))
            }
            TreeOp::Move { node, parent } => {
                match node {
                    NodeId::Site(id) => self.site_mut(id)?.folder = parent,
                    NodeId::Folder(id) => self.folder_mut(id)?.parent = parent,
                }
                Ok(Outcome::node(node))
            }
            TreeOp::Duplicate { site } => {
                let original = self.site(site).ok_or(ValidationError::UnknownNode)?;
                let mut copy = original.clone();
                copy.id = SiteId::new();
                copy.name = self.copy_name(original);
                let id = copy.id;
                self.sites.push(copy);
                Ok(Outcome {
                    node: NodeId::Site(id),
                    removed_sites: Vec::new(),
                    duplicated_from: Some(site),
                })
            }
            TreeOp::Delete { node } => match node {
                NodeId::Site(id) => {
                    let before = self.sites.len();
                    self.sites.retain(|s| s.id != id);
                    if self.sites.len() == before {
                        return Err(ValidationError::UnknownNode);
                    }
                    Ok(Outcome {
                        node,
                        removed_sites: vec![id],
                        duplicated_from: None,
                    })
                }
                NodeId::Folder(id) => {
                    if self.folder(id).is_none() {
                        return Err(ValidationError::UnknownNode);
                    }
                    let doomed = self.descendant_folders(id);
                    let removed_sites = self
                        .sites
                        .iter()
                        .filter(|s| s.folder.is_some_and(|f| doomed.contains(&f)))
                        .map(|s| s.id)
                        .collect();
                    self.folders.retain(|f| !doomed.contains(&f.id));
                    self.sites
                        .retain(|s| !s.folder.is_some_and(|f| doomed.contains(&f)));
                    Ok(Outcome {
                        node,
                        removed_sites,
                        duplicated_from: None,
                    })
                }
            },
        }
    }

    fn site_mut(&mut self, id: SiteId) -> Result<&mut Site, ValidationError> {
        self.sites
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or(ValidationError::UnknownNode)
    }

    fn folder_mut(&mut self, id: FolderId) -> Result<&mut Folder, ValidationError> {
        self.folders
            .iter_mut()
            .find(|f| f.id == id)
            .ok_or(ValidationError::UnknownNode)
    }

    /// `root` and every folder below it.
    fn descendant_folders(&self, root: FolderId) -> HashSet<FolderId> {
        let mut found = HashSet::from([root]);
        loop {
            let before = found.len();
            for f in &self.folders {
                if f.parent.is_some_and(|p| found.contains(&p)) {
                    found.insert(f.id);
                }
            }
            if found.len() == before {
                return found;
            }
        }
    }

    /// First free `"<name> (copy)"`, `"<name> (copy 2)"`, ... among `site`'s siblings.
    fn copy_name(&self, site: &Site) -> String {
        let taken = |candidate: &str| {
            let key = candidate.trim().to_lowercase();
            self.folders
                .iter()
                .filter(|f| f.parent == site.folder)
                .map(|f| &f.name)
                .chain(
                    self.sites
                        .iter()
                        .filter(|s| s.folder == site.folder)
                        .map(|s| &s.name),
                )
                .any(|n| n.trim().to_lowercase() == key)
        };
        let first = format!("{} (copy)", site.name);
        if !taken(&first) {
            return first;
        }
        (2..)
            .map(|n| format!("{} (copy {n})", site.name))
            .find(|c| !taken(c))
            .unwrap_or(first)
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
