//! Per-owner import, refresh, and guide-work state.

use crate::{ImportWork, Slot, SourceId, StatusSnapshot, guide_checks, model};
use iptv_import::CancelToken;
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, HashMap},
    rc::{Rc, Weak},
};

type GuideUpdates = Vec<(SourceId, Rc<ImportWork>)>;

pub(crate) struct State {
    pub(super) activity: RefCell<BTreeMap<SourceId, model::Activity>>,
    pub(super) generation: Slot<Cell<u64>>,
    pub(super) imports: Slot<RefCell<HashMap<SourceId, (u64, CancelToken)>>>,
    pub(super) attempts: Slot<RefCell<HashMap<SourceId, i64>>>,
    pub(super) tickets: Slot<Cell<u64>>,
    pub(super) completions: Slot<RefCell<HashMap<SourceId, Weak<ImportWork>>>>,
    pub(super) resume: Slot<Cell<bool>>,
    pub(super) rebuild: Slot<Cell<u64>>,
    pub(super) epoch: Slot<Cell<i64>>,
    pub(super) updating: Slot<RefCell<GuideUpdates>>,
    pub(super) edits: Slot<RefCell<BTreeMap<SourceId, u64>>>,
    pub(super) status: Slot<RefCell<StatusSnapshot>>,
    pub(super) status_revision: Cell<u64>,
    pub(super) wizard_import:
        Slot<RefCell<Option<(iptv_core::add_playlist::ImportId, CancelToken)>>>,
    pub(super) wizard_tickets: Cell<u64>,
    pub(super) guide_checks: RefCell<guide_checks::GuideChecks>,
}

impl State {
    pub(super) fn new(guide_checks: guide_checks::GuideChecks) -> Self {
        Self {
            activity: RefCell::default(),
            generation: Slot::default(),
            imports: Slot::default(),
            attempts: Slot::default(),
            tickets: Slot::default(),
            completions: Slot::default(),
            resume: Slot::default(),
            rebuild: Slot::default(),
            epoch: Slot::default(),
            updating: Slot::default(),
            edits: Slot::default(),
            status: Slot::default(),
            status_revision: Cell::new(0),
            wizard_import: Slot::default(),
            wizard_tickets: Cell::new(0),
            guide_checks: RefCell::new(guide_checks),
        }
    }
}
