//! Named shell operations; mutable cells never cross the runtime boundary.
use crate::Runtime;
use iptv_core::wizard::ImportId;
use iptv_import::CancelToken;

impl Runtime {
    pub fn browser_generation(&self) -> u64 {
        self.generation.get()
    }
    pub fn account_limit(&self) -> Option<u32> {
        self.account_streams.get()
    }
    pub fn update_account_limit(&self, limit: Option<u32>) {
        self.account_streams.set(limit);
    }
    pub fn set_foreground(&self, foreground: bool) {
        self.foreground.set(foreground);
    }
    pub fn cancel_link_resolution(&self, surface: usize) {
        let running = self
            .resolving
            .borrow_mut()
            .get_mut(surface)
            .and_then(Option::take);
        if let Some((_, cancel)) = running {
            cancel.cancel();
        }
    }
    pub fn begin_link_resolution(&self, surface: usize, session: u64) -> CancelToken {
        self.cancel_link_resolution(surface);
        let cancel = CancelToken::new();
        if let Some(slot) = self.resolving.borrow_mut().get_mut(surface) {
            *slot = Some((session, cancel.clone()));
        }
        cancel
    }
    pub fn finish_link_resolution(&self, surface: usize, session: u64) {
        if let Some(slot) = self.resolving.borrow_mut().get_mut(surface)
            && slot.as_ref().is_some_and(|(at, _)| *at == session)
        {
            *slot = None;
        }
    }
    pub fn remember_wizard_import(&self, id: ImportId, cancel: CancelToken) {
        *self.wizard_import.borrow_mut() = Some((id, cancel));
    }
    pub fn cancel_wizard_import(&self, id: ImportId) {
        let mut running = self.wizard_import.borrow_mut();
        if running.as_ref().is_some_and(|(at, _)| *at == id)
            && let Some((_, token)) = running.take()
        {
            log::info!("iptv wizard: import {id:?} cancelled");
            token.cancel();
        }
    }
    pub fn finish_wizard_import(&self) {
        self.wizard_import.borrow_mut().take();
    }
    pub fn search_history(&self) -> Vec<String> {
        self.saved_search.borrow().clone().unwrap_or_default()
    }
    pub fn clear_loaded_search_history(&self) {
        *self.saved_search.borrow_mut() = None;
    }
    pub fn install_search_history(&self, history: Vec<String>) {
        *self.saved_search.borrow_mut() = Some(history);
    }
    pub fn search_history_matches(&self, history: &[String]) -> bool {
        self.saved_search.borrow().as_deref() == Some(history)
    }
    pub fn begin_search(&self) -> u64 {
        let next = self.pending_search.get() + 1;
        self.pending_search.set(next);
        next
    }
    pub fn search_is_current(&self, ticket: u64) -> bool {
        self.pending_search.get() == ticket
    }
}
