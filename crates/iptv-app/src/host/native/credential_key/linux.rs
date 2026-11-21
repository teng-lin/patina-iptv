//! Stable Secret Service backend with a bounded unlock prompt. Attribute names
//! match keyring's existing item, so this does not create a second credential.
use dbus_secret_service::{EncryptionType, SecretService};
use std::collections::HashMap;
pub(super) struct Entry(SecretService);
fn error(error: dbus_secret_service::Error) -> keyring::Error {
    keyring::Error::PlatformFailure(Box::new(error))
}
fn attributes() -> HashMap<&'static str, &'static str> {
    HashMap::from([
        ("service", "org.iptv-rs.database"),
        ("username", "catalog-v1"),
        ("target", "default"),
    ])
}
impl Entry {
    pub(super) fn new(_: &str, _: &str) -> Result<Self, keyring::Error> {
        SecretService::connect_with_max_prompt_timeout(EncryptionType::Dh, 30)
            .map(Self)
            .map_err(error)
    }
    pub(super) fn get_secret(&self) -> Result<Vec<u8>, keyring::Error> {
        let found = self.0.search_items(attributes()).map_err(error)?;
        let count = found.locked.len() + found.unlocked.len();
        if count == 0 {
            return Err(keyring::Error::NoEntry);
        }
        if count != 1 {
            return Err(keyring::Error::Ambiguous(Vec::new()));
        }
        let item = found
            .unlocked
            .first()
            .or_else(|| found.locked.first())
            .expect("one item");
        if item.is_locked().map_err(error)? {
            item.unlock().map_err(error)?;
        }
        item.get_secret().map_err(error)
    }
    pub(super) fn set_secret(&self, secret: &[u8]) -> Result<(), keyring::Error> {
        let collection = self.0.get_default_collection().map_err(error)?;
        if collection.is_locked().map_err(error)? {
            collection.unlock().map_err(error)?;
        }
        let mut attributes = attributes();
        attributes.insert("application", "rust-keyring");
        collection
            .create_item("Patina catalog key", attributes, secret, true, "text/plain")
            .map_err(error)?;
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn lock_collection(&self) -> Result<(), keyring::Error> {
        self.0
            .get_default_collection()
            .map_err(error)?
            .lock()
            .map_err(error)
    }

    #[cfg(test)]
    pub(super) fn delete_secret(&self) -> Result<(), keyring::Error> {
        let found = self.0.search_items(attributes()).map_err(error)?;
        for item in found.unlocked.iter().chain(&found.locked) {
            item.delete().map_err(error)?;
        }
        Ok(())
    }
}
