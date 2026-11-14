//! A bounded analysis snapshot produces an immutable, owner-bound repair plan.
use super::*;
use iptv_epg::{ChannelKey, FeedId, MatchIndex, Overrides, PlaylistChannel, XmltvChannel};
use iptv_store_sqlite::{
    ChannelQuery, EpgOverride, GuideSource, RepairBatch, RepairCandidate, RepairChoice, RepairFeed,
    RepairMapping, Store,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};

fn storage<T>(result: iptv_store_sqlite::Result<T>) -> Result<T> {
    result.map_err(|_| Error::Storage)
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
    rows: &[iptv_store_sqlite::ChannelRow],
    overrides: &Overrides,
) -> HashMap<ChannelKey, Vec<iptv_epg::EpgRef>> {
    let keys: Vec<_> = rows
        .iter()
        .map(|r| ChannelKey::new(r.channel.id.as_str()))
        .collect();
    index
        .match_channels(
            rows.iter()
                .zip(keys.iter())
                .map(|(r, key)| PlaylistChannel {
                    key,
                    tvg_id: r.channel.epg_id.as_deref(),
                    name: &r.channel.name,
                    tvg_name: r.channel.epg_name.as_deref(),
                }),
            overrides,
        )
        .matches
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

pub(super) fn prepare(
    store: &Store,
    request: Request,
    leases: &[ProbeLease],
    epoch: u64,
    id: String,
    created_at: i64,
) -> Result<StoredPlan> {
    if !request.valid() {
        return Err(Error::InvalidRequest);
    }
    let source = request.source;
    let incarnation = storage(store.source_incarnation(source))?;
    if !storage(store.source_policy(source))?.enabled {
        return Err(InspectionError::SourceDisabled(source).into());
    }
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
    let configs = storage(store.guide_sources(source))?;
    let listed = storage(store.guide_channels(source))?;
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
    // Staging supplies all display names and candidates, including entries the
    // normal filtered import did not retain programmes for.
    for held in leases {
        if let Some(feed) = held.evidence().feed {
            channels.insert(feed, held.listed_channels()?);
        }
    }
    let rows = storage(store.load_channels(
        source,
        &ChannelQuery {
            include_hidden: true,
            limit: Some(100_001),
            ..Default::default()
        },
    ))?;
    if rows.len() > 100_000 {
        return Err(InspectionError::LimitExceeded.into());
    }
    let rows_by_id: HashMap<_, _> = rows
        .iter()
        .map(|row| (row.channel.id.as_str(), row))
        .collect();
    let before_overrides: Overrides = storage(store.epg_overrides(source))?
        .into_iter()
        .map(|(key, value)| (ChannelKey::new(key.as_str()), value.to_override()))
        .collect();
    let mut after_overrides = before_overrides.clone();
    let mut after_configs = configs.clone();
    let mut feed_changes = Vec::new();
    let mut feed_previews = Vec::new();
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
            .find(|f| f.feed == edit.feed)
            .ok_or(Error::InvalidRequest)?
            .clone();
        let at = after_configs
            .iter()
            .position(|f| f.feed == edit.feed)
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
        feed_previews.push(FeedPreview {
            feed: Some(edit.feed),
            name: before.name.clone().unwrap_or_default(),
            before_enabled: Some(before.enabled),
            after_enabled: after.as_ref().map(|f| f.enabled),
            before_shift_minutes: Some(before.shift_minutes),
            after_shift_minutes: after.as_ref().map(|f| f.shift_minutes),
            added: false,
            removed: after.is_none(),
        });
        feed_changes.push(RepairFeed {
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
    let mut additions = Vec::new();
    let mut staged_feeds = BTreeMap::new();
    for (position, addition) in request.additions.iter().enumerate() {
        if addition.evidence.feed.is_some() || addition.name.as_ref().is_some_and(|n| n.len() > 256)
        {
            return Err(Error::InvalidRequest);
        }
        let held = lease(leases, &addition.evidence)?;
        // A private matching-only ID is translated back to a staged Target in
        // the preview. It is never persisted or exposed as a configured feed.
        let temporary = FeedId(u32::MAX - position as u32);
        if configs.iter().any(|c| c.feed == temporary) {
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
        channels.insert(temporary, held.listed_channels()?);
        staged_feeds.insert(temporary, (position, addition.evidence.clone()));
        after_configs.push(config.clone());
        additions.push(config);
        feed_previews.push(FeedPreview {
            feed: None,
            name: addition.name.clone().unwrap_or_default(),
            before_enabled: None,
            after_enabled: Some(true),
            before_shift_minutes: None,
            after_shift_minutes: Some(0),
            added: true,
            removed: false,
        });
    }
    let mut mappings = Vec::new();
    let mut previews = Vec::new();
    let mut candidates = Vec::new();
    let mut mapped = HashSet::new();
    for selected in &request.mappings {
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
        let desired = match &selected.target {
            Target::Automatic => RepairChoice::Automatic,
            Target::NoGuide => RepairChoice::Disabled,
            Target::Configured(key) => {
                if key.source != source {
                    return Err(Error::InvalidRequest);
                }
                valid_candidate(store, key)?;
                if !channels
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
                let held = lease(leases, evidence)?;
                let candidate = CandidateRef {
                    source,
                    feed: evidence.feed.unwrap_or(FeedId(0)),
                    incarnation: evidence.incarnation.clone(),
                    epg_channel: epg_channel.clone(),
                };
                held.validate_candidate(&candidate)?;
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
        };
        let after = match &desired {
            RepairChoice::Automatic => None,
            RepairChoice::Disabled => Some(iptv_epg::Override::NoEpg),
            RepairChoice::Existing { feed, epg_channel } => {
                if !after_configs.iter().any(|f| f.feed == *feed && f.enabled) {
                    return Err(Error::InvalidRequest);
                }
                Some(iptv_epg::Override::Epg {
                    source: Some(*feed),
                    epg_id: epg_channel.clone(),
                })
            }
            RepairChoice::Addition { index, epg_channel } => Some(iptv_epg::Override::Epg {
                source: Some(additions[*index].feed),
                epg_id: epg_channel.clone(),
            }),
        };
        let key = ChannelKey::new(selected.channel.content.as_str());
        let protected = before.is_some() && before.as_ref().map(EpgOverride::to_override) != after;
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
    let before = matched(&index(&configs, &channels), &rows, &before_overrides);
    let after = matched(&index(&after_configs, &channels), &rows, &after_overrides);
    let mut incarnations = HashMap::new();
    for feed in &configs {
        incarnations.insert(
            feed.feed,
            storage(store.feed_incarnation(source, feed.feed))?
                .ok_or(InspectionError::StaleEvidence)?,
        );
    }
    let target = |reference: &iptv_epg::EpgRef| -> Target {
        if let Some((addition, evidence)) = staged_feeds.get(&reference.source) {
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
    let mut indirect = Vec::new();
    for row in &rows {
        if mapped.contains(&row.channel.id) || row.blocked {
            continue;
        }
        let key = ChannelKey::new(row.channel.id.as_str());
        let old = before.get(&key).and_then(|c| c.first());
        let new = after.get(&key).and_then(|c| c.first());
        let timing_changed = old.is_some_and(|old| {
            request.feeds.iter().any(|edit| {
                edit.feed == old.source
                    && edit.shift_minutes.is_some_and(|shift| {
                        configs
                            .iter()
                            .find(|f| f.feed == edit.feed)
                            .is_some_and(|f| f.shift_minutes != shift)
                    })
            })
        });
        if old.map(|r| (r.source, &r.epg_id)) != new.map(|r| (r.source, &r.epg_id))
            || timing_changed
        {
            indirect.push(IndirectEffect {
                channel: ChannelRef {
                    source,
                    content: row.channel.id.clone(),
                },
                name: row.name.clone(),
                before: old.map(target),
                after: new.map(target),
                timing_changed,
            });
        }
    }
    let indirect_complete = configs
        .iter()
        .filter(|f| f.enabled)
        .all(|f| channels.get(&f.feed).is_some_and(|rows| !rows.is_empty()));
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
    Ok(StoredPlan {
        request,
        preview,
        rows: previews,
        indirect,
        batch,
        epoch,
    })
}
