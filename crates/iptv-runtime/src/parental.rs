//! Authoritative parental access and credential operations. PIN hashing and
//! durable retry accounting run only on the store worker, never the UI thread.
use crate::Runtime;
use iptv_core::parental::{Access, ContentKind, Pin, Policy, Target, UnlockWindow};
use iptv_library::SourceId;
use iptv_store_sqlite::parental::ParentalCredential;
use std::{
    collections::BTreeSet,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
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
    complete: bool,
    epoch: Arc<AtomicU64>,
    policy_revision: Arc<AtomicU64>,
    blocked: BTreeSet<Target>,
    ancestry: std::collections::BTreeMap<Target, Vec<Target>>,
    names: std::collections::BTreeMap<Target, String>,
}

fn apply_policy(state: &mut State, new: Policy) {
    if state.access.policy == new {
        return;
    }
    let settings = state.access.granted(&Target::settings(), now_ms());
    state.epoch.fetch_add(1, Ordering::SeqCst);
    state.policy_revision.fetch_add(1, Ordering::SeqCst);
    state.access.lock();
    state.access.policy = new;
    if settings {
        state.access.unlock(Target::settings(), now_ms());
    }
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
    policy_settings(&owner.settings())
}
pub(crate) fn policy_settings(settings: &iptv_settings::Settings) -> Policy {
    use iptv_settings::Target as Scope;
    let scope = Scope::Global;
    let window = match settings.choice(iptv_settings::keys::PARENTAL_UNLOCK_WINDOW, &scope) {
        "min_10" => UnlockWindow::Minutes(10),
        "min_30" => UnlockWindow::Minutes(30),
        "hour_1" => UnlockWindow::Minutes(60),
        "hour_2" => UnlockWindow::Minutes(120),
        "until_restart" => UnlockWindow::UntilRestart,
        "until_unblocked_channel" => UnlockWindow::UntilUnblockedChannel,
        _ => UnlockWindow::EveryTime,
    };
    Policy {
        live: settings.bool(iptv_settings::keys::PARENTAL_LIVE_ENABLED, &scope),
        vod: settings.bool(iptv_settings::keys::PARENTAL_VOD_ENABLED, &scope),
        settings: settings.choice(iptv_settings::keys::PARENTAL_REQUIRE_PIN_FOR, &scope)
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
        let (has_pin, blocked, ancestry, names) = services
            .read(|store| {
                store.read_snapshot(|store| {
                    let blocked = store.parental_blocks()?;
                    let mut names = std::collections::BTreeMap::new();
                    for target in &blocked {
                        if let Some(name) = store.parental_name(target)? {
                            names.insert(target.clone(), name);
                        }
                    }
                    Ok((
                        store.parental_credential()?.is_some(),
                        blocked,
                        store.parental_memberships()?,
                        names,
                    ))
                })
            })
            .await
            .map_err(|_| Error::Storage)?;
        if !services.is_current() {
            return Err(Error::OwnerClosed);
        }
        let mut state = self.parental.borrow_mut();
        state.has_pin = has_pin;
        state.ready = true;
        let blocked: BTreeSet<_> = blocked.into_iter().collect();
        let mut ancestry = ancestry;
        for parents in ancestry.values_mut() {
            parents.sort_unstable();
            parents.dedup();
        }
        // Targeted reads cache empty parent lists; full membership reads omit
        // them. Neither cache population nor SQL row order changes authority.
        let ancestry_changed = if state.complete {
            state
                .ancestry
                .iter()
                .filter(|(_, parents)| !parents.is_empty())
                .ne(ancestry.iter())
        } else {
            state.ancestry.iter().any(|(target, parents)| {
                ancestry
                    .get(target)
                    .map_or(!parents.is_empty(), |current| current != parents)
            })
        };
        if state.blocked != blocked || ancestry_changed {
            state.policy_revision.fetch_add(1, Ordering::SeqCst);
        }
        state.complete = true;
        state.blocked = blocked;
        state.ancestry = ancestry;
        state.names = names;
        apply_policy(&mut state, policy(self));
        Ok(())
    }
    /// Update authorization for a bounded set of content identities. The full
    /// catalogue snapshot is reserved for boot, catalogue changes and management.
    pub(crate) async fn parental_refresh_targets(&self, targets: Vec<Target>) -> Result<(), Error> {
        self.refresh_targets(targets, None).await.map(|_| ())
    }

    async fn refresh_targets(
        &self,
        targets: Vec<Target>,
        guide_channel: Option<Target>,
    ) -> Result<bool, Error> {
        let services = self.services();
        let result = services
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
                            .collect::<iptv_store_sqlite::Result<Vec<_>>>()?;
                        entries.push((target, parents, blocked));
                    }
                    let guide_blocked = match guide_channel {
                        Some(channel) => {
                            store.guide_channel_blocked(SourceId(channel.source), &channel.id)?
                        }
                        None => false,
                    };
                    Ok((
                        store.parental_credential()?.is_some(),
                        entries,
                        guide_blocked,
                    ))
                })
            })
            .await
            .map_err(|_| Error::Storage)?;
        if !services.is_current() {
            return Err(Error::OwnerClosed);
        }
        let mut state = self.parental.borrow_mut();
        state.has_pin = result.0;
        state.ready = true;
        let mut changed = false;
        for (target, parents, blocked) in result.1 {
            let known_target = state.complete || state.ancestry.contains_key(&target);
            changed |= state
                .ancestry
                .get(&target)
                .map_or(state.complete && !parents.is_empty(), |previous| {
                    previous != &parents
                });
            // Absence from a partial cache says nothing about a new identity.
            // Previously resolved parents, including allowed ones, are known.
            for (candidate, blocked) in blocked {
                let known = known_target
                    || state.blocked.contains(&candidate)
                    || state.ancestry.contains_key(&candidate)
                    || state
                        .ancestry
                        .values()
                        .any(|parents| parents.contains(&candidate));
                changed |= known && state.blocked.contains(&candidate) != blocked;
                if blocked {
                    state.blocked.insert(candidate);
                } else {
                    state.blocked.remove(&candidate);
                }
            }
            state.ancestry.insert(target, parents);
        }
        if changed {
            state.policy_revision.fetch_add(1, Ordering::SeqCst);
        }
        apply_policy(&mut state, policy(self));
        Ok(result.2)
    }
    pub(crate) fn parental_policy_changed(&self) {
        apply_policy(&mut self.parental.borrow_mut(), policy(self));
    }
    pub fn parental_has_pin(&self) -> bool {
        self.parental.borrow().has_pin
    }
    pub fn parental_is_ready(&self) -> bool {
        self.parental.borrow().ready
    }
    pub fn parental_allowed(&self, target: &Target) -> bool {
        let mut state = self.parental.borrow_mut();
        apply_policy(&mut state, policy(self));
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
        if !state.complete && !state.ancestry.contains_key(target) {
            return false;
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
        self.parental_refresh_targets(vec![target.clone()]).await?;
        if self.parental_allowed(target) {
            Ok(())
        } else {
            Err(Error::Locked)
        }
    }
    /// Checks durable guide blocks and parental access in one read for playback.
    pub async fn live_channel_allowed(
        &self,
        source: SourceId,
        channel: String,
    ) -> Result<bool, Error> {
        let target = Target::live(source.0, channel);
        let blocked = self
            .refresh_targets(vec![target.clone()], Some(target.clone()))
            .await?;
        Ok(!blocked && self.parental_allowed(&target))
    }

    pub async fn parental_authorize(&self, target: Target, pin: Pin) -> Result<(), Error> {
        self.parental_refresh_targets(vec![target.clone()]).await?;
        let services = self.services();
        let now = (now_ms() / 1000) as i64;
        let epoch = self.parental.borrow().epoch.clone();
        let expected = epoch.load(Ordering::SeqCst);
        let verification_epoch = epoch.clone();
        services
            .call(move |store| {
                if verification_epoch.load(Ordering::SeqCst) != expected {
                    return Ok(Err(Error::Locked));
                }
                let verified = verify(store, pin.bytes(), false, now);
                if verification_epoch.load(Ordering::SeqCst) != expected {
                    return Ok(Err(Error::Locked));
                }
                Ok(verified)
            })
            .await
            .map_err(|_| Error::Storage)??;
        if !services.is_current() {
            return Err(Error::OwnerClosed);
        }
        if epoch.load(Ordering::SeqCst) != expected {
            return Err(Error::Locked);
        }
        let mut state = self.parental.borrow_mut();
        apply_policy(&mut state, policy(self));
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
        let epoch = self.parental.borrow().epoch.clone();
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
                if result.is_ok() {
                    epoch.fetch_add(1, Ordering::SeqCst);
                }
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
        let epoch = self.parental.borrow().epoch.clone();
        let now = (now_ms() / 1000) as i64;
        let result = services
            .call(move |store| {
                let result = verify(store, recovery.trim().as_bytes(), true, now)
                    .and_then(|()| replace_pin(store, &new));
                if result.is_ok() {
                    epoch.fetch_add(1, Ordering::SeqCst);
                }
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
    pub fn parental_content_left(&self, target: &Target) {
        let mut state = self.parental.borrow_mut();
        state.access.content_left(target);
        state.epoch.fetch_add(1, Ordering::SeqCst);
    }
    pub fn parental_content_closed(&self) {
        if self.parental.borrow().access.policy.window == UnlockWindow::EveryTime {
            self.parental_lock();
        }
    }
    pub fn parental_backgrounded(&self) {
        if self.parental.borrow().access.policy.window == UnlockWindow::EveryTime {
            self.parental_lock();
        }
    }
    pub(crate) fn parental_snapshot(&self) -> Snapshot {
        let mut state = self.parental.borrow_mut();
        apply_policy(&mut state, policy(self));
        Snapshot {
            access: state.access.clone(),
            epoch: state.epoch.clone(),
            at: state.epoch.load(Ordering::SeqCst),
            policy_revision: state.policy_revision.clone(),
            policy_at: state.policy_revision.load(Ordering::SeqCst),
        }
    }
    pub fn parental_explicitly_blocked(&self, target: &Target) -> bool {
        self.parental.borrow().blocked.contains(target)
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
        let mut state = self.parental.borrow_mut();
        state.epoch.fetch_add(1, Ordering::SeqCst);
        state.access.lock();
    }
    pub fn parental_unblocked_channel_played(&self) {
        if matches!(
            self.parental.borrow().access.policy.window,
            UnlockWindow::EveryTime | UnlockWindow::UntilUnblockedChannel
        ) {
            self.parental_lock();
        }
    }
    /// Strict administrative authority for parental configuration, backup and
    /// protected membership edits. Ordinary settings only request this grant
    /// when `parental.require_pin_for` includes settings.
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
        let access = self.parental_snapshot();
        access.policy_revision.fetch_add(1, Ordering::SeqCst);
        self.services()
            .call(move |store| {
                require_admin(store, &access)?;
                store.set_parental_blocked(&target, blocked)
            })
            .await
            .map_err(|_| Error::Storage)?;
        self.parental_refresh().await
    }
    pub fn parental_name(&self, target: &Target) -> Option<String> {
        self.parental.borrow().names.get(target).cloned()
    }
    pub fn parental_blocked_targets(&self) -> Vec<Target> {
        self.parental.borrow().blocked.iter().cloned().collect()
    }
}

/// Native command gate. Resolve membership from storage for every direct-ID
/// command; callers cannot omit or spoof the item's category.
#[derive(Clone)]
pub(crate) struct Snapshot {
    pub access: Access,
    epoch: Arc<AtomicU64>,
    at: u64,
    policy_revision: Arc<AtomicU64>,
    policy_at: u64,
}
impl Snapshot {
    pub(crate) fn inspection_access(&self) -> iptv_store_sqlite::InspectionAccess {
        let current = self.epoch.load(Ordering::SeqCst) == self.at;
        let now = now_ms();
        iptv_store_sqlite::InspectionAccess {
            unrestricted: !self.access.policy.protects(ContentKind::Live)
                || (current && self.access.all_content_granted(now)),
            grants: if current {
                self.access.content_grants().cloned().collect()
            } else {
                Vec::new()
            },
        }
    }
    pub(crate) fn inspection_context(&self) -> String {
        let access = self.inspection_access();
        use sha2::Digest;
        format!(
            "{:x}",
            sha2::Sha256::digest(format!("{}:{access:?}", self.policy_at).as_bytes())
        )
    }
    pub(crate) fn inspection_dependencies(
        &self,
        store: &iptv_store_sqlite::Store,
        key: &iptv_library::inspection::ChannelRef,
    ) -> iptv_store_sqlite::Result<Vec<Target>> {
        if !self.access.policy.protects(ContentKind::Live) {
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
        if self.access.granted(&target, now_ms()) {
            Ok(vec![target])
        } else {
            Ok(blocked)
        }
    }
    pub(crate) fn inspection_current(&self, grants: &[Target]) -> bool {
        self.policy_revision.load(Ordering::SeqCst) == self.policy_at
            && (grants.is_empty()
                || (self.epoch.load(Ordering::SeqCst) == self.at
                    && grants
                        .iter()
                        .all(|target| self.access.granted(target, now_ms()))))
    }
}

/// Administrative changes are gated against the credential that exists at
/// execution time, including PIN setup or relock after work was queued.
pub(crate) fn require_admin(
    store: &iptv_store_sqlite::Store,
    snapshot: &Snapshot,
) -> iptv_store_sqlite::Result<()> {
    if store.parental_credential()?.is_some()
        && (snapshot.epoch.load(Ordering::SeqCst) != snapshot.at
            || !snapshot.access.granted(&Target::settings(), now_ms()))
    {
        return Err(iptv_store_sqlite::StoreError::Unavailable(
            "Unlock parental controls before changing settings",
        ));
    }
    Ok(())
}
/// Only a refresh of the exact, durably authorized provider configuration is
/// exempt. Caller-controlled purpose/edit flags never grant that exemption.
pub(crate) fn require_import(
    store: &iptv_store_sqlite::Store,
    job: &crate::imports::Job,
    snapshot: &Snapshot,
) -> iptv_store_sqlite::Result<()> {
    let same = job
        .stored
        .map(|id| store.source(id))
        .transpose()?
        .flatten()
        .is_some_and(|stored| stored.source.config == job.source.config);
    if !same || job.edit {
        require_admin(store, snapshot)?;
    }
    Ok(())
}
pub(crate) fn require_group_edit(
    store: &iptv_store_sqlite::Store,
    group: iptv_store_sqlite::CustomGroupId,
    snapshot: &Snapshot,
) -> iptv_store_sqlite::Result<()> {
    if store.parental_custom_group_blocked(group)?
        && (snapshot.epoch.load(Ordering::SeqCst) != snapshot.at
            || !snapshot.access.granted(&Target::settings(), now_ms()))
    {
        return Err(iptv_store_sqlite::StoreError::Unavailable(
            "Unlock parental controls before changing settings",
        ));
    }
    Ok(())
}
pub(crate) fn native_allowed(
    store: &iptv_store_sqlite::Store,
    target: &Target,
    snapshot: &Snapshot,
) -> iptv_store_sqlite::Result<bool> {
    if snapshot.policy_revision.load(Ordering::SeqCst) != snapshot.policy_at {
        return Ok(false);
    }
    let access = snapshot.access.clone();
    let grants_current = snapshot.epoch.load(Ordering::SeqCst) == snapshot.at;
    if !access.policy.protects(target.kind) {
        return Ok(true);
    }
    let parents = store.parental_ancestors(target)?;
    let mut denied = store.parental_target_blocked(target)?;
    for parent in parents {
        denied |= store.parental_target_blocked(&parent)?
            && !(grants_current && access.granted(&parent, now_ms()));
    }
    Ok(!denied || (grants_current && access.granted(target, now_ms())))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot() -> Snapshot {
        Snapshot {
            access: Access::default(),
            epoch: Arc::new(AtomicU64::new(0)),
            at: 0,
            policy_revision: Arc::new(AtomicU64::new(0)),
            policy_at: 0,
        }
    }

    #[test]
    fn final_inspection_fence_only_needs_live_grants_when_the_result_used_them() {
        let target = Target::live(1, "synthetic-channel");
        let mut authority = snapshot();
        authority.access.policy.live = true;
        authority.access.unlock(target.clone(), now_ms());
        assert!(authority.inspection_current(std::slice::from_ref(&target)));
        authority.epoch.fetch_add(1, Ordering::SeqCst);
        assert!(!authority.inspection_current(std::slice::from_ref(&target)));
        assert!(
            authority.inspection_current(&[]),
            "unblocked results survive unrelated zapping"
        );
        authority.policy_revision.fetch_add(1, Ordering::SeqCst);
        assert!(
            !authority.inspection_current(&[]),
            "policy/block changes invalidate publication"
        );
        let mut expired = snapshot();
        expired.access.policy.window = UnlockWindow::Minutes(10);
        expired
            .access
            .unlock(target.clone(), now_ms().saturating_sub(600_000));
        assert!(!expired.inspection_current(&[target]));
    }

    #[test]
    fn queued_native_admission_distinguishes_grant_revocation_from_unprotected_zapping() {
        let mut store = iptv_store_sqlite::Store::open_in_memory().unwrap();
        let source = store
            .add_source(&iptv_providers::model::Source {
                name: "Synthetic authority fixture".into(),
                config: iptv_providers::model::SourceConfig::M3u(
                    iptv_providers::model::M3uSourceConfig {
                        location: iptv_providers::SecretUrl::new(
                            "https://source.example.test/generated.m3u",
                        ),
                        user_agent: None,
                        referrer: None,
                    },
                ),
            })
            .unwrap();
        let target = Target::live(source.0, "synthetic-blocked");
        store.set_parental_blocked(&target, true).unwrap();
        let mut authority = snapshot();
        authority.access.policy.live = true;
        authority.access.unlock(target.clone(), now_ms());
        assert!(native_allowed(&store, &target, &authority).unwrap());
        authority.epoch.fetch_add(1, Ordering::SeqCst);
        assert!(!native_allowed(&store, &target, &authority).unwrap());
        assert!(
            native_allowed(
                &store,
                &Target::live(source.0, "synthetic-unblocked"),
                &authority
            )
            .unwrap()
        );
        authority.access.policy.live = false;
        assert!(native_allowed(&store, &target, &authority).unwrap());
        authority.policy_revision.fetch_add(1, Ordering::SeqCst);
        assert!(!native_allowed(&store, &target, &authority).unwrap());
    }

    #[test]
    fn administrative_worker_gate_rejects_expired_and_revoked_snapshots() {
        let mut store = iptv_store_sqlite::Store::open_in_memory().unwrap();
        replace_pin(&mut store, &Pin::parse("2468").unwrap()).unwrap();
        let mut snapshot = Snapshot {
            access: Access::default(),
            epoch: Arc::new(AtomicU64::new(0)),
            at: 0,
            policy_revision: Arc::new(AtomicU64::new(0)),
            policy_at: 0,
        };
        snapshot.access.policy.window = UnlockWindow::Minutes(10);
        snapshot.access.unlock(Target::settings(), 0);
        assert!(require_admin(&store, &snapshot).is_err());
        snapshot.access.unlock(Target::settings(), now_ms());
        assert!(require_admin(&store, &snapshot).is_ok());
        snapshot.epoch.fetch_add(1, Ordering::SeqCst);
        assert!(require_admin(&store, &snapshot).is_err());
    }
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

#[cfg(test)]
mod source_tests {
    use super::*;
    use crate::imports::{self, Job, Purpose};
    use iptv_providers::{
        SecretUrl,
        model::{M3uSourceConfig, Source, SourceConfig},
    };
    fn source(path: &str) -> Source {
        Source {
            name: "Generated source".into(),
            config: SourceConfig::M3u(M3uSourceConfig {
                location: SecretUrl::new(format!("https://example.invalid/{path}.m3u")),
                user_agent: None,
                referrer: None,
            }),
        }
    }
    fn stage(services: &crate::store_service::Services) -> iptv_store_sqlite::CatalogStage {
        let mut stage =
            iptv_store_sqlite::CatalogStage::create_in_with(services.directory(), |path| {
                let path = path.to_owned();
                services.blocking(move |store| store.open_staging_connection(&path))
            })
            .unwrap();
        iptv_import::CatalogSink::finish(&mut stage).unwrap();
        stage
    }
    async fn save(
        services: &crate::store_service::Services,
        job: &Job,
        snapshot: &Snapshot,
    ) -> Result<
        (
            iptv_store_sqlite::SourceId,
            iptv_store_sqlite::CatalogCounts,
            Vec<iptv_store_sqlite::GuideSource>,
        ),
        imports::PublicationError,
    > {
        let worker = services.clone();
        let job = job.clone();
        let snapshot = snapshot.clone();
        services
            .spawn_blocking(move || {
                let publication = imports::Publication::capture(
                    &worker,
                    &job,
                    &iptv_import::CancelToken::new(),
                    &snapshot,
                )?;
                imports::save(&worker, &job, &catalog(), &[], stage(&worker), &publication)
                    .map(|(id, counts, guides, _)| (id, counts, guides))
            })
            .await
            .unwrap()
    }
    fn catalog() -> iptv_import::Catalog {
        iptv_import::Catalog {
            source: iptv_import::SourceMeta::M3u {
                header: Default::default(),
            },
            groups: vec![],
            channels: vec![],
            vod: vec![],
            series: vec![],
            warnings: vec![],
            warnings_dropped: 0,
            on_demand_failures: vec![],
            stats: Default::default(),
        }
    }
    #[tokio::test(flavor = "current_thread")]
    async fn targeted_checks_keep_unrelated_catalog_memberships_out_of_the_read() {
        let directory = tempfile::tempdir().unwrap();
        let owner = std::rc::Rc::new(Runtime::new(crate::Ports::new(
            directory.path().to_owned(),
            |_| Ok(iptv_store_sqlite::DatabaseKey::from_bytes([0x74; 32])),
        )));
        owner.bind_tasks(drop, |_| Box::pin(std::future::pending()));
        owner.start_services();
        let services = owner.services();
        let (target, group, other) = services
            .call(|store| {
                let id = store.add_source(&source("generated-large-membership"))?;
                let mut playlist = String::from("#EXTM3U\n");
                for number in 0..5000 {
                    playlist.push_str(&format!(
                        "#EXTINF:-1 group-title=\"Generated {}\",Generated channel {number}\nhttps://example.test/{number}.ts\n",
                        number % 100
                    ));
                }
                let parsed = iptv_providers::m3u::parse_m3u(&playlist);
                store.replace_catalog(id, &parsed.groups, &parsed.channels.items, &[], &[])?;
                let group = Target {
                    source: id.0,
                    kind: ContentKind::Group,
                    id: parsed.channels.items[0].groups[0].as_str().into(),
                };
                store.set_parental_blocked(&group, true)?;
                Ok((
                    Target::live(id.0, parsed.channels.items[0].id.as_str()),
                    group,
                    Target::live(id.0, parsed.channels.items[1].id.as_str()),
                ))
            })
            .await
            .unwrap();
        owner.parental_check(&target).await.unwrap();
        assert_eq!(owner.parental.borrow().ancestry.len(), 1);
        assert!(owner.parental.borrow().blocked.contains(&group));
        owner
            .set_setting("parental.live_enabled", iptv_settings::Value::Bool(true))
            .unwrap();
        assert_eq!(owner.parental_check(&target).await, Err(Error::Locked));
        assert!(
            !owner.parental_allowed(&other),
            "unloaded protected identities fail closed"
        );
        let removed = group.clone();
        services
            .call(move |store| store.set_parental_blocked(&removed, false))
            .await
            .unwrap();
        owner.parental_check(&target).await.unwrap();
        assert!(!owner.parental.borrow().blocked.contains(&group));
        assert_eq!(owner.parental.borrow().ancestry.len(), 1);
        owner.parental_refresh().await.unwrap();
        assert_eq!(owner.parental.borrow().ancestry.len(), 5000);
        assert!(owner.parental_allowed(&other));
        assert!(owner.drain_services());
    }
    #[tokio::test(flavor = "current_thread")]
    async fn full_refresh_preserves_authority_after_an_empty_targeted_parent_cache_fill() {
        let directory = tempfile::tempdir().unwrap();
        let owner = Runtime::new(crate::Ports::new(directory.path().to_owned(), |_| {
            Ok(iptv_store_sqlite::DatabaseKey::from_bytes([0x79; 32]))
        }));
        owner.bind_tasks(drop, |_| Box::pin(std::future::pending()));
        owner.start_services();
        let id = owner
            .services()
            .call(|store| store.add_source(&source("generated-empty-parent-cache")))
            .await
            .unwrap();
        owner.parental_refresh().await.unwrap();
        let authority = owner.parental_snapshot();
        owner
            .parental_refresh_targets(vec![Target::live(id.0, "generated-ungrouped")])
            .await
            .unwrap();
        assert!(authority.inspection_current(&[]));
        owner.parental_refresh().await.unwrap();
        assert!(
            authority.inspection_current(&[]),
            "empty parent cache entries are not a policy change"
        );
        assert!(owner.drain_services());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn targeted_refresh_invalidates_changed_known_policy_but_not_new_cache_entries() {
        let directory = tempfile::tempdir().unwrap();
        let owner = Runtime::new(crate::Ports::new(directory.path().to_owned(), |_| {
            Ok(iptv_store_sqlite::DatabaseKey::from_bytes([0x78; 32]))
        }));
        owner.bind_tasks(drop, |_| Box::pin(std::future::pending()));
        owner.start_services();
        let services = owner.services();
        let (id, targets, group) = services.call(|store| {
            let id = store.add_source(&source("generated-policy-refresh"))?;
            let parsed = iptv_providers::m3u::parse_m3u(concat!(
                "#EXTM3U\n",
                "#EXTINF:-1 group-title=\"Generated A\",Generated first\nhttps://example.test/first.ts\n",
                "#EXTINF:-1 group-title=\"Generated A\",Generated second\nhttps://example.test/second.ts\n",
                "#EXTINF:-1 group-title=\"Generated B\",Generated third\nhttps://example.test/third.ts\n",
            ));
            store.replace_catalog(id, &parsed.groups, &parsed.channels.items, &[], &[])?;
            let targets: Vec<_> = parsed.channels.items.iter().map(|channel| Target::live(id.0, channel.id.as_str())).collect();
            let group = Target { source: id.0, kind: ContentKind::Group, id: parsed.channels.items[0].groups[0].as_str().into() };
            Ok((id, targets, group))
        }).await.unwrap();
        let first = owner.parental_snapshot();
        owner
            .parental_refresh_targets(vec![targets[0].clone()])
            .await
            .unwrap();
        assert!(
            first.inspection_current(&[]),
            "first cache fill is not a policy change"
        );

        let parent = group.clone();
        services
            .call(move |store| store.set_parental_blocked(&parent, true))
            .await
            .unwrap();
        owner
            .parental_refresh_targets(vec![targets[1].clone()])
            .await
            .unwrap();
        assert!(
            !first.inspection_current(&[]),
            "a known parent's changed block invalidates even on a new child"
        );

        let before_fill = owner.parental_snapshot();
        owner
            .parental_refresh_targets(vec![targets[2].clone()])
            .await
            .unwrap();
        assert!(
            before_fill.inspection_current(&[]),
            "unrelated new unblocked content remains a cache fill"
        );
        owner.parental_content_left(&targets[2]);
        owner
            .parental_refresh_targets(vec![targets[2].clone()])
            .await
            .unwrap();
        assert!(
            before_fill.inspection_current(&[]),
            "ordinary unblocked zapping does not change policy"
        );

        let direct = targets[2].clone();
        services
            .call(move |store| store.set_parental_blocked(&direct, true))
            .await
            .unwrap();
        owner
            .parental_refresh_targets(vec![targets[2].clone()])
            .await
            .unwrap();
        assert!(
            !before_fill.inspection_current(&[]),
            "a known subject's changed block invalidates publication"
        );

        let before_membership = owner.parental_snapshot();
        let member = iptv_providers::ContentId::from_stored(targets[2].id.clone());
        services
            .call(move |store| {
                let group = store.create_custom_group(id, "Generated additional parent")?;
                assert!(store.add_to_custom_group(group, &member)?);
                Ok(())
            })
            .await
            .unwrap();
        owner
            .parental_refresh_targets(vec![targets[2].clone()])
            .await
            .unwrap();
        assert!(
            !before_membership.inspection_current(&[]),
            "changed known ancestry invalidates publication"
        );
        let stable = owner.parental_snapshot();
        owner
            .parental_refresh_targets(vec![targets[2].clone()])
            .await
            .unwrap();
        assert!(
            stable.inspection_current(&[]),
            "unchanged known metadata does not invalidate"
        );
        assert!(owner.drain_services());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn content_unlocks_cannot_mutate_parental_policy_or_sources_for_any_window() {
        let directory = tempfile::tempdir().unwrap();
        let owner = std::rc::Rc::new(Runtime::new(crate::Ports::new(
            directory.path().to_owned(),
            |_| Ok(iptv_store_sqlite::DatabaseKey::from_bytes([0x73; 32])),
        )));
        owner.bind_tasks(drop, |_| Box::pin(std::future::pending()));
        owner.start_services();
        let services = owner.services();
        let id = services
            .call(|store| store.add_source(&source("generated-admin-boundary")))
            .await
            .unwrap();
        owner
            .parental_change_pin(None, Pin::parse("2468").unwrap())
            .await
            .unwrap();
        for window in [
            "every_time",
            "min_10",
            "min_30",
            "hour_1",
            "hour_2",
            "until_restart",
            "until_unblocked_channel",
        ] {
            owner
                .set_setting(
                    "parental.unlock_window",
                    iptv_settings::Value::Enum(window.into()),
                )
                .unwrap();
            owner.parental_lock();
            owner
                .parental_authorize(
                    Target::live(id.0, "generated-channel"),
                    Pin::parse("2468").unwrap(),
                )
                .await
                .unwrap();
            assert!(!owner.parental_settings_allowed(), "{window}");
            assert!(
                owner
                    .set_setting("parental.live_enabled", iptv_settings::Value::Bool(false))
                    .is_err()
            );
            assert!(crate::library::delete_source(&owner, id).await.is_err());
            assert!(
                owner
                    .parental_set_blocked(Target::live(id.0, "generated-channel"), false)
                    .await
                    .is_err()
            );
            owner
                .parental_authorize(Target::settings(), Pin::parse("2468").unwrap())
                .await
                .unwrap();
            assert!(owner.parental_settings_allowed());
        }
        assert!(owner.drain_services());
    }
    #[tokio::test(flavor = "current_thread")]
    async fn queued_unblock_is_revoked_before_the_writer_executes_it() {
        let directory = tempfile::tempdir().unwrap();
        let owner = Runtime::new(crate::Ports::new(directory.path().to_owned(), |_| {
            Ok(iptv_store_sqlite::DatabaseKey::from_bytes([0x72; 32]))
        }));
        owner.start_services();
        let services = owner.services();
        let id = services
            .call(|store| store.add_source(&source("generated-blocked")))
            .await
            .unwrap();
        owner
            .parental_change_pin(None, Pin::parse("2468").unwrap())
            .await
            .unwrap();
        let target = Target {
            source: id.0,
            kind: ContentKind::Movie,
            id: "generated-protected-title".into(),
        };
        owner
            .parental_set_blocked(target.clone(), true)
            .await
            .unwrap();
        let (started, ready) = std::sync::mpsc::channel();
        let (release, held) = std::sync::mpsc::channel();
        let writer = services.call(move |_| {
            started.send(()).unwrap();
            held.recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            Ok(())
        });
        ready
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        owner.parental.borrow_mut().ready = false;
        let mut unblock = std::pin::pin!(owner.parental_set_blocked(target.clone(), false));
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        while !owner.parental_is_ready() {
            tokio::select! {
                result = &mut unblock => panic!("the held writer completed early: {result:?}"),
                _ = tokio::time::sleep(std::time::Duration::from_millis(1)) => {}
            }
            assert!(tokio::time::Instant::now() < deadline);
        }
        // The read lane admitted the request; it is now queued behind the writer.
        owner.parental_lock();
        release.send(()).unwrap();
        writer.await.unwrap();
        assert!(
            unblock.await.is_err(),
            "relocking must revoke a queued unblock"
        );
        assert!(
            services
                .read(move |store| Ok(store.parental_blocks()?.contains(&target)))
                .await
                .unwrap()
        );
        assert!(owner.drain_services());
    }
    #[tokio::test(flavor = "current_thread")]
    async fn native_import_admission_and_commit_bind_configuration_to_admin_authority() {
        let directory = tempfile::tempdir().unwrap();
        let owner = Runtime::new(crate::Ports::new(directory.path().to_owned(), |_| {
            Ok(iptv_store_sqlite::DatabaseKey::from_bytes([0x71; 32]))
        }));
        owner.start_services();
        let services = owner.services();
        let initial = source("generated-original");
        let saved = initial.clone();
        let id = services
            .call(move |store| store.add_source(&saved))
            .await
            .unwrap();
        owner
            .parental_change_pin(None, Pin::parse("2468").unwrap())
            .await
            .unwrap();
        let authorized_before_lock = owner.parental_snapshot();
        owner.parental_lock();
        let locked = owner.parental_snapshot();
        let add = Job {
            source: source("generated-add"),
            epg_url: None,
            stored: None,
            purpose: Purpose::Background,
            edit: false,
        };
        assert!(save(&services, &add, &locked).await.is_err());
        assert!(
            save(&services, &add, &authorized_before_lock)
                .await
                .is_err()
        );
        let forged = Job {
            source: source("generated-replacement"),
            stored: Some(id),
            ..add.clone()
        };
        // Neither a claimed background purpose nor edit=false can launder a
        // different provider into an already authorized source's identity.
        assert!(save(&services, &forged, &locked).await.is_err());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let worker = services.clone();
        let admission = locked.clone();
        services
            .spawn_blocking(move || {
                worker.block_on(imports::pipeline(
                    &worker,
                    forged,
                    &tx,
                    &iptv_import::CancelToken::new(),
                    &iptv_import::CancelToken::new(),
                    &admission,
                ))
            })
            .await
            .unwrap();
        assert!(
            matches!(rx.try_recv(),Ok(imports::Msg::Playlist(Err(failure)))
            if failure.kind==iptv_core::wizard::FailureKind::Auth)
        );
        assert_eq!(
            services
                .read(|store| Ok(store.sources()?.len()))
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            services
                .read(move |store| Ok(store.source(id)?.unwrap().source.config))
                .await
                .unwrap(),
            initial.config
        );
        // Exact existing configurations continue refreshing after viewing locks.
        let refresh = Job {
            source: initial,
            stored: Some(id),
            purpose: Purpose::Background,
            edit: false,
            ..add.clone()
        };
        save(&services, &refresh, &locked).await.unwrap();
        owner
            .parental_authorize(Target::settings(), Pin::parse("2468").unwrap())
            .await
            .unwrap();
        let authorized = owner.parental_snapshot();
        let added = save(&services, &add, &authorized).await.unwrap().0;
        // Deletion queues eagerly, so hold the writer until its captured grant
        // has been revoked. Otherwise a fast worker may legitimately delete the
        // source before the lock, making this assertion a scheduling race.
        let (started, ready) = std::sync::mpsc::channel();
        let (release, held) = std::sync::mpsc::channel();
        let writer = services.call(move |_| {
            started.send(()).unwrap();
            held.recv_timeout(std::time::Duration::from_secs(30))
                .unwrap();
            Ok(())
        });
        ready
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        let pending = crate::library::delete_source(&owner, added);
        owner.parental_lock();
        release.send(()).unwrap();
        writer.await.unwrap();
        assert!(pending.await.is_err());
        assert!(
            services
                .read(move |store| Ok(store.source(added)?.is_some()))
                .await
                .unwrap(),
            "a rejected queued deletion must retain its source"
        );
        owner
            .parental_authorize(Target::settings(), Pin::parse("2468").unwrap())
            .await
            .unwrap();
        assert!(crate::library::delete_source(&owner, added).await.unwrap());
        assert!(owner.drain_services());
    }
}
