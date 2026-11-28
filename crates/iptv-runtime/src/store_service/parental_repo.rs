//! Store operations that enforce parental credentials and grants.

use super::{Services, Store, StoreError};
use crate::error::SETTINGS_LOCKED_REASON;
use crate::parental::gate::{Error, Snapshot, now_ms, policy_settings};
use iptv_core::parental::{ContentKind, Pin, Target};
use iptv_library::SourceId;
use iptv_providers::model::SourceConfig;
use iptv_store_sqlite::parental::ParentalCredential;
use std::collections::BTreeMap;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

pub(crate) struct ParentalRepo {
    pub(super) services: Services,
}

pub(crate) struct CatalogAccess {
    pub has_pin: bool,
    pub blocked: Vec<Target>,
    pub ancestry: BTreeMap<Target, Vec<Target>>,
    pub names: BTreeMap<Target, String>,
}

pub(crate) struct TargetAccess {
    pub target: Target,
    pub parents: Vec<Target>,
    pub blocked: Vec<(Target, bool)>,
}

pub(crate) struct TargetAccessSnapshot {
    pub has_pin: bool,
    pub entries: Vec<TargetAccess>,
    pub guide_blocked: bool,
}

fn hash(secret: &[u8], salt: &[u8]) -> [u8; 32] {
    let mut out = [0; 32];
    pbkdf2::pbkdf2_hmac::<sha2::Sha256>(secret, salt, 210_000, &mut out);
    out
}

fn verify(store: &mut Store, secret: &[u8], recovery: bool, now: i64) -> Result<(), Error> {
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
    store: &mut Store,
    new: &Pin,
    entropy: crate::ports::Entropy,
) -> Result<Zeroizing<String>, Error> {
    if !new.valid() {
        return Err(Error::InvalidPin);
    }
    let mut salt = [0; 16];
    let mut recovery = Zeroizing::new([0; 16]);
    entropy(&mut salt).map_err(|_| Error::Storage)?;
    entropy(recovery.as_mut()).map_err(|_| Error::Storage)?;
    let code = Zeroizing::new(
        recovery
            .iter()
            .map(|byte| format!("{byte:02x}"))
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

impl ParentalRepo {
    pub(crate) async fn catalog(&self) -> Result<CatalogAccess, StoreError> {
        self.services
            .read(|store| {
                store.read_snapshot(|store| {
                    let blocked = store.parental_blocks()?;
                    let mut names = BTreeMap::new();
                    for target in &blocked {
                        if let Some(name) = store.parental_name(target)? {
                            names.insert(target.clone(), name);
                        }
                    }
                    Ok(CatalogAccess {
                        has_pin: store.parental_credential()?.is_some(),
                        blocked,
                        ancestry: store.parental_memberships()?,
                        names,
                    })
                })
            })
            .await
    }

    pub(crate) async fn targets(
        self,
        targets: Vec<Target>,
        guide_channel: Option<Target>,
    ) -> Result<TargetAccessSnapshot, StoreError> {
        self.services
            .read(move |store| {
                store.read_snapshot(|store| {
                    let mut entries = Vec::new();
                    for target in targets {
                        let mut parents = store.parental_ancestors(&target)?;
                        parents.sort_unstable();
                        parents.dedup();
                        let blocked = std::iter::once(&target)
                            .chain(parents.iter())
                            .map(|candidate| {
                                Ok((candidate.clone(), store.parental_target_blocked(candidate)?))
                            })
                            .collect::<Result<_, StoreError>>()?;
                        entries.push(TargetAccess {
                            target,
                            parents,
                            blocked,
                        });
                    }
                    let guide_blocked = match guide_channel {
                        Some(channel) => {
                            store.guide_channel_blocked(SourceId(channel.source), &channel.id)?
                        }
                        None => false,
                    };
                    Ok(TargetAccessSnapshot {
                        has_pin: store.parental_credential()?.is_some(),
                        entries,
                        guide_blocked,
                    })
                })
            })
            .await
    }

    pub(crate) async fn verify_pin(
        self,
        pin: Pin,
        epoch: Arc<AtomicU64>,
        expected: u64,
        now: i64,
    ) -> Result<(), Error> {
        self.services
            .call(move |store| {
                if epoch.load(Ordering::SeqCst) != expected {
                    return Ok(Err(Error::Locked));
                }
                let verified = verify(store, pin.bytes(), false, now);
                if epoch.load(Ordering::SeqCst) != expected {
                    return Ok(Err(Error::Locked));
                }
                Ok(verified)
            })
            .await
            .map_err(|_| Error::Storage)?
    }

    pub(crate) async fn change_pin(
        self,
        old: Option<Pin>,
        new: Pin,
        epoch: Arc<AtomicU64>,
        now: i64,
    ) -> Result<Zeroizing<String>, Error> {
        let entropy = self.services.ports().entropy;
        self.services
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
                    replace_pin(store, &new, entropy)
                })();
                if result.is_ok() {
                    epoch.fetch_add(1, Ordering::SeqCst);
                }
                Ok(result)
            })
            .await
            .map_err(|_| Error::Storage)?
    }

    pub(crate) async fn recover_pin(
        self,
        recovery: Zeroizing<String>,
        new: Pin,
        epoch: Arc<AtomicU64>,
        now: i64,
    ) -> Result<Zeroizing<String>, Error> {
        let entropy = self.services.ports().entropy;
        self.services
            .call(move |store| {
                let result = verify(store, recovery.trim().as_bytes(), true, now)
                    .and_then(|()| replace_pin(store, &new, entropy));
                if result.is_ok() {
                    epoch.fetch_add(1, Ordering::SeqCst);
                }
                Ok(result)
            })
            .await
            .map_err(|_| Error::Storage)?
    }

    pub(crate) async fn set_blocked(
        self,
        target: Target,
        blocked: bool,
        access: Snapshot,
    ) -> Result<(), StoreError> {
        self.services
            .call(move |store| {
                Self::require_admin(store, &access)?;
                store.set_parental_blocked(&target, blocked)
            })
            .await
    }

    pub(crate) fn require_settings_write(
        store: &Store,
        snapshot: &Snapshot,
        settings: &iptv_settings::Settings,
    ) -> Result<(), StoreError> {
        let policy = policy_settings(settings);
        if !policy.settings {
            return Ok(());
        }
        if policy != snapshot.access.policy || !snapshot.policy_current() {
            return Err(settings_locked());
        }
        Self::require_admin(store, snapshot)
    }

    #[cfg(all(test, feature = "native"))]
    pub(crate) fn verify(
        store: &mut Store,
        secret: &[u8],
        recovery: bool,
        now: i64,
    ) -> Result<(), Error> {
        verify(store, secret, recovery, now)
    }

    #[cfg(all(test, feature = "native"))]
    pub(crate) fn replace_pin(
        store: &mut Store,
        new: &Pin,
        entropy: crate::ports::Entropy,
    ) -> Result<Zeroizing<String>, Error> {
        replace_pin(store, new, entropy)
    }

    pub(crate) fn inspection_dependencies(
        store: &Store,
        snapshot: &Snapshot,
        key: &iptv_library::inspection::ChannelRef,
    ) -> Result<Vec<Target>, StoreError> {
        if !snapshot.access.policy.protects(ContentKind::Live) {
            return Ok(Vec::new());
        }
        let target = Target::live(key.source.0, key.content.as_str());
        let mut blocked = Vec::new();
        for candidate in std::iter::once(target.clone()).chain(store.parental_ancestors(&target)?) {
            if store.parental_target_blocked(&candidate)? {
                blocked.push(candidate);
            }
        }
        if blocked.is_empty() {
            return Ok(Vec::new());
        }
        if snapshot.access.granted(&target, now_ms()) {
            Ok(vec![target])
        } else {
            Ok(blocked)
        }
    }

    pub(crate) fn require_admin(store: &Store, snapshot: &Snapshot) -> Result<(), StoreError> {
        if store.parental_credential()?.is_none()
            || (snapshot.epoch.load(Ordering::SeqCst) == snapshot.at
                && snapshot.access.granted(&Target::settings(), now_ms()))
        {
            Ok(())
        } else {
            Err(settings_locked())
        }
    }

    pub(crate) fn import_allowed(
        store: &Store,
        stored: Option<SourceId>,
        config: &SourceConfig,
        edit: bool,
        snapshot: &Snapshot,
    ) -> Result<bool, StoreError> {
        let same = stored
            .map(|id| store.source(id))
            .transpose()?
            .flatten()
            .is_some_and(|stored| stored.source.config == *config);
        if same && !edit {
            Ok(true)
        } else {
            Ok(store.parental_credential()?.is_none()
                || (snapshot.epoch.load(Ordering::SeqCst) == snapshot.at
                    && snapshot.access.granted(&Target::settings(), now_ms())))
        }
    }

    pub(crate) async fn import_allowed_for(
        self,
        stored: Option<SourceId>,
        config: SourceConfig,
        edit: bool,
        snapshot: Snapshot,
    ) -> Result<bool, StoreError> {
        self.services
            .read(move |store| Self::import_allowed(store, stored, &config, edit, &snapshot))
            .await
    }

    pub(crate) fn require_group_edit(
        store: &Store,
        group: crate::model::CustomGroupId,
        snapshot: &Snapshot,
    ) -> Result<(), StoreError> {
        if store.parental_custom_group_blocked(group)?
            && (snapshot.epoch.load(Ordering::SeqCst) != snapshot.at
                || !snapshot.access.granted(&Target::settings(), now_ms()))
        {
            Err(settings_locked())
        } else {
            Ok(())
        }
    }

    pub(crate) fn native_allowed(
        store: &Store,
        target: &Target,
        snapshot: &Snapshot,
    ) -> Result<bool, StoreError> {
        Ok(Self::native_grants(store, target, snapshot)?.is_some())
    }

    pub(crate) fn native_grants(
        store: &Store,
        target: &Target,
        snapshot: &Snapshot,
    ) -> Result<Option<Vec<Target>>, StoreError> {
        if !snapshot.inspection_current(&[]) {
            return Ok(None);
        }
        if !snapshot.access.policy.protects(target.kind) {
            return Ok(Some(Vec::new()));
        }
        let blocked = store.parental_target_blocked(target)?;
        let mut blocked_parents = Vec::new();
        for parent in store.parental_ancestors(target)? {
            if store.parental_target_blocked(&parent)? {
                blocked_parents.push(parent);
            }
        }
        if !blocked && snapshot.playback_current(target, &blocked_parents) {
            return Ok(Some(blocked_parents));
        }
        Ok(snapshot
            .playback_current(target, std::slice::from_ref(target))
            .then(|| vec![target.clone()]))
    }
}

fn settings_locked() -> StoreError {
    StoreError::Unavailable(SETTINGS_LOCKED_REASON)
}
