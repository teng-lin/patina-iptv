//! Per-owner catalog, schedule, and playback view state.

use crate::{
    EpgSchedule, LibrarySnapshot, PageWaiters, Resolves, Schedule, SingleSchedule, Slot, SourceId,
};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, HashMap},
    rc::Rc,
    sync::{Arc, Mutex},
};

type LinkClientCache = Arc<Mutex<Option<(crate::network::NetworkConfig, crate::network::Client)>>>;
type ScheduleParts = BTreeMap<SourceId, Rc<EpgSchedule>>;
type ProviderLimits = BTreeMap<SourceId, Option<u32>>;
type ShowWaiter = futures_channel::oneshot::Receiver<()>;

pub(super) struct State {
    account_streams: Cell<Option<u32>>,
    resolving: Resolves,
    resolve_ticket: Cell<u64>,
    link_client: LinkClientCache,
    library: Slot<RefCell<Option<LibrarySnapshot>>>,
    parts: RefCell<ScheduleParts>,
    single: Slot<RefCell<Option<SingleSchedule>>>,
    schedule: RefCell<Rc<dyn Schedule>>,
    schedule_revision: Cell<u64>,
    shown: Cell<Option<SourceId>>,
    all: Cell<bool>,
    limits: Slot<RefCell<ProviderLimits>>,
    shows: RefCell<HashMap<SourceId, ShowWaiter>>,
    page_waiters: Slot<PageWaiters>,
}

impl State {
    pub(super) fn new(schedule: Rc<dyn Schedule>) -> Self {
        Self {
            account_streams: Cell::default(),
            resolving: Resolves::default(),
            resolve_ticket: Cell::new(0),
            link_client: Arc::new(Mutex::new(None)),
            library: Slot::default(),
            parts: RefCell::default(),
            single: Slot::default(),
            schedule: RefCell::new(schedule),
            schedule_revision: Cell::new(0),
            shown: Cell::default(),
            all: Cell::new(false),
            limits: Slot::default(),
            shows: RefCell::default(),
            page_waiters: Slot::default(),
        }
    }

    pub(super) fn account_streams(&self) -> Option<u32> {
        self.account_streams.get()
    }

    pub(super) fn set_account_streams(&self, limit: Option<u32>) {
        self.account_streams.set(limit);
    }

    pub(super) fn pending_resolutions(&self) -> &Resolves {
        &self.resolving
    }

    pub(super) fn next_resolve_ticket(&self) -> u64 {
        let next = self.resolve_ticket.get().wrapping_add(1).max(1);
        self.resolve_ticket.set(next);
        next
    }

    pub(super) fn link_client_cache(&self) -> LinkClientCache {
        self.link_client.clone()
    }

    pub(super) fn library_snapshot(&self) -> &Slot<RefCell<Option<LibrarySnapshot>>> {
        &self.library
    }

    pub(super) fn schedule_parts(&self) -> &RefCell<ScheduleParts> {
        &self.parts
    }

    pub(super) fn single_schedule(&self) -> &Slot<RefCell<Option<SingleSchedule>>> {
        &self.single
    }

    pub(super) fn schedule(&self) -> Rc<dyn Schedule> {
        self.schedule.borrow().clone()
    }

    pub(super) fn replace_schedule(&self, schedule: Rc<dyn Schedule>) -> Rc<dyn Schedule> {
        self.schedule.replace(schedule)
    }

    pub(super) fn schedule_revision(&self) -> u64 {
        self.schedule_revision.get()
    }

    pub(super) fn set_schedule_revision(&self, revision: u64) {
        self.schedule_revision.set(revision);
    }

    pub(super) fn shown_source(&self) -> Option<SourceId> {
        self.shown.get()
    }

    pub(super) fn set_shown_source(&self, source: Option<SourceId>) {
        self.shown.set(source);
    }

    pub(super) fn combined(&self) -> bool {
        self.all.get()
    }

    pub(super) fn set_combined(&self, combined: bool) {
        self.all.set(combined);
    }

    pub(super) fn provider_limits(&self) -> &Slot<RefCell<ProviderLimits>> {
        &self.limits
    }

    pub(super) fn enqueue_show(&self, source: SourceId, next: ShowWaiter) -> Option<ShowWaiter> {
        self.shows.borrow_mut().insert(source, next)
    }

    pub(super) fn page_waiters(&self) -> &Slot<PageWaiters> {
        &self.page_waiters
    }
}
