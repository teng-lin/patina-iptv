//! The platform side of `tv.player` in the crate's unit tests, in place
//! of GTK's ([`super::gtk`]): no display, no window. The view only counts
//! the draws asked of it (the tests draw into a surfaceless EGL context's
//! framebuffer themselves, `tests::Gpu`), the component's events are kept
//! for the tests to read, and what mpv's threads queue for the main loop
//! runs when a test pumps it ([`pump`]). Everything mpv does is libmpv's.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Mutex;

use serde_json::Value;

use super::Inner;

type Job = Box<dyn FnOnce() + Send>;
/// A player's id and what to call once a second.
type Tick = (u64, fn(&Inner));

/// What mpv's threads asked the main loop to run (one test at a time runs
/// players, `tests::serial`).
static QUEUE: Mutex<Vec<Job>> = Mutex::new(Vec::new());

thread_local! {
    /// The players' `every_second` calls.
    static TICKS: RefCell<Vec<Tick>> = const { RefCell::new(Vec::new()) };
    /// What the picture-in-picture window was told to show, in order.
    static PIP_SYNCS: RefCell<Vec<Option<u64>>> = const { RefCell::new(Vec::new()) };
}

/// GTK registers the components; nothing to register here.
pub fn register(_components: &nsh_platform_linux::Components) {}

/// The component's side towards the app: the events it emitted. Each must
/// be one the shell can read ([`crate::PlayerEvent::decode`]), so no test
/// passes on an event outside the contract.
#[derive(Clone, Default)]
pub struct ComponentContext(pub Rc<RefCell<Vec<(String, Value)>>>);

impl ComponentContext {
    pub fn emit(&self, name: &str, data: Value) {
        if let Err(e) = crate::PlayerEvent::decode(name, &data) {
            panic!("the player emitted `{name}` {data}: {e}");
        }
        self.0.borrow_mut().push((name.to_owned(), data));
    }
}

/// The view a player draws in: how often it was asked to draw again.
#[derive(Clone, Default)]
pub struct Video(pub Rc<Cell<u32>>);

impl Video {
    pub fn redraw(&self) {
        self.0.set(self.0.get() + 1);
    }

    /// The tests' GL context stays current on their thread.
    pub fn make_current(&self) {}
}

pub fn on_main(f: impl FnOnce() + Send + 'static) {
    QUEUE.lock().unwrap().push(Box::new(f));
}

/// Runs what mpv's threads queued for the main loop; how many ran.
pub fn pump() -> usize {
    let jobs = std::mem::take(&mut *QUEUE.lock().unwrap());
    let n = jobs.len();
    for job in jobs {
        job();
    }
    n
}

pub fn every_second(id: u64, f: fn(&Inner)) {
    TICKS.with(|t| t.borrow_mut().push((id, f)));
}

/// Runs player `id`'s once-a-second call, as GTK's timer would.
pub fn tick(p: &Inner) {
    let f = TICKS.with(|t| {
        t.borrow()
            .iter()
            .find(|(id, _)| *id == p.id)
            .map(|(_, f)| *f)
    });
    f.expect("the player asked for a timer")(p);
}

pub fn pip_sync() {
    PIP_SYNCS.with(|s| s.borrow_mut().push(super::pip::wanted()));
}

/// What the picture-in-picture window was told to show since the last
/// call.
pub fn pip_syncs() -> Vec<Option<u64>> {
    PIP_SYNCS.with(|s| std::mem::take(&mut *s.borrow_mut()))
}
