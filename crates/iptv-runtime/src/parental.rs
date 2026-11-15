//! Authoritative parental access and credential operations. PIN hashing and
//! durable retry accounting run only on the store worker, never the UI thread.
use crate::Runtime;
use iptv_core::parental::{Access, ContentKind, Pin, Policy, Target, UnlockWindow};
use iptv_store_sqlite::parental::ParentalCredential;
use std::collections::BTreeSet;
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

const ITERATIONS: u32 = 210_000;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Locked,
    Incorrect,
    RetryAfter(u64),
    NoPin,
    InvalidPin,
    Storage,
    OwnerClosed,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Locked => "Parental access is locked",
            Self::Incorrect => "Incorrect PIN or recovery code",
            Self::RetryAfter(_) => "Too many attempts; try again later",
            Self::NoPin => "Set a parental PIN first",
            Self::InvalidPin => "Use four to eight digits",
            Self::Storage => "Parental settings are unavailable",
            Self::OwnerClosed => "The application has closed",
        })
    }
}
impl std::error::Error for Error {}

#[derive(Default)]
pub(crate) struct State {
    pub access: Access,
    pub ready: bool,
    pub has_pin: bool,
    blocked: BTreeSet<Target>,
    ancestry: std::collections::BTreeMap<Target, Vec<Target>>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}
fn policy(owner: &Runtime) -> Policy {
    use iptv_settings::{
        Target as Scope,
        keys::{BoolKey, EnumKey},
    };
    let settings = owner.settings();
    let scope = Scope::Global;
    let window = match settings.choice(EnumKey("parental.unlock_window"), &scope) {
        "min_10" => UnlockWindow::Minutes(10),
        "min_30" => UnlockWindow::Minutes(30),
        "hour_1" => UnlockWindow::Minutes(60),
        "hour_2" => UnlockWindow::Minutes(120),
        "until_restart" => UnlockWindow::UntilRestart,
        "until_unblocked_channel" => UnlockWindow::UntilUnblockedChannel,
        _ => UnlockWindow::EveryTime,
    };
    Policy {
        live: settings.bool(BoolKey("parental.live_enabled"), &scope),
        vod: settings.bool(BoolKey("parental.vod_enabled"), &scope),
        settings: settings.choice(EnumKey("parental.require_pin_for"), &scope)
            == "blocked_content_and_settings",
        window,
    }
}
fn hash(secret: &[u8], salt: &[u8]) -> [u8; 32] {
    let mut out = [0; 32];
    pbkdf2::pbkdf2_hmac::<sha2::Sha256>(secret, salt, ITERATIONS, &mut out);
    out
}
fn verify(
    store: &mut iptv_store_sqlite::Store,
    secret: &[u8],
    recovery: bool,
    now: i64,
) -> Result<(), Error> {
    let mut record = store
        .parental_credential()
        .map_err(|_| Error::Storage)?
        .ok_or(Error::NoPin)?;
    if record.retry_after > now {
        return Err(Error::RetryAfter((record.retry_after - now) as u64));
    }
    let digest = Zeroizing::new(hash(secret, &record.salt));
    let expected = if recovery {
        &record.recovery_verifier
    } else {
        &record.verifier
    };
    if bool::from(digest.as_slice().ct_eq(expected)) {
        record.failures = 0;
        record.retry_after = 0;
        store
            .set_parental_credential(&record)
            .map_err(|_| Error::Storage)?;
        Ok(())
    } else {
        record.failures = record.failures.saturating_add(1);
        // Persisted backoff survives process termination and clock rollback.
        if record.failures >= 5 {
            record.retry_after = now.saturating_add(
                30_i64.saturating_mul(1_i64 << record.failures.saturating_sub(5).min(7)),
            );
        }
        store
            .set_parental_credential(&record)
            .map_err(|_| Error::Storage)?;
        Err(Error::Incorrect)
    }
}
fn replace_pin(
    store: &mut iptv_store_sqlite::Store,
    new: &Pin,
) -> Result<Zeroizing<String>, Error> {
    if !new.valid() {
        return Err(Error::InvalidPin);
    }
    let mut salt = [0; 16];
    let mut recovery = Zeroizing::new([0; 16]);
    getrandom::fill(&mut salt).map_err(|_| Error::Storage)?;
    getrandom::fill(recovery.as_mut()).map_err(|_| Error::Storage)?;
    let code = Zeroizing::new(
        recovery
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
    );
    let record = ParentalCredential {
        salt,
        verifier: hash(new.bytes(), &salt),
        recovery_verifier: hash(code.as_bytes(), &salt),
        failures: 0,
        retry_after: 0,
    };
    store
        .set_parental_credential(&record)
        .map_err(|_| Error::Storage)?;
    Ok(code)
}
impl Runtime {
    /// Refresh public policy metadata without returning any credential verifier.
    pub async fn parental_refresh(&self) -> Result<(), Error> {
        let services = self.services();
        let (has_pin, blocked, ancestry) = services
            .read(|store| {
                Ok((
                    store.parental_credential()?.is_some(),
                    store.parental_blocks()?,
                    store.parental_memberships()?,
                ))
            })
            .await
            .map_err(|_| Error::Storage)?;
        if !services.is_current() {
            return Err(Error::OwnerClosed);
        }
        let mut state = self.parental.borrow_mut();
        state.has_pin = has_pin;
        state.ready = true;
        state.blocked = blocked.into_iter().collect();
        state.ancestry = ancestry;
        state.access.policy = policy(self);
        Ok(())
    }
    pub fn parental_has_pin(&self) -> bool {
        self.parental.borrow().has_pin
    }
    pub fn parental_is_ready(&self) -> bool {
        self.parental.borrow().ready
    }
    pub fn parental_allowed(&self, target: &Target) -> bool {
        let mut state = self.parental.borrow_mut();
        state.access.policy = policy(self);
        let settings = target.kind == ContentKind::Settings;
        if !state.ready {
            return false;
        }
        if settings {
            return !state.has_pin || state.access.granted(target, now_ms());
        }
        if !state.access.policy.protects(target.kind) {
            return true;
        }
        let blocked = state.blocked.contains(target)
            || state.ancestry.get(target).is_some_and(|parents| {
                parents
                    .iter()
                    .any(|p| state.blocked.contains(p) && !state.access.granted(p, now_ms()))
            });
        !blocked || state.access.granted(target, now_ms())
    }
    /// Fetch ancestry from durable provider membership before accepting a known
    /// content ID. Passing a URL or omitting a category cannot bypass this gate.
    pub async fn parental_check(&self, target: &Target) -> Result<(), Error> {
        self.parental_refresh().await?;
        let services = self.services();
        let target_copy = target.clone();
        let ancestry = services
            .read(move |store| store.parental_ancestors(&target_copy))
            .await
            .map_err(|_| Error::Storage)?;
        if !services.is_current() {
            return Err(Error::OwnerClosed);
        }
        self.parental
            .borrow_mut()
            .ancestry
            .insert(target.clone(), ancestry);
        if self.parental_allowed(target) {
            Ok(())
        } else {
            Err(Error::Locked)
        }
    }
    pub async fn parental_authorize(&self, target: Target, pin: Pin) -> Result<(), Error> {
        self.parental_refresh().await?;
        let services = self.services();
        let now = (now_ms() / 1000) as i64;
        services
            .call(move |store| Ok(verify(store, pin.bytes(), false, now)))
            .await
            .map_err(|_| Error::Storage)??;
        if !services.is_current() {
            return Err(Error::OwnerClosed);
        }
        let mut state = self.parental.borrow_mut();
        state.access.policy = policy(self);
        state.access.unlock(target, now_ms());
        Ok(())
    }
    /// First setup needs no old PIN. Existing credentials can only be replaced
    /// after a verified old PIN; lockout accounting is shared with unlocks.
    pub async fn parental_change_pin(
        &self,
        old: Option<Pin>,
        new: Pin,
    ) -> Result<Zeroizing<String>, Error> {
        let services = self.services();
        let now = (now_ms() / 1000) as i64;
        let result = services
            .call(move |store| {
                let result = (|| {
                    if store
                        .parental_credential()
                        .map_err(|_| Error::Storage)?
                        .is_some()
                    {
                        verify(
                            store,
                            old.as_ref().ok_or(Error::Locked)?.bytes(),
                            false,
                            now,
                        )?;
                    }
                    replace_pin(store, &new)
                })();
                Ok(result)
            })
            .await
            .map_err(|_| Error::Storage)??;
        if !services.is_current() {
            return Err(Error::OwnerClosed);
        }
        self.parental_lock();
        self.parental_refresh().await?;
        self.parental
            .borrow_mut()
            .access
            .unlock(Target::settings(), now_ms());
        Ok(result)
    }
    /// Recovery replaces the PIN, rotates the recovery code and revokes every
    /// previous grant. It never removes protection without a replacement PIN.
    pub async fn parental_recover_pin(
        &self,
        recovery: Zeroizing<String>,
        new: Pin,
    ) -> Result<Zeroizing<String>, Error> {
        let services = self.services();
        let now = (now_ms() / 1000) as i64;
        let result = services
            .call(move |store| {
                Ok(verify(store, recovery.trim().as_bytes(), true, now)
                    .and_then(|()| replace_pin(store, &new)))
            })
            .await
            .map_err(|_| Error::Storage)??;
        if !services.is_current() {
            return Err(Error::OwnerClosed);
        }
        self.parental_lock();
        self.parental_refresh().await?;
        self.parental
            .borrow_mut()
            .access
            .unlock(Target::settings(), now_ms());
        Ok(result)
    }
    pub fn parental_backgrounded(&self) {
        self.parental.borrow_mut().access.backgrounded();
    }
    pub fn parental_snapshot(&self) -> Access {
        let mut state = self.parental.borrow_mut();
        state.access.policy = policy(self);
        state.access.clone()
    }
    pub fn parental_is_blocked(&self, target: &Target) -> bool {
        let state = self.parental.borrow();
        state.blocked.contains(target)
            || state
                .ancestry
                .get(target)
                .is_some_and(|a| a.iter().any(|p| state.blocked.contains(p)))
    }
    pub fn parental_lock(&self) {
        self.parental.borrow_mut().access.lock();
    }
    pub fn parental_unblocked_channel_played(&self) {
        self.parental.borrow_mut().access.unblocked_channel_played();
    }
    pub fn parental_settings_allowed(&self) -> bool {
        self.parental_allowed(&Target::settings())
    }
    pub async fn parental_set_blocked(&self, target: Target, blocked: bool) -> Result<(), Error> {
        self.parental_refresh().await?;
        if !self.parental_has_pin() {
            return Err(Error::NoPin);
        }
        if !self.parental_settings_allowed() {
            return Err(Error::Locked);
        }
        self.services()
            .call(move |store| store.set_parental_blocked(&target, blocked))
            .await
            .map_err(|_| Error::Storage)?;
        self.parental_refresh().await
    }
    pub fn parental_blocked_targets(&self) -> Vec<Target> {
        self.parental.borrow().blocked.iter().cloned().collect()
    }
}

/// Native command gate. Resolve membership from storage for every direct-ID
/// command; callers cannot omit or spoof the item's category.
pub(crate) fn native_allowed(
    store: &iptv_store_sqlite::Store,
    target: &Target,
    access: &Access,
) -> iptv_store_sqlite::Result<bool> {
    if !access.policy.protects(target.kind) {
        return Ok(true);
    }
    let blocked = store.parental_blocks()?;
    let parents = store.parental_ancestors(target)?;
    let denied = blocked.contains(target)
        || parents
            .iter()
            .any(|p| blocked.contains(p) && !access.granted(p, now_ms()));
    Ok(!denied || access.granted(target, now_ms()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn salted_verifiers_rotation_recovery_and_durable_lockout() {
        let mut store = iptv_store_sqlite::Store::open_in_memory().unwrap();
        let pin = Pin::parse("2468").unwrap();
        let code = replace_pin(&mut store, &pin).unwrap();
        let original = store.parental_credential().unwrap().unwrap();
        assert_ne!(original.verifier.as_slice(), pin.bytes());
        assert_eq!(format!("{original:?}"), "ParentalCredential(***)");
        assert!(verify(&mut store, b"2468", false, 100).is_ok());
        assert_eq!(
            verify(&mut store, b"1357", false, 101),
            Err(Error::Incorrect)
        );
        assert_eq!(store.parental_credential().unwrap().unwrap().failures, 1);
        for _ in 1..5 {
            assert_eq!(
                verify(&mut store, b"1357", false, 101),
                Err(Error::Incorrect)
            );
        }
        assert_eq!(
            verify(&mut store, b"2468", false, 102),
            Err(Error::RetryAfter(29))
        );
        assert!(verify(&mut store, code.as_bytes(), true, 131).is_ok());
        let new_code = replace_pin(&mut store, &Pin::parse("8642").unwrap()).unwrap();
        let changed = store.parental_credential().unwrap().unwrap();
        assert_ne!(original.salt, changed.salt);
        assert_ne!(*code, *new_code);
        assert_eq!(
            verify(&mut store, b"2468", false, 132),
            Err(Error::Incorrect)
        );
        assert_eq!(
            verify(&mut store, code.as_bytes(), true, 132),
            Err(Error::Incorrect)
        );
        assert!(verify(&mut store, b"8642", false, 132).is_ok());
        assert_eq!(store.parental_credential().unwrap().unwrap().failures, 0);
    }
}
