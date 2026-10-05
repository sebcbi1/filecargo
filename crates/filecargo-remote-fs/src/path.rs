use std::fmt;

/// Absolute, `/`-separated, UTF-8, normalized path on a remote (or rooted) filesystem:
/// no `.` / `..` segments, no duplicate or trailing `/` (except the root itself).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RemotePath(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
    #[error("path {0:?} is not absolute")]
    NotAbsolute(String),
    #[error("path {0:?} escapes the root")]
    EscapesRoot(String),
    #[error("invalid path name {0:?}")]
    InvalidName(String),
    #[error("path contains a NUL byte")]
    Nul,
}

impl RemotePath {
    pub fn root() -> Self {
        Self("/".to_owned())
    }

    /// Parses and normalizes an absolute path. `a//b`, `/a/./b` and `/a/b/` collapse; `/a/../b`
    /// resolves to `/b`; a `..` that would climb above `/` is an error.
    pub fn parse(s: &str) -> Result<Self, PathError> {
        if s.contains('\0') {
            return Err(PathError::Nul);
        }
        if !s.starts_with('/') {
            return Err(PathError::NotAbsolute(s.to_owned()));
        }
        let mut segments: Vec<&str> = Vec::new();
        for segment in s.split('/') {
            match segment {
                "" | "." => {}
                ".." => {
                    if segments.pop().is_none() {
                        return Err(PathError::EscapesRoot(s.to_owned()));
                    }
                }
                name => segments.push(name),
            }
        }
        if segments.is_empty() {
            return Ok(Self::root());
        }
        Ok(Self(format!("/{}", segments.join("/"))))
    }

    /// Appends one path component. `name` must not contain `/`, be `.`, `..` or empty.
    pub fn join(&self, name: &str) -> Result<Self, PathError> {
        if name.is_empty() || name == "." || name == ".." || name.contains('/') {
            return Err(PathError::InvalidName(name.to_owned()));
        }
        if name.contains('\0') {
            return Err(PathError::Nul);
        }
        if self.0 == "/" {
            Ok(Self(format!("/{name}")))
        } else {
            Ok(Self(format!("{}/{name}", self.0)))
        }
    }

    pub fn parent(&self) -> Option<Self> {
        if self.0 == "/" {
            return None;
        }
        match self.0.rfind('/') {
            Some(0) => Some(Self::root()),
            Some(i) => Some(Self(self.0[..i].to_owned())),
            None => None,
        }
    }

    pub fn file_name(&self) -> Option<&str> {
        if self.0 == "/" {
            return None;
        }
        self.0.rsplit('/').next()
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RemotePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn parse_rejects_relative_paths() {
        assert_eq!(
            RemotePath::parse("a/b"),
            Err(PathError::NotAbsolute("a/b".into()))
        );
        assert!(RemotePath::parse("").is_err());
    }

    #[test]
    fn parse_rejects_dotdot_escaping_the_root() {
        assert!(matches!(
            RemotePath::parse("/.."),
            Err(PathError::EscapesRoot(_))
        ));
        assert!(matches!(
            RemotePath::parse("/a/../../b"),
            Err(PathError::EscapesRoot(_))
        ));
    }

    #[test]
    fn parse_normalizes() {
        let p = RemotePath::parse("//a/./b//c/../d/").unwrap();
        assert_eq!(p.as_str(), "/a/b/d");
        assert_eq!(RemotePath::parse("///").unwrap(), RemotePath::root());
    }

    #[test]
    fn join_rejects_bad_names() {
        let root = RemotePath::root();
        for bad in ["", ".", "..", "a/b", "/a"] {
            assert!(root.join(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn join_parent_and_file_name() {
        let p = RemotePath::root().join("a").unwrap().join("b c").unwrap();
        assert_eq!(p.as_str(), "/a/b c");
        assert_eq!(p.file_name(), Some("b c"));
        assert_eq!(p.parent().unwrap().as_str(), "/a");
        assert_eq!(p.parent().unwrap().parent().unwrap(), RemotePath::root());
        assert_eq!(RemotePath::root().parent(), None);
        assert_eq!(RemotePath::root().file_name(), None);
    }

    proptest! {
        #[test]
        fn parse_of_as_str_round_trips(raw in "(/{0,2}[a-z. ]{0,4}){0,6}") {
            if let Ok(p) = RemotePath::parse(&format!("/{raw}")) {
                prop_assert_eq!(RemotePath::parse(p.as_str()).unwrap(), p);
            }
        }

        #[test]
        fn joined_children_round_trip(names in proptest::collection::vec("[a-z0-9 _-]{1,6}", 0..6)) {
            let mut p = RemotePath::root();
            for n in &names {
                p = p.join(n).unwrap();
            }
            prop_assert_eq!(RemotePath::parse(p.as_str()).unwrap(), p.clone());
            for n in names.iter().rev() {
                prop_assert_eq!(p.file_name(), Some(n.as_str()));
                p = p.parent().unwrap();
            }
            prop_assert_eq!(p, RemotePath::root());
        }
    }
}
