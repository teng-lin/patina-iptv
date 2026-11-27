use crate::{Cell, RefCell, Slot};

pub(crate) struct State {
    favorites: Slot<RefCell<crate::favorites::State>>,
    revision: Slot<Cell<u64>>,
    edits: RefCell<std::collections::HashMap<crate::model::SourceId, u64>>,
    watch_sort_dirty: RefCell<std::collections::BTreeSet<crate::model::SourceId>>,
}

impl State {
    pub(super) fn new(favorites: crate::favorites::State) -> Self {
        Self {
            favorites: Slot(RefCell::new(favorites)),
            revision: Slot::default(),
            edits: RefCell::default(),
            watch_sort_dirty: RefCell::default(),
        }
    }

    pub(super) fn favorites(&self) -> &Slot<RefCell<crate::favorites::State>> {
        &self.favorites
    }
    pub(super) fn revision(&self) -> &Slot<Cell<u64>> {
        &self.revision
    }
    pub(super) fn edits(&self) -> &RefCell<std::collections::HashMap<crate::model::SourceId, u64>> {
        &self.edits
    }
    pub(super) fn watch_sort_dirty(
        &self,
    ) -> &RefCell<std::collections::BTreeSet<crate::model::SourceId>> {
        &self.watch_sort_dirty
    }
}
