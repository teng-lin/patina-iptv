//! Stable Secret Service backend with a bounded unlock prompt. Attribute names
//! match keyring's existing item, so this does not create a second credential.
use dbus_secret_service::{EncryptionType, SecretService};
use std::collections::HashMap;
pub(super) struct Entry {
    broker: SecretService,
    service: String,
    account: String,
}
fn error(error: dbus_secret_service::Error) -> keyring::Error {
    keyring::Error::PlatformFailure(Box::new(error))
}
impl Entry {
    pub(super) fn new(service: &str, account: &str) -> Result<Self, keyring::Error> {
        let broker = SecretService::connect_with_max_prompt_timeout(EncryptionType::Dh, 30)
            .map_err(error)?;
        Ok(Self {
            broker,
            service: service.to_owned(),
            account: account.to_owned(),
        })
    }
    fn attributes(&self) -> HashMap<&str, &str> {
        HashMap::from([
            ("service", self.service.as_str()),
            ("username", self.account.as_str()),
            ("target", "default"),
        ])
    }
    pub(super) fn get_secret(&self) -> Result<Vec<u8>, keyring::Error> {
        let found = self.broker.search_items(self.attributes()).map_err(error)?;
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
        let collection = self.broker.get_default_collection().map_err(error)?;
        if collection.is_locked().map_err(error)? {
            collection.unlock().map_err(error)?;
        }
        let mut attributes = self.attributes();
        attributes.insert("application", "rust-keyring");
        let label = match self.service.as_str() {
            "org.iptv-rs.database" => "Patina catalog key",
            _ => "Patina AirPlay pairing",
        };
        collection
            .create_item(label, attributes, secret, true, "text/plain")
            .map_err(error)?;
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn lock_collection(&self) -> Result<(), keyring::Error> {
        self.broker
            .get_default_collection()
            .map_err(error)?
            .lock()
            .map_err(error)
    }

    pub(super) fn delete_secret(&self) -> Result<(), keyring::Error> {
        let found = self.broker.search_items(self.attributes()).map_err(error)?;
        for item in found.unlocked.iter().chain(&found.locked) {
            if item.is_locked().map_err(error)? {
                item.unlock().map_err(error)?;
            }
            item.delete().map_err(error)?;
        }
        Ok(())
    }
}
