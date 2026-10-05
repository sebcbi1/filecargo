use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use filecargo_config::{ExposeSecret, SecretKey, SecretString};

/// In-memory trust shared by every connection of the app and lost on exit: credentials typed
/// during this run, and host keys accepted with *Trust once*. Transfer workers open their own
/// connections, and this cache keeps them from prompting the user a second time.
#[derive(Default)]
pub struct SessionTrust {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    credentials: HashMap<SecretKey, SecretString>,
    host_keys: HashSet<(String, u16, String)>,
    certs: HashSet<(String, u16, String)>,
}

impl std::fmt::Debug for SessionTrust {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inner = self.lock();
        f.debug_struct("SessionTrust")
            .field("credentials", &inner.credentials.len())
            .field("host_keys", &inner.host_keys.len())
            .finish()
    }
}

impl SessionTrust {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // A poisoned lock only means another thread panicked mid-insert; the maps stay valid.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn credential(&self, key: &SecretKey) -> Option<SecretString> {
        self.lock()
            .credentials
            .get(key)
            .map(|s| SecretString::from(s.expose_secret().to_owned()))
    }

    pub fn remember_credential(&self, key: SecretKey, value: &SecretString) {
        let copy = SecretString::from(value.expose_secret().to_owned());
        self.lock().credentials.insert(key, copy);
    }

    pub fn forget_credential(&self, key: &SecretKey) {
        self.lock().credentials.remove(key);
    }

    pub fn host_key_trusted(&self, host: &str, port: u16, fingerprint: &str) -> bool {
        self.lock()
            .host_keys
            .contains(&(host.to_owned(), port, fingerprint.to_owned()))
    }

    pub fn trust_host_key(&self, host: &str, port: u16, fingerprint: &str) {
        self.lock()
            .host_keys
            .insert((host.to_owned(), port, fingerprint.to_owned()));
    }

    /// FTPS leaf certificates (hex SHA-256) accepted with *Trust once* for `host:port`.
    pub fn trusted_certs(&self, host: &str, port: u16) -> Vec<String> {
        self.lock()
            .certs
            .iter()
            .filter(|(h, p, _)| h == host && *p == port)
            .map(|(_, _, sha)| sha.clone())
            .collect()
    }

    pub fn trust_cert(&self, host: &str, port: u16, sha256: &str) {
        self.lock()
            .certs
            .insert((host.to_owned(), port, sha256.to_owned()));
    }
}

#[cfg(test)]
mod tests {
    use filecargo_config::SiteId;

    use super::*;

    #[test]
    fn credentials_round_trip_and_debug_hides_them() {
        let trust = SessionTrust::new();
        let key = SecretKey::Password(SiteId::new());
        assert!(trust.credential(&key).is_none());
        trust.remember_credential(key, &SecretString::from("hunter2".to_owned()));
        assert_eq!(trust.credential(&key).unwrap().expose_secret(), "hunter2");
        assert!(!format!("{trust:?}").contains("hunter2"));
        trust.forget_credential(&key);
        assert!(trust.credential(&key).is_none());
    }

    #[test]
    fn host_keys_are_per_host_port_and_fingerprint() {
        let trust = SessionTrust::new();
        trust.trust_host_key("h", 22, "SHA256:a");
        assert!(trust.host_key_trusted("h", 22, "SHA256:a"));
        assert!(!trust.host_key_trusted("h", 2222, "SHA256:a"));
        assert!(!trust.host_key_trusted("h", 22, "SHA256:b"));
    }
}
