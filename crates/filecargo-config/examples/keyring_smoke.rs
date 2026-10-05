//! Manual smoke test against the real OS keychain:
//! `cargo run -p filecargo-config --example keyring_smoke`
//!
//! Stores, reads and deletes a dummy secret under the service `filecargo-smoke`
//! (never under the real `filecargo` service).

use filecargo_config::{ExposeSecret, KeyringStore, SecretKey, SecretStore, SecretString, SiteId};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let store = KeyringStore::native_with_service("filecargo-smoke")?;
    let key = SecretKey::Password(SiteId::new());
    let dummy = SecretString::from("dummy-smoke-secret".to_owned());

    store.set(&key, &dummy)?;
    let read = store.get(&key)?.ok_or("secret missing after set")?;
    assert_eq!(
        read.expose_secret(),
        dummy.expose_secret(),
        "round-trip mismatch"
    );
    println!("set + get ok ({})", key.account());

    store.delete(&key)?;
    assert!(
        store.get(&key)?.is_none(),
        "secret still present after delete"
    );
    println!("delete ok; keychain smoke test passed");
    Ok(())
}
