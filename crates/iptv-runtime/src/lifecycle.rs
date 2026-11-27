use crate::{BrowserObserver, Cell, EventSink, RefCell, TaskBinding};

pub(crate) struct State {
    events: RefCell<Option<EventSink>>,
    tasks: TaskBinding,
    browser: RefCell<Option<BrowserObserver>>,
    foreground: Cell<bool>,
    scheduler_started: Cell<bool>,
    shutdown_requested: Cell<bool>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            events: RefCell::new(None),
            tasks: TaskBinding::default(),
            browser: RefCell::default(),
            foreground: Cell::new(true),
            scheduler_started: Cell::new(false),
            shutdown_requested: Cell::new(false),
        }
    }
}

impl State {
    pub(super) fn events(&self) -> &RefCell<Option<EventSink>> {
        &self.events
    }
    pub(super) fn tasks(&self) -> &TaskBinding {
        &self.tasks
    }
    pub(super) fn browser(&self) -> &RefCell<Option<BrowserObserver>> {
        &self.browser
    }
    pub(super) fn foreground(&self) -> &Cell<bool> {
        &self.foreground
    }
    pub(super) fn scheduler_started(&self) -> &Cell<bool> {
        &self.scheduler_started
    }
    pub(super) fn shutdown_requested(&self) -> &Cell<bool> {
        &self.shutdown_requested
    }
}
