//! State and projections for the combined browser.

use crate::Runtime;
use crate::model::SourceId;
use crate::state::{ImportWork, SingleSchedule};
use iptv_guide::{
    EpgSchedule,
    combined::{CombinedSchedule, SourceSchedule, split_key},
};
use std::rc::{Rc, Weak};

pub(super) fn reset_single(owner: &Rc<crate::Runtime>) {
    owner.single.with(|s| s.borrow_mut().take());
}
pub(super) fn single(owner: &Rc<crate::Runtime>) -> Option<SingleSchedule> {
    owner.single.with(|s| s.borrow().clone())
}
pub(super) fn remember_single(
    owner: &Rc<crate::Runtime>,
    source: SourceId,
    schedule: Rc<EpgSchedule>,
    revision: Option<Weak<ImportWork>>,
) {
    owner.single.with(|s| {
        *s.borrow_mut() = Some(SingleSchedule {
            source,
            schedule,
            limit: owner.account_streams.get(),
            pending: None,
            revision,
        })
    });
}

pub fn active(owner: &Rc<crate::Runtime>) -> bool {
    owner.all.get()
}
pub fn owner(owner: &Rc<crate::Runtime>, key: &str) -> Option<(SourceId, String)> {
    if active(owner) {
        let (s, k) = split_key(key)?;
        Some((SourceId(s), k.to_owned()))
    } else {
        Some((owner.shown.get()?, key.to_owned()))
    }
}
pub fn connection_limit(owner: &Rc<crate::Runtime>, source: SourceId) -> Option<u32> {
    owner
        .limits
        .with(|l| l.borrow().get(&source).copied().flatten())
}

pub(super) fn clear_parts(owner: &Runtime) {
    owner.parts.borrow_mut().clear();
    owner.limits.borrow_mut().clear();
}

pub(super) fn retain_single(owner: &Runtime, kept: SingleSchedule) {
    store_part(owner, kept.source, kept.schedule);
    owner.limits.borrow_mut().insert(kept.source, kept.limit);
}

pub(super) fn part(owner: &Runtime, source: SourceId) -> Option<Rc<EpgSchedule>> {
    owner.parts.borrow().get(&source).cloned()
}

pub(super) fn store_part(owner: &Runtime, source: SourceId, schedule: Rc<EpgSchedule>) {
    owner.parts.borrow_mut().insert(source, schedule);
}

pub(super) fn note_account_limit(owner: &Runtime, source: SourceId, allowed: Option<u32>) {
    let mut limits = owner.limits.borrow_mut();
    if allowed.is_some() || !limits.contains_key(&source) {
        limits.insert(source, allowed);
    }
}

pub(super) fn source_schedules(
    owner: &Runtime,
    enabled: Vec<(SourceId, String)>,
) -> Vec<SourceSchedule> {
    let mut parts = owner.parts.borrow_mut();
    parts.retain(|id, _| enabled.iter().any(|(enabled, _)| enabled == id));
    enabled
        .into_iter()
        .filter_map(|(id, name)| {
            Some(SourceSchedule {
                id: id.0,
                name,
                schedule: parts.get(&id)?.clone(),
            })
        })
        .collect()
}

pub(super) fn reset_epoch(owner: &Runtime, now: i64) {
    owner.imports.epoch.set(now - now.rem_euclid(3600));
}

pub(super) fn invalidate_rebuild(owner: &Runtime) -> u64 {
    let next = owner.imports.rebuild.get().wrapping_add(1);
    owner.imports.rebuild.set(next);
    next
}

pub(super) fn toggled_playlist(
    owner: &Runtime,
    group: usize,
) -> Option<(Rc<CombinedSchedule>, Vec<i64>)> {
    let current = owner.schedule();
    let combined = current.as_any()?.downcast_ref::<CombinedSchedule>()?;
    let collapsed = combined.toggle_playlist(group)?;
    let schedule = Rc::new(combined.with_browser_expansion(collapsed.clone()));
    Some((schedule, collapsed))
}
