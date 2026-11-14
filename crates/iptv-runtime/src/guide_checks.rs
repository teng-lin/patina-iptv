//! A source has one cancellable guide timer or in-flight guide read.
//!
//! Work that takes a source's guide over (an import, an update asked for)
//! cancels its entry. A timer or a check cancelled that way stays *owed*
//! until that work answers with the next check or with none: work that
//! ends without an answer (it failed, was refused, or could not read the
//! store) then leaves the check the refresh policy had asked for, instead
//! of dropping it or making one up.
use iptv_import::CancelToken;
use iptv_library::SourceId;
use std::collections::BTreeMap;

/// The sources with a registered check, for a headless test: it runs on
/// its own thread and cannot reach the app thread's runtime. Each owner
/// has its own (`store_service::Service`), so two runtimes in one process
/// do not see each other's checks.
#[cfg(feature = "test-support")]
pub(crate) type Mirror = std::sync::Arc<std::sync::Mutex<std::collections::BTreeSet<SourceId>>>;

/// Whether `source` has a guide timer or an in-flight guide read in the
/// runtime of this context (off the app's thread: the active one).
#[cfg(feature = "test-support")]
pub fn guide_check_registered(source: SourceId) -> bool {
    crate::store_service::try_service().is_some_and(|owner| {
        owner
            .guide_checks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&source)
    })
}

/// What a source's registered entry is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuideWork {
    /// A timer waiting until this time (unix) to check the guide.
    Timer(i64),
    /// A read under way that checks the guide: it may download it and
    /// says when to look again.
    Check,
    /// A read under way of what is stored only.
    Cache,
}

struct Entry {
    ticket: u64,
    token: CancelToken,
    work: GuideWork,
}

#[derive(Default)]
pub(crate) struct GuideChecks {
    next: u64,
    entries: BTreeMap<SourceId, Entry>,
    /// When the check that other work cancelled was due (0: it was
    /// running), until that work answers.
    owed: BTreeMap<SourceId, i64>,
    #[cfg(feature = "test-support")]
    mirror: Mirror,
}
impl GuideChecks {
    #[cfg(feature = "test-support")]
    pub fn mirrored(mirror: Mirror) -> Self {
        let mut checks = Self::default();
        checks.mirror = mirror;
        checks
    }
    #[cfg(feature = "test-support")]
    fn note(&self, source: SourceId, registered: bool) {
        let mut sources = self
            .mirror
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if registered {
            sources.insert(source);
        } else {
            sources.remove(&source);
        }
    }
    fn remove(&mut self, source: SourceId) -> Option<GuideWork> {
        let entry = self.entries.remove(&source)?;
        entry.token.cancel();
        #[cfg(feature = "test-support")]
        self.note(source, false);
        Some(entry.work)
    }
    /// Registers `work` in place of `source`'s entry. A timer is the
    /// policy's new answer and a check is the owed one running: nothing
    /// stays owed. A cache read answers for nothing: the timer or check
    /// it displaces stays owed.
    pub fn replace(&mut self, source: SourceId, work: GuideWork) -> (u64, CancelToken) {
        let displaced = self.remove(source);
        match (work, displaced) {
            (GuideWork::Cache, Some(GuideWork::Timer(at))) => {
                self.owed.insert(source, at);
            }
            (GuideWork::Cache, Some(GuideWork::Check)) => {
                self.owed.insert(source, 0);
            }
            (GuideWork::Cache, _) => {}
            _ => {
                self.owed.remove(&source);
            }
        }
        self.next = self.next.wrapping_add(1);
        let token = CancelToken::new();
        self.entries.insert(
            source,
            Entry {
                ticket: self.next,
                token: token.clone(),
                work,
            },
        );
        #[cfg(feature = "test-support")]
        self.note(source, true);
        (self.next, token)
    }
    pub fn current(&self, source: SourceId, ticket: u64) -> bool {
        self.entries
            .get(&source)
            .is_some_and(|entry| entry.ticket == ticket && !entry.token.is_cancelled())
    }
    /// Other work takes `source`'s guide over: its timer or check stops
    /// and stays owed until that work answers.
    pub fn cancel(&mut self, source: SourceId) {
        match self.remove(source) {
            Some(GuideWork::Timer(at)) => {
                self.owed.insert(source, at);
            }
            Some(GuideWork::Check) => {
                self.owed.insert(source, 0);
            }
            Some(GuideWork::Cache) | None => {}
        }
    }
    /// `source`'s entry ended by itself (its read finished or could not
    /// start): nothing becomes owed by that.
    pub fn end(&mut self, source: SourceId) {
        self.remove(source);
    }
    /// Stops `source`'s in-flight read, if one runs, and says what it
    /// was; a waiting timer stays. A check stopped that way is owed.
    pub fn interrupt_read(&mut self, source: SourceId) -> Option<GuideWork> {
        let work = self.entries.get(&source)?.work;
        if matches!(work, GuideWork::Timer(_)) {
            return None;
        }
        self.cancel(source);
        Some(work)
    }
    /// The check `source` is owed, which the caller now takes care of.
    pub fn take_owed(&mut self, source: SourceId) -> Option<i64> {
        self.owed.remove(&source)
    }
    /// `source`'s work asked for a check at `at` that cannot be set now
    /// (a newer generation began): it is owed if the source stays shown.
    pub fn owe(&mut self, source: SourceId, at: i64) {
        self.owed.insert(source, at);
    }
    /// A new generation: every entry ends. Nothing stays owed, except to
    /// `shown`, the playlist on screen: the import that begins the
    /// generation may fail and leave it there, and its own work under way
    /// may still answer for it (a later answer replaces what is kept).
    pub fn clear(&mut self, shown: Option<SourceId>) {
        let kept = shown.and_then(|source| {
            self.cancel(source);
            Some((source, self.owed.remove(&source)?))
        });
        self.owed.clear();
        self.owed.extend(kept);
        for (_source, entry) in std::mem::take(&mut self.entries) {
            entry.token.cancel();
            #[cfg(feature = "test-support")]
            self.note(_source, false);
        }
    }
}
impl Drop for GuideChecks {
    fn drop(&mut self) {
        self.clear(None);
    }
}
impl crate::Runtime {
    /// Registers a guide check for `source`, only while this browser
    /// generation still owns the guide.
    pub fn schedule_guide_check(
        &self,
        source: SourceId,
        generation: u64,
    ) -> Option<(u64, CancelToken)> {
        self.schedule_guide_work(source, generation, GuideWork::Check)
    }
    /// [`Self::schedule_guide_check`] for a timer or a read of that kind.
    pub fn schedule_guide_work(
        &self,
        source: SourceId,
        generation: u64,
        work: GuideWork,
    ) -> Option<(u64, CancelToken)> {
        (self.generation().with(std::cell::Cell::get) == generation)
            .then(|| self.guide_checks.borrow_mut().replace(source, work))
    }
    pub fn replace_guide_check(&self, source: SourceId) -> (u64, CancelToken) {
        self.guide_checks
            .borrow_mut()
            .replace(source, GuideWork::Check)
    }
    pub fn guide_check_current(&self, source: SourceId, ticket: u64) -> bool {
        self.guide_checks.borrow().current(source, ticket)
    }
    pub fn has_guide_check(&self, source: SourceId) -> bool {
        self.guide_checks.borrow().entries.contains_key(&source)
    }
    /// Other work takes `source`'s guide over (see [`GuideChecks::cancel`]).
    pub fn cancel_guide_check(&self, source: SourceId) {
        self.guide_checks.borrow_mut().cancel(source);
    }
    /// `source`'s own read ended (see [`GuideChecks::end`]).
    pub fn end_guide_check(&self, source: SourceId) {
        self.guide_checks.borrow_mut().end(source);
    }
    /// A guide action replaces what `source`'s in-flight read would show
    /// or store (see [`GuideChecks::interrupt_read`]).
    pub fn interrupt_guide_read(&self, source: SourceId) -> Option<GuideWork> {
        self.guide_checks.borrow_mut().interrupt_read(source)
    }
    /// The work that owned `source`'s guide answered with no next check,
    /// or the caller keeps the owed one itself.
    pub fn take_owed_guide_check(&self, source: SourceId) -> Option<i64> {
        self.guide_checks.borrow_mut().take_owed(source)
    }
    /// See [`GuideChecks::owe`].
    pub fn owe_guide_check(&self, source: SourceId, at: i64) {
        self.guide_checks.borrow_mut().owe(source, at);
    }
    pub fn cancel_guide_checks(&self) {
        self.guide_checks.borrow_mut().clear(None);
    }
    /// A new generation begins while `shown` is on screen (see
    /// [`GuideChecks::clear`]).
    pub fn cancel_guide_checks_keeping(&self, shown: Option<SourceId>) {
        self.guide_checks.borrow_mut().clear(shown);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn two_combined_refreshes_leave_one_check_for_each_source() {
        let mut checks = GuideChecks::default();
        let source = SourceId(1);
        let (first, first_token) = checks.replace(source, GuideWork::Check);
        let (_, other) = checks.replace(SourceId(2), GuideWork::Check);
        let (second, second_token) = checks.replace(source, GuideWork::Check);
        assert!(first_token.is_cancelled());
        assert!(!checks.current(source, first));
        assert!(checks.current(source, second));
        assert_eq!(checks.entries.len(), 2);
        checks.cancel(source);
        assert!(second_token.is_cancelled());
        assert!(!other.is_cancelled());
        drop(checks);
        assert!(other.is_cancelled());
    }

    #[test]
    fn a_timer_cancelled_by_other_work_stays_owed_until_that_work_answers() {
        let mut checks = GuideChecks::default();
        let source = SourceId(1);
        // Nothing registered: work that takes over owes nothing.
        checks.cancel(source);
        assert_eq!(checks.take_owed(source), None);
        // A timer: owed at its own time, once.
        checks.replace(source, GuideWork::Timer(500));
        checks.cancel(source);
        assert_eq!(checks.take_owed(source), Some(500));
        assert_eq!(checks.take_owed(source), None);
        // The work's answer (a new timer) replaces what was owed.
        checks.replace(source, GuideWork::Timer(500));
        checks.cancel(source);
        checks.replace(source, GuideWork::Timer(900));
        assert_eq!(checks.take_owed(source), None);
        // So does the owed check running; cancelled again, it is owed now.
        checks.cancel(source);
        checks.replace(source, GuideWork::Check);
        assert_eq!(checks.take_owed(source), None);
        checks.cancel(source);
        assert_eq!(checks.take_owed(source), Some(0));
        // A read that ends by itself owes nothing.
        checks.replace(source, GuideWork::Check);
        checks.end(source);
        assert_eq!(checks.take_owed(source), None);
        // A cache read displaces a timer without answering for it.
        checks.replace(source, GuideWork::Timer(700));
        checks.replace(source, GuideWork::Cache);
        checks.end(source);
        assert_eq!(checks.take_owed(source), Some(700));
        // A new generation owes nothing...
        checks.replace(source, GuideWork::Timer(700));
        checks.cancel(source);
        checks.clear(None);
        assert_eq!(checks.take_owed(source), None);
    }

    #[test]
    fn a_new_generation_keeps_what_the_playlist_on_screen_is_owed() {
        let mut checks = GuideChecks::default();
        let (shown, other) = (SourceId(1), SourceId(2));
        // Its timer, and nobody else's.
        let (ticket, timer) = checks.replace(shown, GuideWork::Timer(500));
        checks.replace(other, GuideWork::Timer(600));
        checks.clear(Some(shown));
        assert!(timer.is_cancelled() && !checks.current(shown, ticket));
        assert!(checks.entries.is_empty());
        assert_eq!(checks.take_owed(other), None);
        assert_eq!(checks.take_owed(shown), Some(500));
        // What work under way had cancelled before, and a running check.
        checks.replace(shown, GuideWork::Timer(500));
        checks.cancel(shown);
        checks.clear(Some(shown));
        assert_eq!(checks.take_owed(shown), Some(500));
        checks.replace(shown, GuideWork::Check);
        checks.clear(Some(shown));
        assert_eq!(checks.take_owed(shown), Some(0));
        // Nothing registered or owed: nothing made up.
        checks.clear(Some(shown));
        assert_eq!(checks.take_owed(shown), None);
        // Its work's later answers replace what was kept: a check it asks
        // for under the old generation, or none at all.
        checks.replace(shown, GuideWork::Timer(500));
        checks.clear(Some(shown));
        checks.owe(shown, 900);
        assert_eq!(checks.take_owed(shown), Some(900));
        checks.replace(shown, GuideWork::Timer(500));
        checks.clear(Some(shown));
        assert_eq!(checks.take_owed(shown), Some(500));
        assert_eq!(checks.take_owed(shown), None);
    }

    #[test]
    fn a_guide_action_stops_a_read_and_leaves_a_timer() {
        let mut checks = GuideChecks::default();
        let source = SourceId(1);
        let (ticket, timer) = checks.replace(source, GuideWork::Timer(500));
        assert_eq!(checks.interrupt_read(source), None);
        assert!(checks.current(source, ticket) && !timer.is_cancelled());
        let (_, read) = checks.replace(source, GuideWork::Check);
        assert_eq!(checks.interrupt_read(source), Some(GuideWork::Check));
        assert!(read.is_cancelled() && checks.entries.is_empty());
        // The stopped check is owed until other work answers for it.
        assert_eq!(checks.take_owed(source), Some(0));
        let (_, cache) = checks.replace(source, GuideWork::Cache);
        assert_eq!(checks.interrupt_read(source), Some(GuideWork::Cache));
        assert!(cache.is_cancelled());
        assert_eq!(checks.take_owed(source), None);
        checks.owe(source, 300);
        assert_eq!(checks.take_owed(source), Some(300));
    }
}
