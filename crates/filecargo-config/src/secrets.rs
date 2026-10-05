use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

use keyring_core::CredentialStore;
pub use secrecy::{ExposeSecret, SecretString};

use crate::model::SiteId;

/// Keychain service name used by the real application.
pub const SERVICE: &str = "filecargo";

/// Which secret of a site is addressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SecretKey {
    Password(SiteId),
    Passphrase(SiteId),
}

impl SecretKey {
    pub fn site(self) -> SiteId {
        match self {
            Self::Password(id) | Self::Passphrase(id) => id,
        }
    }

    /// The same kind of secret, for another site.
    pub fn for_site(self, id: SiteId) -> Self {
        match self {
            Self::Password(_) => Self::Password(id),
            Self::Passphrase(_) => Self::Passphrase(id),
        }
    }

    /// Keychain account name, e.g. `site:<uuid>:password`.
    pub fn account(self) -> String {
        match self {
            Self::Password(id) => format!("site:{id}:password"),
            Self::Passphrase(id) => format!("site:{id}:passphrase"),
        }
    }
}

/// Secret-store failures. Messages never contain secret material.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SecretError {
    /// No usable keychain (e.g. headless Linux without a Secret Service).
    #[error("keychain unavailable: {0}")]
    Unavailable(String),
    #[error("keychain error: {0}")]
    Backend(String),
}

/// Storage for passwords and key passphrases. Secrets never reach any config file.
pub trait SecretStore: Send + Sync + fmt::Debug {
    fn get(&self, key: &SecretKey) -> Result<Option<SecretString>, SecretError>;
    fn set(&self, key: &SecretKey, value: &SecretString) -> Result<(), SecretError>;
    /// Deleting a secret that does not exist succeeds.
    fn delete(&self, key: &SecretKey) -> Result<(), SecretError>;
}

/// In-memory store for tests.
#[derive(Default)]
pub struct MemoryStore {
    secrets: Mutex<HashMap<SecretKey, SecretString>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<SecretKey, SecretString>> {
        self.secrets.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl fmt::Debug for MemoryStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MemoryStore")
            .field("entries", &self.len())
            .finish()
    }
}

impl SecretStore for MemoryStore {
    fn get(&self, key: &SecretKey) -> Result<Option<SecretString>, SecretError> {
        Ok(self
            .lock()
            .get(key)
            .map(|s| SecretString::from(s.expose_secret().to_owned())))
    }

    fn set(&self, key: &SecretKey, value: &SecretString) -> Result<(), SecretError> {
        let copy = SecretString::from(value.expose_secret().to_owned());
        self.lock().insert(*key, copy);
        Ok(())
    }

    fn delete(&self, key: &SecretKey) -> Result<(), SecretError> {
        self.lock().remove(key);
        Ok(())
    }
}

/// Stand-in used when no keychain exists: nothing is remembered, every `set` fails with
/// [`SecretError::Unavailable`], so front-ends fall back to prompting each time.
#[derive(Debug, Default)]
pub struct UnavailableStore {
    reason: String,
}

impl UnavailableStore {
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

impl SecretStore for UnavailableStore {
    fn get(&self, _key: &SecretKey) -> Result<Option<SecretString>, SecretError> {
        Ok(None)
    }

    fn set(&self, _key: &SecretKey, _value: &SecretString) -> Result<(), SecretError> {
        Err(SecretError::Unavailable(self.reason.clone()))
    }

    fn delete(&self, _key: &SecretKey) -> Result<(), SecretError> {
        Ok(())
    }
}

/// OS keychain through `keyring-core` (macOS Keychain, Windows Credential Manager,
/// Linux Secret Service).
pub struct KeyringStore {
    store: Arc<CredentialStore>,
    service: String,
}

impl fmt::Debug for KeyringStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeyringStore")
            .field("service", &self.service)
            .finish_non_exhaustive()
    }
}

impl KeyringStore {
    /// The platform keychain under service `filecargo`.
    pub fn native() -> Result<Self, SecretError> {
        Self::native_with_service(SERVICE)
    }

    /// The platform keychain under a custom service name (smoke tests).
    pub fn native_with_service(service: &str) -> Result<Self, SecretError> {
        Ok(Self::from_store(native_store()?, service))
    }

    /// Wraps any `keyring-core` store, e.g. `keyring_core::mock::Store` in tests.
    pub fn from_store(store: Arc<CredentialStore>, service: &str) -> Self {
        Self {
            store,
            service: service.to_owned(),
        }
    }

    fn entry(&self, key: &SecretKey) -> Result<keyring_core::Entry, SecretError> {
        self.store
            .build(&self.service, &key.account(), None)
            .map_err(map_error)
    }
}

impl SecretStore for KeyringStore {
    fn get(&self, key: &SecretKey) -> Result<Option<SecretString>, SecretError> {
        match self.entry(key)?.get_password() {
            Ok(secret) => Ok(Some(SecretString::from(secret))),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(e) => Err(map_error(e)),
        }
    }

    fn set(&self, key: &SecretKey, value: &SecretString) -> Result<(), SecretError> {
        self.entry(key)?
            .set_password(value.expose_secret())
            .map_err(map_error)
    }

    fn delete(&self, key: &SecretKey) -> Result<(), SecretError> {
        match self.entry(key)?.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(e) => Err(map_error(e)),
        }
    }
}

/// The platform keychain, or [`UnavailableStore`] (with one warning) when there is none.
pub fn default_secret_store() -> Arc<dyn SecretStore> {
    match KeyringStore::native() {
        Ok(store) => Arc::new(store),
        Err(e) => {
            tracing::warn!(error = %e, "no keychain; passwords will be asked for every time");
            Arc::new(UnavailableStore::new(e.to_string()))
        }
    }
}

/// Maps without ever formatting variants that can carry secret bytes (`BadEncoding`, ...).
fn map_error(e: keyring_core::Error) -> SecretError {
    use keyring_core::Error as E;
    match e {
        E::NoStorageAccess(err) => SecretError::Unavailable(err.to_string()),
        E::NoDefaultStore => SecretError::Unavailable("no credential store".into()),
        E::NotSupportedByStore(what) => SecretError::Unavailable(what),
        E::PlatformFailure(err) => SecretError::Backend(err.to_string()),
        E::NoEntry => SecretError::Backend("no such entry".into()),
        E::BadEncoding(_) | E::BadDataFormat(..) | E::BadStoreFormat(_) => {
            SecretError::Backend("stored secret is unreadable".into())
        }
        E::TooLong(attr, max) => SecretError::Backend(format!("{attr} longer than {max}")),
        E::Invalid(attr, why) => SecretError::Backend(format!("invalid {attr}: {why}")),
        E::Ambiguous(_) => SecretError::Backend("several matching entries".into()),
        _ => SecretError::Backend("unknown keychain error".into()),
    }
}

#[cfg(target_os = "macos")]
fn native_store() -> Result<Arc<CredentialStore>, SecretError> {
    let store = apple_native_keyring_store::keychain::Store::new().map_err(map_error)?;
    Ok(store)
}

#[cfg(target_os = "windows")]
fn native_store() -> Result<Arc<CredentialStore>, SecretError> {
    let store = windows_native_keyring_store::Store::new().map_err(map_error)?;
    Ok(store)
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn native_store() -> Result<Arc<CredentialStore>, SecretError> {
    let store = zbus_secret_service_keyring_store::Store::new().map_err(map_error)?;
    Ok(store)
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "windows",
    target_os = "linux",
    target_os = "freebsd"
)))]
fn native_store() -> Result<Arc<CredentialStore>, SecretError> {
    Err(SecretError::Unavailable(
        "no keychain on this platform".into(),
    ))
}
