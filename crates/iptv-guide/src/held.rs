//! A cold start's first schedule holds only its first screen: some hours
//! ([`EpgSchedule::held_only`]) of some channels
//! ([`EpgSchedule::held_channels`]). Everything else shows as
//! [`crate::LOADING`] and asks for nothing; the schedule with the whole
//! window replaces it (`iptv-app` `data/opening.rs`).

use iptv_epg::{ChannelKey, GuideStore, UnixTime};

use crate::{EpgSchedule, SLOT};

impl EpgSchedule {
    /// `store` holds only the programmes overlapping `[from, to)` (a cold
    /// start's first screen, read before the rest of the window): other
    /// times show as [`crate::LOADING`], and ask for nothing; the schedule with the
    /// whole window replaces this one.
    pub fn held_only(mut self, from: UnixTime, to: UnixTime) -> Self {
        self.held = Some((from, to));
        self
    }

    /// With [`Self::held_only`]: `store` holds only the channels `keys`
    /// (the first screen's rows); the others show as loading.
    pub fn held_channels(mut self, keys: impl IntoIterator<Item = ChannelKey>) -> Self {
        self.held_keys = Some(keys.into_iter().collect());
        self
    }

    /// The times `store` holds when it holds only part of its window.
    pub fn held(&self) -> Option<(UnixTime, UnixTime)> {
        self.held
    }

    /// [`Self::with_store`] for channel `key`: `None` also while the
    /// channel is not held ([`Self::held_channels`]).
    pub(crate) fn channel_store<R>(
        &self,
        key: Option<&ChannelKey>,
        t: UnixTime,
        f: impl FnOnce(&GuideStore) -> R,
    ) -> Option<R> {
        if let (Some(keys), Some(key)) = (&self.held_keys, key)
            && !keys.contains(key)
        {
            return None;
        }
        self.with_store(t, f)
    }

    /// A programme held whole that reaches past the held part's end to `t`
    /// ([`Self::held_only`]: the read takes every programme overlapping it).
    pub(crate) fn held_across(
        &self,
        key: Option<&ChannelKey>,
        t: UnixTime,
    ) -> Option<(UnixTime, UnixTime)> {
        let (_, to) = self.held?;
        let p = self.store.programme_at(key?, t).filter(|_| t >= to)?;
        Some((p.start(), p.stop()))
    }

    /// The hour slot around `t` shown while its programmes load; past a
    /// held part's end it starts no earlier than the programme held there
    /// ends, so cells do not overlap.
    pub(crate) fn loading_slot(
        &self,
        key: Option<&ChannelKey>,
        t: UnixTime,
    ) -> (UnixTime, UnixTime) {
        let slot = t - t.rem_euclid(SLOT);
        let mut start = slot;
        if let (Some((_, to)), Some(key)) = (self.held, key)
            && t >= to
            && let Some(p) = self.store.programme_at(key, to - 1)
            && p.stop() <= t
        {
            start = start.max(p.stop());
        }
        (start, (slot + SLOT).max(start + 60))
    }
}
