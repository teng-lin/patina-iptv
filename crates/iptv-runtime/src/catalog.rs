//! Per-owner catalog, schedule, and playback view state.

use crate::{
    EpgSchedule, LibrarySnapshot, PageWaiters, Resolves, Runtime, Schedule, SingleSchedule, Slot,
    SourceId,
};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, HashMap},
    rc::Rc,
    sync::{Arc, Mutex},
};

pub struct State {
    pub(super) account_streams: Slot<Cell<Option<u32>>>,
    pub(super) resolving: Slot<Resolves>,
    pub(super) link_client:
        Arc<Mutex<Option<(crate::network::NetworkConfig, crate::network::Client)>>>,
    pub(super) library: Slot<RefCell<Option<LibrarySnapshot>>>,
    pub(super) parts: Slot<RefCell<BTreeMap<SourceId, Rc<EpgSchedule>>>>,
    pub(super) single: Slot<RefCell<Option<SingleSchedule>>>,
    pub(super) schedule: RefCell<Rc<dyn Schedule>>,
    pub(super) schedule_revision: Cell<u64>,
    pub(super) shown: Slot<Cell<Option<SourceId>>>,
    pub(super) all: Cell<bool>,
    pub(super) limits: Slot<RefCell<BTreeMap<SourceId, Option<u32>>>>,
    pub(super) shows: RefCell<HashMap<SourceId, futures_channel::oneshot::Receiver<()>>>,
    pub(super) page_waiters: Slot<PageWaiters>,
}

impl State {
    pub(super) fn new(schedule: Rc<dyn Schedule>) -> Self {
        Self {
            account_streams: Slot::default(),
            resolving: Slot::default(),
            link_client: Arc::new(Mutex::new(None)),
            library: Slot::default(),
            parts: Slot::default(),
            single: Slot::default(),
            schedule: RefCell::new(schedule),
            schedule_revision: Cell::new(0),
            shown: Slot::default(),
            all: Cell::new(false),
            limits: Slot::default(),
            shows: RefCell::default(),
            page_waiters: Slot::default(),
        }
    }
}

impl std::ops::Deref for Runtime {
    type Target = State;
    fn deref(&self) -> &Self::Target {
        &self.catalog
    }
}
