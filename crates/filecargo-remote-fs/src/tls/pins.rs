//! `<data>/trusted_certs.toml`: leaf certificates the user chose to trust permanently, per
//! `host:port`, by SHA-256 fingerprint of the DER.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum PinError {
    #[error("cannot read {}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{} is not valid: {message}. Fix or remove it; filecargo will not overwrite it.", path.display())]
    Parse { path: PathBuf, message: String },
}

#[derive(Debug, Serialize, Deserialize)]
struct PinFile {
    #[serde(default = "version_one")]
    version: u32,
    #[serde(default)]
    cert: Vec<Pin>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Pin {
    host: String,
    port: u16,
    /// Lower-case hex SHA-256 of the DER certificate.
    sha256: String,
}

fn version_one() -> u32 {
    1
}

fn read(path: &Path) -> Result<PinFile, PinError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(PinFile {
                version: 1,
                cert: Vec::new(),
            });
        }
        Err(source) => {
            return Err(PinError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    let file: PinFile = toml::from_str(&text).map_err(|e| PinError::Parse {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;
    if file.version != 1 {
        return Err(PinError::Parse {
            path: path.to_path_buf(),
            message: format!("unsupported version {}", file.version),
        });
    }
    Ok(file)
}

/// Fingerprints pinned for `host:port`. A missing file means none.
pub(crate) fn pinned(path: &Path, host: &str, port: u16) -> Result<Vec<String>, PinError> {
    Ok(read(path)?
        .cert
        .into_iter()
        .filter(|p| p.host.eq_ignore_ascii_case(host) && p.port == port)
        .map(|p| p.sha256)
        .collect())
}

/// Adds a pin (idempotent). An existing file that does not parse is left untouched.
pub(crate) fn add(path: &Path, host: &str, port: u16, sha256: &str) -> Result<(), PinError> {
    let mut file = read(path)?;
    let pin = Pin {
        host: host.to_owned(),
        port,
        sha256: sha256.to_owned(),
    };
    if file.cert.contains(&pin) {
        return Ok(());
    }
    file.cert.push(pin);
    let text = toml::to_string_pretty(&file).map_err(|e| PinError::Parse {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;
    let io = |source| PinError::Io {
        path: path.to_path_buf(),
        source,
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    let temp = path.with_extension("toml.tmp");
    std::fs::write(&temp, text).map_err(io)?;
    std::fs::rename(&temp, path).map_err(io)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_has_no_pins_and_add_creates_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("trusted_certs.toml");
        assert_eq!(pinned(&path, "h", 21).unwrap(), Vec::<String>::new());
        add(&path, "h", 21, "aa").unwrap();
        assert_eq!(pinned(&path, "h", 21).unwrap(), ["aa"]);
    }

    #[test]
    fn pins_are_per_host_and_port_and_adds_are_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.toml");
        add(&path, "Example.org", 990, "aa").unwrap();
        add(&path, "Example.org", 990, "aa").unwrap();
        add(&path, "Example.org", 990, "bb").unwrap();
        add(&path, "other", 990, "cc").unwrap();
        assert_eq!(pinned(&path, "example.org", 990).unwrap(), ["aa", "bb"]);
        assert_eq!(
            pinned(&path, "example.org", 21).unwrap(),
            Vec::<String>::new()
        );
        assert_eq!(pinned(&path, "other", 990).unwrap(), ["cc"]);
    }

    #[test]
    fn a_corrupt_file_is_an_error_and_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.toml");
        std::fs::write(&path, "this is = not [valid").unwrap();
        assert!(matches!(pinned(&path, "h", 1), Err(PinError::Parse { .. })));
        assert!(matches!(
            add(&path, "h", 1, "aa"),
            Err(PinError::Parse { .. })
        ));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "this is = not [valid"
        );
    }

    #[test]
    fn a_newer_version_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.toml");
        std::fs::write(&path, "version = 2\n").unwrap();
        assert!(matches!(pinned(&path, "h", 1), Err(PinError::Parse { .. })));
    }
}
