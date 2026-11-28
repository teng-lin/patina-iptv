//! Authoritative parental access and credential operations. PIN hashing and
//! durable retry accounting run only on the store worker, never the UI thread.
pub(crate) mod gate;

use crate::Runtime;
pub use gate::Error;
#[allow(unused_imports)]
pub(crate) use gate::SETTINGS_LOCKED_REASON;
pub(crate) use gate::{Snapshot, TransferredGroups, now_ms, policy_settings};
use iptv_core::parental::{Access, ContentKind, Pin, Policy, Target, UnlockWindow};
use iptv_library::SourceId;
use std::{
    collections::BTreeSet,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use zeroize::Zeroizing;

#[derive(Default)]
pub(crate) struct State {
    pub access: Access,
    pub ready: bool,
    pub has_pin: bool,
    complete: bool,
    epoch: Arc<AtomicU64>,
    policy_revision: Arc<AtomicU64>,
    transferred_groups: Option<TransferredGroups>,
    blocked: BTreeSet<Target>,
    ancestry: std::collections::BTreeMap<Target, Vec<Target>>,
    names: std::collections::BTreeMap<Target, String>,
}

impl State {
    fn lock(&mut self) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        self.access.lock();
        self.transferred_groups = None;
    }

    fn clear_transferred_groups(&mut self, channel: &Target) -> bool {
        if self
            .transferred_groups
            .as_ref()
            .is_some_and(|transfer| transfer.channel == *channel)
        {
            self.transferred_groups = None;
            return true;
        }
        false
    }

    fn granted_for(&self, channel: &Target, grant: &Target, now: u64) -> bool {
        self.access.granted(grant, now)
            || TransferredGroups::allows(self.transferred_groups.as_ref(), channel, grant)
    }

    fn transfer_groups(&self, channel: &Target, now: u64) -> Option<TransferredGroups> {
        let groups: Vec<_> = self
            .ancestry
            .get(channel)?
            .iter()
            .filter(|group| self.blocked.contains(*group) && self.granted_for(channel, group, now))
            .cloned()
            .collect();
        (!groups.is_empty()).then(|| TransferredGroups {
            channel: channel.clone(),
            groups,
        })
    }
}

fn apply_policy(state: &mut State, new: Policy) {
    if state.access.policy == new {
        return;
    }
    let settings = state.access.granted(&Target::settings(), now_ms());
    state.policy_revision.fetch_add(1, Ordering::SeqCst);
    state.lock();
    state.access.policy = new;
    if settings {
        state.access.unlock(Target::settings(), now_ms());
    }
}
fn policy(owner: &Runtime) -> Policy {
    policy_settings(&owner.settings())
}
impl Runtime {
    /// Refresh public policy metadata without returning any credential verifier.
    pub async fn parental_refresh(&self) -> Result<(), Error> {
        let services = self.services();
        let snapshot = services
            .parental()
            .catalog()
            .await
            .map_err(|_| Error::Storage)?;
        if !services.is_current() {
            return Err(Error::OwnerClosed);
        }
        let mut state = self.parental.borrow_mut();
        state.has_pin = snapshot.has_pin;
        state.ready = true;
        let blocked: BTreeSet<_> = snapshot.blocked.into_iter().collect();
        let mut ancestry = snapshot.ancestry;
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
        state.names = snapshot.names;
        apply_policy(&mut state, policy(self));
        Ok(())
    }
    /// Update authorization for a bounded set of content identities. The full
    /// catalog snapshot is reserved for boot, catalog changes and management.
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
            .parental()
            .targets(targets, guide_channel)
            .await
            .map_err(|_| Error::Storage)?;
        if !services.is_current() {
            return Err(Error::OwnerClosed);
        }
        let mut state = self.parental.borrow_mut();
        state.has_pin = result.has_pin;
        state.ready = true;
        let mut changed = false;
        for entry in result.entries {
            let target = entry.target;
            let parents = entry.parents;
            let blocked = entry.blocked;
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
        Ok(result.guide_blocked)
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
                    .any(|p| state.blocked.contains(p) && !state.granted_for(target, p, now_ms()))
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
        services
            .parental()
            .verify_pin(pin, epoch.clone(), expected, now)
            .await?;
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
        let result = services.parental().change_pin(old, new, epoch, now).await?;
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
            .parental()
            .recover_pin(recovery, new, epoch, now)
            .await?;
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
        state.clear_transferred_groups(target);
        state.epoch.fetch_add(1, Ordering::SeqCst);
    }
    pub fn parental_content_closed(&self) {
        self.clear_per_use_grants(None);
    }
    pub fn parental_backgrounded(&self) {
        self.clear_per_use_grants(None);
    }
    /// Keeps per-use authorization for the same live subject during an
    /// external handoff, revoking all other per-use grants.
    pub fn parental_content_transferred(&self, target: &Target) {
        self.clear_per_use_grants(Some(target));
    }
    /// Revokes the transferred per-use grant when external playback ends.
    pub fn parental_end_transfer(&self, target: &Target) {
        let mut state = self.parental.borrow_mut();
        if state.access.policy.window == UnlockWindow::EveryTime {
            let had_groups = state.clear_transferred_groups(target);
            if had_groups || state.access.granted(target, now_ms()) {
                state.access.content_left(target);
                state.epoch.fetch_add(1, Ordering::SeqCst);
            }
        }
    }
    fn clear_per_use_grants(&self, preserve: Option<&Target>) {
        let mut state = self.parental.borrow_mut();
        if state.access.policy.window != UnlockWindow::EveryTime {
            return;
        }
        let now = now_ms();
        let preserve = preserve.filter(|target| target.kind == ContentKind::Live);
        let keep = preserve.filter(|target| state.access.granted(target, now));
        let groups = preserve
            .filter(|_| keep.is_none())
            .and_then(|target| state.transfer_groups(target, now));
        state.lock();
        state.transferred_groups = groups;
        if let Some(target) = keep {
            state.access.unlock(target.clone(), now);
        }
    }
    pub(crate) fn parental_snapshot(&self) -> Snapshot {
        let mut state = self.parental.borrow_mut();
        apply_policy(&mut state, policy(self));
        Snapshot {
            access: state.access.clone(),
            transferred_groups: state.transferred_groups.clone(),
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
        self.parental.borrow_mut().lock();
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
            .parental()
            .set_blocked(target, blocked, access)
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

#[cfg(all(test, feature = "native"))]
mod tests {
    use super::*;
    use crate::error::AdapterError as StoreError;
    use crate::store_service::ParentalRepo;
    use iptv_store_sqlite::Store;

    fn native_allowed(
        store: &Store,
        target: &Target,
        snapshot: &Snapshot,
    ) -> Result<bool, StoreError> {
        ParentalRepo::native_allowed(store, target, snapshot)
    }

    fn replace_pin(
        store: &mut Store,
        pin: &Pin,
        entropy: crate::ports::Entropy,
    ) -> Result<Zeroizing<String>, Error> {
        ParentalRepo::replace_pin(store, pin, entropy)
    }

    fn verify(store: &mut Store, secret: &[u8], recovery: bool, now: i64) -> Result<(), Error> {
        ParentalRepo::verify(store, secret, recovery, now)
    }

    fn require_admin(store: &Store, snapshot: &Snapshot) -> Result<(), StoreError> {
        ParentalRepo::require_admin(store, snapshot)
    }

    #[test]
    fn external_transfer_keeps_only_the_existing_per_use_grant() {
        let directory = tempfile::tempdir().unwrap();
        let owner = Runtime::new(crate::Ports::new(directory.path().to_owned(), |_| {
            Ok(iptv_store_sqlite::DatabaseKey::from_bytes([0x31; 32]))
        }));
        let channel = Target::live(1, "synthetic-channel");
        let other = Target::live(1, "synthetic-other");
        {
            let mut state = owner.parental.borrow_mut();
            state.access.policy.window = UnlockWindow::EveryTime;
            state.access.unlock(channel.clone(), now_ms());
            state.access.unlock(other.clone(), now_ms());
            state.access.unlock(Target::settings(), now_ms());
        }

        owner.set_foreground_for_external(false, Some(&channel));
        owner.parental_content_transferred(&channel);
        {
            let state = owner.parental.borrow();
            assert!(state.access.granted(&channel, now_ms()));
            assert!(!state.access.granted(&other, now_ms()));
            assert!(!state.access.granted(&Target::settings(), now_ms()));
        }

        owner.parental_end_transfer(&channel);
        owner.parental_content_transferred(&channel);
        assert!(!owner.parental.borrow().access.granted(&channel, now_ms()));
    }

    fn snapshot() -> Snapshot {
        Snapshot {
            access: Access::default(),
            transferred_groups: None,
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
        replace_pin(
            &mut store,
            &Pin::parse("2468").unwrap(),
            crate::native::entropy,
        )
        .unwrap();
        let mut snapshot = Snapshot {
            access: Access::default(),
            transferred_groups: None,
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
        let code = replace_pin(&mut store, &pin, crate::native::entropy).unwrap();
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
        let new_code = replace_pin(
            &mut store,
            &Pin::parse("8642").unwrap(),
            crate::native::entropy,
        )
        .unwrap();
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

#[cfg(all(test, feature = "native"))]
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
    async fn external_transfer_scopes_all_required_group_grants_to_the_chosen_channel() {
        let directory = tempfile::tempdir().unwrap();
        let owner = std::rc::Rc::new(Runtime::new(crate::Ports::new(
            directory.path().to_owned(),
            |_| Ok(iptv_store_sqlite::DatabaseKey::from_bytes([0x77; 32])),
        )));
        owner.bind_tasks(drop, |_| Box::pin(std::future::pending()));
        owner.start_services();
        let services = owner.services();
        let (authority, sibling, groups) = services.call(|store| {
            let id = store.add_source(&source("generated-group-handoff"))?;
            let mut parsed = iptv_providers::m3u::parse_m3u(concat!(
                "#EXTM3U\n",
                "#EXTINF:-1 group-title=\"Alpha\",Selected channel\nhttps://example.test/selected.ts\n",
                "#EXTINF:-1 group-title=\"Alpha\",Sibling channel\nhttps://example.test/sibling.ts\n",
                "#EXTINF:-1 group-title=\"Beta\",Separate channel\nhttps://example.test/separate.ts\n",
            ));
            let beta = parsed.channels.items[2].groups[0].clone();
            parsed.channels.items[0].groups.push(beta);
            store.replace_catalog(id, &parsed.groups, &parsed.channels.items, &[], &[])?;
            let selected = &parsed.channels.items[0];
            let groups: Vec<_> = selected.groups.iter().map(|group| Target {
                source: id.0,
                kind: ContentKind::Group,
                id: group.as_str().into(),
            }).collect();
            for group in &groups {
                store.set_parental_blocked(group, true)?;
            }
            store.set_guide_group_preference(
                id,
                &groups[0].id,
                iptv_core::organization::GroupPreference::ExternalPlayer(true),
            )?;
            let stored = store.source(id)?.unwrap();
            let authority = iptv_guide::playback::SourceAuthority::new(id.0, stored.incarnation, stored.source.config);
            let sibling = Target::live(id.0, parsed.channels.items[1].id.as_str());
            Ok((authority.resolve(selected), sibling, groups))
        }).await.unwrap();
        let target = Target::live(authority.source(), authority.channel().id.as_str());
        owner.parental_refresh().await.unwrap();
        owner
            .set_setting("parental.live_enabled", iptv_settings::Value::Bool(true))
            .unwrap();
        {
            let mut state = owner.parental.borrow_mut();
            assert_eq!(state.access.policy.window, UnlockWindow::EveryTime);
            state.access.unlock(groups[0].clone(), now_ms());
        }
        assert!(
            owner
                .authorize_external_playback(authority.clone(), groups[0].id.clone())
                .await
                .unwrap()
                .is_none(),
            "both blocked parents must be granted"
        );
        {
            let mut state = owner.parental.borrow_mut();
            state.access.unlock(groups[1].clone(), now_ms());
            state.access.unlock(Target::settings(), now_ms());
        }
        assert!(owner.parental_allowed(&target));
        assert!(owner.parental_allowed(&sibling));
        // Android's chooser takes the app to the background before retiring
        // native playback. The same grants must survive both transitions.
        owner.set_foreground_for_external(false, Some(&target));
        owner.parental_content_transferred(&target);
        assert!(owner.parental_allowed(&target));
        assert!(!owner.parental_allowed(&sibling));
        let snapshot = owner.parental_snapshot();
        {
            let state = owner.parental.borrow();
            assert!(!state.access.granted(&target, now_ms()));
            assert!(!state.access.granted(&Target::settings(), now_ms()));
            assert!(
                groups
                    .iter()
                    .all(|group| !state.access.granted(group, now_ms()))
            );
        }
        assert!(!snapshot.inspection_current(&groups));
        let admitted = owner
            .authorize_external_playback(authority.clone(), groups[0].id.clone())
            .await
            .unwrap()
            .unwrap();
        assert!(admitted.is_current());
        assert!(
            owner
                .external_playback_allowed(authority.clone(), groups[0].id.clone())
                .await
                .unwrap()
        );

        // A new direct block still requires a direct channel unlock, even
        // before this change has been reflected in the UI cache.
        let block = target.clone();
        services
            .call(move |store| store.set_parental_blocked(&block, true))
            .await
            .unwrap();
        assert!(
            owner
                .authorize_external_playback(authority.clone(), groups[0].id.clone())
                .await
                .unwrap()
                .is_none()
        );
        let block = target.clone();
        services
            .call(move |store| store.set_parental_blocked(&block, false))
            .await
            .unwrap();
        owner.parental_end_transfer(&target);
        assert!(!admitted.is_current());
        assert!(!owner.parental_allowed(&target));
        assert!(
            !owner
                .external_playback_allowed(authority, groups[0].id.clone())
                .await
                .unwrap()
        );
        assert!(owner.drain_services());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn external_admission_rejects_committed_blocks_before_cache_refresh() {
        let directory = tempfile::tempdir().unwrap();
        let owner = std::rc::Rc::new(Runtime::new(crate::Ports::new(
            directory.path().to_owned(),
            |_| Ok(iptv_store_sqlite::DatabaseKey::from_bytes([0x79; 32])),
        )));
        owner.bind_tasks(drop, |_| Box::pin(std::future::pending()));
        owner.start_services();
        let services = owner.services();
        let (authority, group) = services
            .call(|store| {
                let id = store.add_source(&source("generated-external-admission"))?;
                let parsed = iptv_providers::m3u::parse_m3u(concat!(
                    "#EXTM3U\n",
                    "#EXTINF:-1 group-title=\"Generated external group\",Generated channel\n",
                    "https://example.test/channel.ts\n",
                ));
                store.replace_catalog(id, &parsed.groups, &parsed.channels.items, &[], &[])?;
                let channel = &parsed.channels.items[0];
                let group = channel.groups[0].as_str().to_owned();
                store.set_guide_group_preference(
                    id,
                    &group,
                    iptv_core::organization::GroupPreference::ExternalPlayer(true),
                )?;
                let stored = store.source(id)?.unwrap();
                let authority = iptv_guide::playback::SourceAuthority::new(
                    id.0,
                    stored.incarnation,
                    stored.source.config,
                );
                Ok((authority.resolve(channel), group))
            })
            .await
            .unwrap();
        let target = Target::live(authority.source(), authority.channel().id.as_str());
        owner.parental_check(&target).await.unwrap();
        owner
            .set_setting("parental.live_enabled", iptv_settings::Value::Bool(true))
            .unwrap();
        let approved = owner
            .authorize_external_playback(authority.clone(), group.clone())
            .await
            .unwrap()
            .unwrap();
        assert!(approved.is_current());

        let block = Target {
            source: target.source,
            kind: ContentKind::Group,
            id: group.clone(),
        };
        // Mirror the policy change's writer and deliberately hold back the
        // separate cache refresh until after checking admission.
        owner
            .parental
            .borrow()
            .policy_revision
            .fetch_add(1, Ordering::SeqCst);
        assert!(!approved.is_current());
        let queued_block = block.clone();
        let pending_block =
            services.call(move |store| store.set_parental_blocked(&queued_block, true));
        assert!(
            owner
                .authorize_external_playback(authority.clone(), group.clone())
                .await
                .unwrap()
                .is_none()
        );
        pending_block.await.unwrap();
        assert!(
            owner.parental_allowed(&target),
            "cache still allows the channel"
        );
        owner.parental.borrow_mut().access.unlock(block, now_ms());
        let granted = owner
            .authorize_external_playback(authority, group)
            .await
            .unwrap()
            .unwrap();
        assert!(granted.is_current());
        owner.parental_lock();
        assert!(!granted.is_current());
        assert!(
            owner.parental_allowed(&target),
            "cache still lacks the committed block"
        );
        assert!(owner.drain_services());
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
            if failure.kind==iptv_core::add_playlist::FailureKind::Auth)
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
