//! A bounded analysis snapshot produces an immutable, owner-bound repair plan.
use super::*;
use iptv_epg::{ChannelKey, FeedId, MatchIndex, Overrides, PlaylistChannel, XmltvChannel};
use iptv_library::inspection::{ProbeEvidence, ProbeId};
use iptv_providers::ContentId;
use iptv_store_sqlite::{
    EpgOverride, GuideSource, RepairBatch, RepairBinding, RepairCandidate, RepairChannel,
    RepairChoice, RepairFeed, RepairMapping, Store,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};

fn storage<T>(result: iptv_store_sqlite::Result<T>) -> Result<T> {
    result.map_err(|_| Error::Storage)
}
fn check(cancel: &CancelToken, deadline: Instant) -> Result<()> {
    if cancel.is_canceled() {
        return Err(Error::Canceled);
    }
    if Instant::now() >= deadline {
        return Err(InspectionError::Deadline.into());
    }
    Ok(())
}

fn old_choice(value: Option<&EpgOverride>) -> Choice {
    match value {
        None => Choice::Automatic,
        Some(EpgOverride::Disabled) => Choice::NoGuide,
        Some(value @ EpgOverride::Xmltv { epg_channel, .. }) => value
            .feed()
            .map(|feed| Choice::Channel {
                feed,
                epg_channel: epg_channel.clone(),
            })
            .unwrap_or(Choice::Automatic),
    }
}
fn lease<'a>(
    leases: &'a [ProbeLease],
    evidence: &iptv_library::inspection::ProbeEvidence,
) -> Result<&'a ProbeLease> {
    leases
        .iter()
        .find(|l| l.evidence() == evidence)
        .ok_or(Error::Inspection(InspectionError::StaleEvidence))
}
fn valid_candidate(store: &Store, key: &CandidateRef) -> Result<()> {
    if storage(store.feed_incarnation(key.source, key.feed))?.as_ref() != Some(&key.incarnation) {
        return Err(Error::Inspection(InspectionError::StaleEvidence));
    }
    Ok(())
}
fn matched(
    index: &MatchIndex,
    rows: &[iptv_store_sqlite::RepairChannel],
    overrides: &Overrides,
) -> iptv_epg::MatchResult {
    let keys: Vec<_> = rows
        .iter()
        .map(|r| ChannelKey::new(r.key.as_str()))
        .collect();
    index.match_channels(
        rows.iter()
            .zip(keys.iter())
            .map(|(r, key)| PlaylistChannel {
                key,
                tvg_id: r.epg_id.as_deref(),
                name: &r.provider_name,
                tvg_name: r.epg_name.as_deref(),
            }),
        overrides,
    )
}
fn index(config: &[GuideSource], channels: &HashMap<FeedId, Vec<XmltvChannel>>) -> MatchIndex {
    let mut index = MatchIndex::new();
    for (position, feed) in config.iter().enumerate().filter(|(_, f)| f.enabled) {
        if let Some(channels) = channels.get(&feed.feed) {
            index.add_source(feed.feed, -(position as i32), channels);
        }
    }
    index
}

pub(super) struct Preparation<'a> {
    pub epoch: u64,
    pub id: String,
    pub created_at: i64,
    pub cancel: &'a CancelToken,
    pub deadline: Instant,
}

struct Evidence {
    expires_at: i64,
    staged_channels: BTreeMap<ProbeId, Vec<XmltvChannel>>,
}

struct FeedPlan {
    after_configs: Vec<GuideSource>,
    changes: Vec<RepairFeed>,
    previews: Vec<FeedPreview>,
    order: Option<(Vec<FeedId>, Vec<FeedId>)>,
    additions: Vec<GuideSource>,
    staged_feeds: BTreeMap<FeedId, (usize, ProbeEvidence)>,
}

/// Apply validated feed edits and preserve the complete ordering preview.
fn prepare_feed_edits(request: &Request, configs: &[GuideSource]) -> Result<FeedPlan> {
    let mut after_configs = configs.to_vec();
    let mut changes = Vec::new();
    let mut previews = Vec::new();
    let mut seen = HashSet::new();
    for edit in &request.feeds {
        if !seen.insert(edit.feed)
            || edit.remove && (edit.enabled.is_some() || edit.shift_minutes.is_some())
            || edit
                .shift_minutes
                .is_some_and(|shift| shift.unsigned_abs() > 24 * 60)
        {
            return Err(Error::InvalidRequest);
        }
        let before = configs
            .iter()
            .find(|feed| feed.feed == edit.feed)
            .ok_or(Error::InvalidRequest)?
            .clone();
        let at = after_configs
            .iter()
            .position(|feed| feed.feed == edit.feed)
            .ok_or(Error::InvalidRequest)?;
        let after = if edit.remove {
            after_configs.remove(at);
            None
        } else {
            let mut updated = before.clone();
            if let Some(enabled) = edit.enabled {
                updated.enabled = enabled;
            }
            if let Some(shift) = edit.shift_minutes {
                updated.shift_minutes = shift;
            }
            after_configs[at] = updated.clone();
            Some(updated)
        };
        previews.push(FeedPreview {
            feed: Some(edit.feed),
            name: before.name.clone().unwrap_or_default(),
            before_position: configs
                .iter()
                .position(|f| f.feed == edit.feed)
                .map(|v| v as u32),
            after_position: after_configs
                .iter()
                .position(|f| f.feed == edit.feed)
                .map(|v| v as u32),
            before_enabled: Some(before.enabled),
            after_enabled: after.as_ref().map(|f| f.enabled),
            before_shift_minutes: Some(before.shift_minutes),
            after_shift_minutes: after.as_ref().map(|f| f.shift_minutes),
            added: false,
            removed: after.is_none(),
        });
        changes.push(RepairFeed {
            before: Some(before),
            after,
        });
    }
    let order = if let Some(order) = &request.feed_order {
        let before: Vec<_> = configs.iter().map(|f| f.feed).collect();
        let unique: HashSet<_> = order.iter().copied().collect();
        if unique.len() != order.len()
            || unique != before.iter().copied().collect()
            || request.feeds.iter().any(|f| f.remove)
        {
            return Err(Error::InvalidRequest);
        }
        let by_id: HashMap<_, _> = after_configs.into_iter().map(|f| (f.feed, f)).collect();
        after_configs = order.iter().map(|id| by_id[id].clone()).collect();
        Some((before, order.clone()))
    } else {
        None
    };
    for (position, config) in after_configs.iter().enumerate() {
        let before_position = configs
            .iter()
            .position(|feed| feed.feed == config.feed)
            .map(|v| v as u32);
        if let Some(preview) = previews.iter_mut().find(|p| p.feed == Some(config.feed)) {
            preview.after_position = Some(position as u32);
        } else if before_position != Some(position as u32) {
            previews.push(FeedPreview {
                feed: Some(config.feed),
                name: config.name.clone().unwrap_or_default(),
                before_position,
                after_position: Some(position as u32),
                before_enabled: Some(config.enabled),
                after_enabled: Some(config.enabled),
                before_shift_minutes: Some(config.shift_minutes),
                after_shift_minutes: Some(config.shift_minutes),
                added: false,
                removed: false,
            });
        }
    }
    Ok(FeedPlan {
        after_configs,
        changes,
        previews,
        order,
        additions: Vec::new(),
        staged_feeds: BTreeMap::new(),
    })
}

struct PreparationSession<'a> {
    store: &'a Store,
    request: &'a Request,
    leases: &'a [ProbeLease],
    cancel: &'a CancelToken,
    deadline: Instant,
}

struct IndexedFeeds {
    configs: Vec<GuideSource>,
    before: HashMap<FeedId, Vec<XmltvChannel>>,
    after: HashMap<FeedId, Vec<XmltvChannel>>,
    fresh: HashSet<FeedId>,
}

struct ChannelSnapshot {
    rows: Vec<RepairChannel>,
    bindings: HashMap<ContentId, RepairBinding>,
    available: HashSet<(FeedId, String)>,
    overrides: Overrides,
}

struct MappingPlan {
    changes: Vec<RepairMapping>,
    previews: Vec<MappingPreview>,
    candidates: Vec<RepairCandidate>,
    explicit: HashSet<ContentId>,
    overrides: Overrides,
}

struct Impact {
    rows: Vec<IndirectEffect>,
    complete: bool,
}

impl PreparationSession<'_> {
    /// Verify every admitted lease before using its staged channel rows.
    fn evidence(&self, incarnation: &str, created_at: i64) -> Result<Evidence> {
        let Self {
            store,
            request,
            leases,
            cancel,
            deadline,
        } = *self;
        let source = request.source;
        let mut expires_at = created_at.saturating_add(PLAN_TTL_SECONDS);
        for held in leases {
            held.validate_alive()?;
            if held.source_incarnation() != incarnation || held.evidence().source != source {
                return Err(InspectionError::StaleEvidence.into());
            }
            if let Some(feed) = held.evidence().feed {
                valid_candidate(
                    store,
                    &CandidateRef {
                        source,
                        feed,
                        incarnation: held.evidence().incarnation.clone(),
                        epg_channel: String::new(),
                    },
                )?;
            }
            expires_at = expires_at.min(held.evidence().expires_at);
        }
        let mut staged_channels = BTreeMap::new();
        for held in leases {
            check(cancel, deadline)?;
            let chosen: Vec<_> = request
                .mappings
                .iter()
                .filter_map(|mapping| match &mapping.target {
                    Target::Staged {
                        evidence,
                        epg_channel,
                        ..
                    } if evidence == held.evidence() => Some(CandidateRef {
                        source,
                        feed: evidence.feed.unwrap_or(FeedId(0)),
                        incarnation: evidence.incarnation.clone(),
                        epg_channel: epg_channel.clone(),
                    }),
                    _ => None,
                })
                .collect();
            held.validate_candidates_with_control(&chosen, cancel, deadline)?;
            staged_channels.insert(
                held.evidence().stage.clone(),
                held.listed_channels_with_control(cancel, deadline)?,
            );
        }
        Ok(Evidence {
            expires_at,
            staged_channels,
        })
    }

    /// Reads configured guide indexes and overlays the admitted staged evidence.
    fn index_feeds(&self, evidence: &Evidence) -> Result<IndexedFeeds> {
        let store = self.store;
        let source = self.request.source;
        let configs = storage(store.guide_sources(source))?;
        let listed =
            storage(store.guide_repair_listed(source))?.ok_or(InspectionError::LimitExceeded)?;
        if listed.len() > 100_000 {
            return Err(InspectionError::LimitExceeded.into());
        }
        let mut channels: HashMap<FeedId, Vec<XmltvChannel>> = HashMap::new();
        for row in listed {
            channels.entry(row.feed).or_default().push(XmltvChannel {
                id: row.epg_channel,
                display_names: row.name.into_iter().collect(),
                icon: None,
            });
        }
        let mut fresh_feeds = HashSet::new();
        for config in &configs {
            check(self.cancel, self.deadline)?;
            if storage(store.inspection_stored_feed_incarnation(source, config.feed))?.is_some() {
                fresh_feeds.insert(config.feed);
            } else {
                channels.remove(&config.feed);
            }
        }
        let before_channels = channels.clone();
        // Staging supplies all display names and candidates, including entries the
        // normal filtered import did not retain programs for.
        for held in self.leases {
            if let Some(feed) = held.evidence().feed {
                channels.insert(
                    feed,
                    evidence.staged_channels[&held.evidence().stage].clone(),
                );
                fresh_feeds.insert(feed);
            }
        }
        Ok(IndexedFeeds {
            configs,
            before: before_channels,
            after: channels,
            fresh: fresh_feeds,
        })
    }

    /// Reads the stored channel snapshot and its actual guide bindings.
    fn snapshot_channels(&self) -> Result<ChannelSnapshot> {
        let store = self.store;
        let source = self.request.source;
        let inputs =
            storage(store.guide_repair_inputs(source))?.ok_or(InspectionError::LimitExceeded)?;
        let rows = inputs.channels;
        let actual: HashMap<_, _> = inputs
            .bindings
            .into_iter()
            .map(|binding| (binding.key.clone(), binding))
            .collect();
        let available: HashSet<_> = inputs.available.into_iter().collect();
        if rows.len() > 100_000 {
            return Err(InspectionError::LimitExceeded.into());
        }
        let before_overrides: Overrides = rows
            .iter()
            .filter_map(|row| {
                row.epg_override
                    .as_ref()
                    .map(|value| (ChannelKey::new(row.key.as_str()), value.to_override()))
            })
            .collect();
        Ok(ChannelSnapshot {
            rows,
            bindings: actual,
            available,
            overrides: before_overrides,
        })
    }

    /// Appends requested staged feeds using matching-only identities.
    fn add_feeds(
        &self,
        evidence: &Evidence,
        indexed: &mut IndexedFeeds,
        feeds: &mut FeedPlan,
    ) -> Result<()> {
        let store = self.store;
        let request = self.request;
        let source = request.source;
        if request.additions.is_empty() {
            return Ok(());
        }
        let feed_floor = storage(store.guide_repair_feed_high_water(source))?;
        for (position, addition) in request.additions.iter().enumerate() {
            if addition.evidence.feed.is_some()
                || addition.name.as_ref().is_some_and(|n| n.len() > 256)
            {
                return Err(Error::InvalidRequest);
            }
            let held = lease(self.leases, &addition.evidence)?;
            // A private matching-only ID is translated back to a staged Target in
            // the preview. It is never persisted or exposed as a configured feed.
            let temporary = FeedId(u32::MAX - position as u32);
            if temporary.0 <= feed_floor {
                return Err(Error::InvalidRequest);
            }
            let config = GuideSource {
                feed: temporary,
                provider_index: None,
                url: held.location(),
                name: addition.name.clone(),
                enabled: true,
                shift_minutes: 0,
            };
            indexed.after.insert(
                temporary,
                evidence.staged_channels[&held.evidence().stage].clone(),
            );
            feeds
                .staged_feeds
                .insert(temporary, (position, addition.evidence.clone()));
            feeds.after_configs.push(config.clone());
            feeds.additions.push(config);
            feeds.previews.push(FeedPreview {
                feed: None,
                name: addition.name.clone().unwrap_or_default(),
                before_position: None,
                after_position: Some((feeds.after_configs.len() - 1) as u32),
                before_enabled: None,
                after_enabled: Some(true),
                before_shift_minutes: None,
                after_shift_minutes: Some(0),
                added: true,
                removed: false,
            });
        }
        Ok(())
    }

    fn desired_choice(
        &self,
        target: &Target,
        verified: &Evidence,
        indexed: &IndexedFeeds,
        candidates: &mut Vec<RepairCandidate>,
    ) -> Result<RepairChoice> {
        let store = self.store;
        let request = self.request;
        let source = request.source;
        Ok(match target {
            Target::Automatic => RepairChoice::Automatic,
            Target::NoGuide => RepairChoice::Disabled,
            Target::Configured(key) => {
                if key.source != source {
                    return Err(Error::InvalidRequest);
                }
                valid_candidate(store, key)?;
                if storage(store.inspection_stored_feed_incarnation(source, key.feed))?.as_ref()
                    != Some(&key.incarnation)
                {
                    return Err(InspectionError::StaleEvidence.into());
                }
                if !indexed
                    .after
                    .get(&key.feed)
                    .is_some_and(|list| list.iter().any(|c| c.id == key.epg_channel))
                {
                    return Err(InspectionError::StaleEvidence.into());
                }
                candidates.push(RepairCandidate {
                    feed: key.feed,
                    incarnation: key.incarnation.clone(),
                    epg_channel: key.epg_channel.clone(),
                    staged: false,
                });
                RepairChoice::Existing {
                    feed: key.feed,
                    epg_channel: key.epg_channel.clone(),
                }
            }
            Target::Staged {
                evidence,
                epg_channel,
                addition,
            } => {
                let held = lease(self.leases, evidence)?;
                let candidate = CandidateRef {
                    source,
                    feed: evidence.feed.unwrap_or(FeedId(0)),
                    incarnation: evidence.incarnation.clone(),
                    epg_channel: epg_channel.clone(),
                };
                if !verified
                    .staged_channels
                    .get(&held.evidence().stage)
                    .is_some_and(|rows| rows.iter().any(|row| row.id == candidate.epg_channel))
                {
                    return Err(InspectionError::StaleEvidence.into());
                }
                match (evidence.feed, addition) {
                    (Some(feed), None) => {
                        candidates.push(RepairCandidate {
                            feed,
                            incarnation: evidence.incarnation.clone(),
                            epg_channel: epg_channel.clone(),
                            staged: true,
                        });
                        RepairChoice::Existing {
                            feed,
                            epg_channel: epg_channel.clone(),
                        }
                    }
                    (None, Some(at))
                        if request
                            .additions
                            .get(*at)
                            .is_some_and(|a| a.evidence == *evidence) =>
                    {
                        RepairChoice::Addition {
                            index: *at,
                            epg_channel: epg_channel.clone(),
                        }
                    }
                    _ => return Err(Error::InvalidRequest),
                }
            }
        })
    }

    /// Validates explicit mappings and prepares the matching overrides.
    fn map_channels(
        &self,
        evidence: &Evidence,
        snapshot: &ChannelSnapshot,
        indexed: &IndexedFeeds,
        feeds: &FeedPlan,
    ) -> Result<MappingPlan> {
        let rows_by_id: HashMap<_, _> = snapshot
            .rows
            .iter()
            .map(|row| (row.key.as_str(), row))
            .collect();
        let mut after_overrides = snapshot.overrides.clone();
        let mut mappings = Vec::new();
        let mut previews = Vec::new();
        let mut candidates = Vec::new();
        let mut mapped = HashSet::new();
        for selected in &self.request.mappings {
            check(self.cancel, self.deadline)?;
            if !mapped.insert(selected.channel.content.clone()) {
                return Err(Error::InvalidRequest);
            }
            let row = rows_by_id
                .get(selected.channel.content.as_str())
                .ok_or(InspectionError::ChannelMissing)?;
            if row.blocked {
                return Err(InspectionError::PermissionDenied.into());
            }
            let before = row.epg_override.clone();
            let desired =
                self.desired_choice(&selected.target, evidence, indexed, &mut candidates)?;
            let after = match &desired {
                RepairChoice::Automatic => None,
                RepairChoice::Disabled => Some(iptv_epg::Override::NoEpg),
                RepairChoice::Existing { feed, epg_channel } => {
                    if !feeds
                        .after_configs
                        .iter()
                        .any(|f| f.feed == *feed && f.enabled)
                    {
                        return Err(Error::InvalidRequest);
                    }
                    Some(iptv_epg::Override::Epg {
                        source: Some(*feed),
                        epg_id: epg_channel.clone(),
                    })
                }
                RepairChoice::Addition { index, epg_channel } => Some(iptv_epg::Override::Epg {
                    source: Some(feeds.additions[*index].feed),
                    epg_id: epg_channel.clone(),
                }),
            };
            let key = ChannelKey::new(selected.channel.content.as_str());
            let protected =
                before.is_some() && before.as_ref().map(EpgOverride::to_override) != after;
            if protected && !selected.replace_explicit {
                return Err(Error::Conflict(vec![
                    selected.channel.content.as_str().into(),
                ]));
            }
            match after {
                Some(value) => {
                    after_overrides.insert(key, value);
                }
                None => {
                    after_overrides.remove(&key);
                }
            }
            previews.push(MappingPreview {
                channel: selected.channel.clone(),
                name: row.name.clone(),
                before: old_choice(before.as_ref()),
                after: selected.target.clone(),
                explicit_replacement: protected,
            });
            mappings.push(RepairMapping {
                channel: selected.channel.content.clone(),
                before,
                after: desired,
                replace_explicit: selected.replace_explicit,
            });
        }
        Ok(MappingPlan {
            changes: mappings,
            previews,
            candidates,
            explicit: mapped,
            overrides: after_overrides,
        })
    }

    /// Compares before and after matching evidence to preview indirect effects.
    fn assess_impact(
        &self,
        channels: &ChannelSnapshot,
        indexed: &IndexedFeeds,
        feeds: &FeedPlan,
        mappings: &MappingPlan,
    ) -> Result<Impact> {
        let store = self.store;
        let source = self.request.source;
        let before = matched(
            &index(&indexed.configs, &indexed.before),
            &channels.rows,
            &channels.overrides,
        );
        let after = matched(
            &index(&feeds.after_configs, &indexed.after),
            &channels.rows,
            &mappings.overrides,
        );
        let ambiguous = before.report.ambiguous_name > 0 || after.report.ambiguous_name > 0;
        let mut incarnations = HashMap::new();
        for feed in &indexed.configs {
            incarnations.insert(
                feed.feed,
                storage(store.feed_incarnation(source, feed.feed))?
                    .ok_or(InspectionError::StaleEvidence)?,
            );
        }
        let target = |reference: &iptv_epg::EpgRef| -> Target {
            if let Some((addition, evidence)) = feeds.staged_feeds.get(&reference.source) {
                Target::Staged {
                    evidence: evidence.clone(),
                    epg_channel: reference.epg_id.clone(),
                    addition: Some(*addition),
                }
            } else {
                Target::Configured(CandidateRef {
                    source,
                    feed: reference.source,
                    incarnation: incarnations
                        .get(&reference.source)
                        .cloned()
                        .unwrap_or_default(),
                    epg_channel: reference.epg_id.clone(),
                })
            }
        };
        let (rows, unknown) = self.indirect_effects(
            channels,
            &indexed.configs,
            &mappings.explicit,
            &before,
            &after,
            target,
        )?;
        let complete = indexed
            .configs
            .iter()
            .filter(|feed| feed.enabled)
            .all(|feed| indexed.fresh.contains(&feed.feed))
            && !ambiguous
            && !unknown;
        Ok(Impact { rows, complete })
    }

    fn indirect_effects(
        &self,
        channels: &ChannelSnapshot,
        configs: &[GuideSource],
        explicit: &HashSet<ContentId>,
        before: &iptv_epg::MatchResult,
        after: &iptv_epg::MatchResult,
        target: impl Fn(&iptv_epg::EpgRef) -> Target,
    ) -> Result<(Vec<IndirectEffect>, bool)> {
        let source = self.request.source;
        let mut indirect = Vec::new();
        let mut unknown = false;
        for row in &channels.rows {
            check(self.cancel, self.deadline)?;
            if explicit.contains(&row.key) || row.blocked {
                continue;
            }
            let key = ChannelKey::new(row.key.as_str());
            let old_chain = before.matches.get(&key).map(Vec::as_slice).unwrap_or(&[]);
            let new_chain = after.matches.get(&key).map(Vec::as_slice).unwrap_or(&[]);
            let chain_changed = old_chain
                .iter()
                .map(|r| (r.source, &r.epg_id))
                .collect::<Vec<_>>()
                != new_chain
                    .iter()
                    .map(|r| (r.source, &r.epg_id))
                    .collect::<Vec<_>>();
            let old = channels.bindings.get(&row.key);
            let touches = |feed| {
                self.request.feeds.iter().any(|edit| {
                    edit.feed == feed
                        && configs
                            .iter()
                            .find(|f| f.feed == feed)
                            .is_some_and(|config| {
                                edit.remove
                                    || edit.enabled.is_some_and(|v| v != config.enabled)
                                    || edit
                                        .shift_minutes
                                        .is_some_and(|v| v != config.shift_minutes)
                            })
                })
            };
            let affected = chain_changed
                || old.is_some_and(|binding| touches(binding.candidate.source))
                || old_chain
                    .iter()
                    .chain(new_chain)
                    .any(|candidate| touches(candidate.source));
            if !affected {
                continue;
            }
            let first = new_chain.first();
            let proven = first.is_some_and(|candidate| {
                channels
                    .available
                    .contains(&(candidate.source, candidate.epg_id.clone()))
                    && !touches(candidate.source)
                    && !self
                        .leases
                        .iter()
                        .any(|held| held.evidence().feed == Some(candidate.source))
            });
            let availability_unknown = first.is_some() && !proven;
            unknown |= availability_unknown;
            let before = old.map(|binding| {
                Target::Configured(CandidateRef {
                    source,
                    feed: binding.candidate.source,
                    incarnation: binding.incarnation.clone().unwrap_or_default(),
                    epg_channel: binding.candidate.epg_id.clone(),
                })
            });
            let timing_changed = old.is_some_and(|binding| {
                self.request.feeds.iter().any(|edit| {
                    edit.feed == binding.candidate.source
                        && edit.shift_minutes.is_some_and(|shift| {
                            configs
                                .iter()
                                .find(|f| f.feed == edit.feed)
                                .is_some_and(|f| f.shift_minutes != shift)
                        })
                })
            });
            indirect.push(IndirectEffect {
                channel: ChannelRef {
                    source,
                    content: row.key.clone(),
                },
                name: row.name.clone(),
                before,
                after: if proven { first.map(&target) } else { None },
                timing_changed,
                availability_unknown,
            });
        }
        Ok((indirect, unknown))
    }
}

pub(super) fn prepare(
    store: &Store,
    request: Request,
    leases: &[ProbeLease],
    context: Preparation<'_>,
) -> Result<StoredPlan> {
    if !request.valid() {
        return Err(Error::InvalidRequest);
    }
    let source = request.source;
    if storage(store.source(source))?.is_none() {
        return Err(InspectionError::SourceMissing(source).into());
    }
    let incarnation = storage(store.source_incarnation(source))?;
    if !storage(store.source_policy(source))?.enabled {
        return Err(InspectionError::SourceDisabled(source).into());
    }

    let session = PreparationSession {
        store,
        request: &request,
        leases,
        cancel: context.cancel,
        deadline: context.deadline,
    };
    let evidence = session.evidence(&incarnation, context.created_at)?;
    let mut indexed = session.index_feeds(&evidence)?;
    let channels = session.snapshot_channels()?;
    let mut feeds = prepare_feed_edits(&request, &indexed.configs)?;
    session.add_feeds(&evidence, &mut indexed, &mut feeds)?;
    let mappings = session.map_channels(&evidence, &channels, &indexed, &feeds)?;
    let impact = session.assess_impact(&channels, &indexed, &feeds, &mappings)?;
    Ok(stored_plan(
        context,
        request,
        incarnation,
        evidence.expires_at,
        feeds,
        mappings,
        impact,
    ))
}

fn stored_plan(
    context: Preparation<'_>,
    request: Request,
    incarnation: String,
    expires_at: i64,
    feeds: FeedPlan,
    mappings: MappingPlan,
    impact: Impact,
) -> StoredPlan {
    let source = request.source;
    let Preparation {
        id,
        epoch,
        created_at,
        ..
    } = context;
    let FeedPlan {
        changes: feed_changes,
        previews: feed_previews,
        additions,
        order,
        ..
    } = feeds;
    let MappingPlan {
        changes: mappings,
        previews,
        candidates,
        ..
    } = mappings;
    let Impact {
        rows: indirect,
        complete: indirect_complete,
    } = impact;
    let mut hash = Sha256::new();
    hash.update(id.as_bytes());
    hash.update(epoch.to_le_bytes());
    hash.update(incarnation.as_bytes());
    hash.update(format!("{request:?}").as_bytes());
    hash.update(format!("{previews:?}{feed_previews:?}{indirect:?}").as_bytes());
    let digest: [u8; 32] = hash.finalize().into();
    let preview = Preview {
        id,
        digest,
        source,
        created_at,
        expires_at,
        mapping_count: previews.len(),
        indirect_count: indirect.len(),
        indirect_complete,
        mappings: previews.iter().take(MAX_PREVIEW_PAGE).cloned().collect(),
        feeds: feed_previews,
        indirect: indirect.iter().take(MAX_PREVIEW_PAGE).cloned().collect(),
        next_mapping: (previews.len() > MAX_PREVIEW_PAGE).then_some(MAX_PREVIEW_PAGE),
        next_indirect: (indirect.len() > MAX_PREVIEW_PAGE).then_some(MAX_PREVIEW_PAGE),
        expires_on_owner_close: true,
    };
    let batch = RepairBatch {
        source,
        source_incarnation: incarnation,
        mappings,
        feeds: feed_changes,
        additions,
        candidates,
        preview_expires_at: expires_at,
        order,
    };
    StoredPlan {
        admitted: Instant::now(),
        request,
        preview,
        rows: previews,
        indirect,
        batch,
        epoch,
    }
}
