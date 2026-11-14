//! A source has one cancellable guide timer or in-flight guide read.
use iptv_import::CancelToken;
use iptv_library::SourceId;
use std::collections::BTreeMap;

#[derive(Default)]
pub(crate) struct GuideChecks {
    next: u64,
    entries: BTreeMap<SourceId, (u64, CancelToken)>,
}
impl GuideChecks {
    pub fn replace(&mut self, source: SourceId) -> (u64, CancelToken) {
        self.cancel(source);
        self.next = self.next.wrapping_add(1);
        let token = CancelToken::new();
        self.entries.insert(source, (self.next, token.clone()));
        (self.next, token)
    }
    pub fn current(&self, source: SourceId, ticket: u64) -> bool {
        self.entries
            .get(&source)
            .is_some_and(|(at, token)| *at == ticket && !token.is_cancelled())
    }
    pub fn cancel(&mut self, source: SourceId) {
        if let Some((_, token)) = self.entries.remove(&source) {
            token.cancel();
        }
    }
    pub fn clear(&mut self) {
        for (_, (_, token)) in std::mem::take(&mut self.entries) {
            token.cancel();
        }
    }
}
impl Drop for GuideChecks {
    fn drop(&mut self) {
        self.clear();
    }
}
impl crate::Runtime {
    /// Register only while this browser generation still owns the guide.
    pub fn schedule_guide_check(
        &self,
        source: SourceId,
        generation: u64,
    ) -> Option<(u64, CancelToken)> {
        (self.generation.get() == generation).then(|| self.replace_guide_check(source))
    }
    pub fn replace_guide_check(&self, source: SourceId) -> (u64, CancelToken) {
        self.guide_checks.borrow_mut().replace(source)
    }
    pub fn guide_check_current(&self, source: SourceId, ticket: u64) -> bool {
        self.guide_checks.borrow().current(source, ticket)
    }
    pub fn has_guide_check(&self, source: SourceId) -> bool {
        self.guide_checks.borrow().entries.contains_key(&source)
    }
    pub fn cancel_guide_check(&self, source: SourceId) {
        self.guide_checks.borrow_mut().cancel(source);
    }
    pub fn cancel_guide_checks(&self) {
        self.guide_checks.borrow_mut().clear();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn two_combined_refreshes_leave_one_check_for_each_source() {
        let mut checks = GuideChecks::default();
        let source = SourceId(1);
        let (first, first_token) = checks.replace(source);
        let (_, other) = checks.replace(SourceId(2));
        let (second, second_token) = checks.replace(source);
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
}
