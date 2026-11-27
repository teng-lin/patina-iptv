//! Associates the shell thread and each accepted task with its runtime owner.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use iptv_core::guide::Schedule;

use iptv_runtime::Runtime;

use crate::host;

thread_local! {
    /// The application owner this thread's shell code is working for: the
    /// root's, or, while a task accepted by an earlier root is polled, that
    /// task's ([`in_runtime`]). The runtime crate keeps no such handle of its
    /// own; every call into it is given this one.
    static RUNTIME: RefCell<Option<Rc<Runtime>>> = const { RefCell::new(None) };
}

/// The shell owns one application handle; all library state belongs to it.
///
/// Application-thread code only. A player, JNI or worker thread holds no
/// handle and must be given what it needs; once a root exists, asking for
/// the handle elsewhere is a bug, not a reason to build a second owner.
pub(crate) fn runtime() -> Rc<Runtime> {
    RUNTIME.with(|runtime| {
        runtime
            .borrow_mut()
            .get_or_insert_with(|| {
                // Pre-root shell tests may construct an owner. Once a root
                // exists, callbacks must dispatch to its shell thread; a second
                // dormant owner would silently fork state in release builds.
                assert!(
                    !ROOT_STARTED.load(Ordering::Acquire),
                    "the application handle was requested off the application thread"
                );
                let directory = host::resolve_data_dir()
                    .expect("private storage must be resolved before using runtime services");
                Rc::new(new_runtime(directory))
            })
            .clone()
    })
}

/// An application root installed its owner in this process. Only a guard
/// against asking for the handle on another thread; it selects no owner.
static ROOT_STARTED: AtomicBool = AtomicBool::new(false);

/// Installs the freshly mounted root's owner for later app-thread tasks.
pub(crate) fn install_root(owner: Rc<Runtime>) {
    RUNTIME.set(Some(owner));
    ROOT_STARTED.store(true, Ordering::Release);
}

/// Whether an application owner exists (none before storage is resolved).
pub(crate) fn has_runtime() -> bool {
    RUNTIME.with(|runtime| runtime.borrow().is_some())
}

/// Runs `run` with `owner` as the shell's handle, then puts the previous
/// one back (also when `run` unwinds).
pub(crate) fn with_runtime<R>(owner: Rc<Runtime>, run: impl FnOnce() -> R) -> R {
    struct Restore(Option<Rc<Runtime>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            RUNTIME.set(self.0.take());
        }
    }
    let _restore = Restore(RUNTIME.replace(Some(owner)));
    run()
}

pub(crate) fn new_runtime(directory: std::path::PathBuf) -> Runtime {
    let mut ports = host::ports(
        directory,
        crate::chrome::i18n::system_text(crate::chrome::i18n::key("Patina AirPlay pairing"))
            .to_owned(),
    );
    ports.startup_mark = crate::chrome::startup::mark_with;
    Runtime::new(ports)
}

/// A local task always polls with the owner that accepted it, also after
/// the root (and with it the shell's handle) was recreated.
pub(crate) fn in_runtime(
    future: impl std::future::Future<Output = ()> + 'static,
) -> impl std::future::Future<Output = ()> {
    let owner = runtime();
    let mut future = Box::pin(future);
    std::future::poll_fn(move |context| {
        with_runtime(owner.clone(), || future.as_mut().poll(context))
    })
}

/// The guide data in use (synthetic until a playlist is imported).
pub fn schedule() -> Rc<dyn Schedule> {
    runtime().schedule()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn localized_host_ports_can_be_created_while_installing_an_owner() {
        let directory = tempfile::tempdir().unwrap();
        RUNTIME.with(|slot| {
            let _installing = slot.borrow_mut();
            let _owner = new_runtime(directory.path().to_owned());
        });
    }
}
