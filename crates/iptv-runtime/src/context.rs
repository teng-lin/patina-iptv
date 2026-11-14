//! Local tasks always poll with the owner that accepted them.
use crate::Runtime;
use std::{
    cell::RefCell,
    future::Future,
    rc::{Rc, Weak},
};

thread_local! {
    static CURRENT: RefCell<Option<Weak<Runtime>>> = const { RefCell::new(None) };
}

pub fn set_current(owner: &Rc<Runtime>) {
    CURRENT.set(Some(Rc::downgrade(owner)));
}

pub fn try_current() -> Option<Rc<Runtime>> {
    CURRENT.with(|owner| owner.borrow().as_ref().and_then(Weak::upgrade))
}

pub fn current() -> Rc<Runtime> {
    try_current().unwrap_or_else(|| {
        #[cfg(test)]
        {
            // Unit tests never discover private paths or an OS credential broker.
            thread_local! { static TEST_OWNER: Rc<Runtime> = Rc::default(); }
            TEST_OWNER.with(Clone::clone)
        }
        #[cfg(not(test))]
        panic!("runtime context must be installed before application work starts")
    })
}

pub fn with_runtime<R>(owner: Rc<Runtime>, run: impl FnOnce() -> R) -> R {
    struct Restore(Option<Weak<Runtime>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            CURRENT.set(self.0.take());
        }
    }
    let _restore = Restore(CURRENT.replace(Some(Rc::downgrade(&owner))));
    owner.with_context(run)
}

pub fn in_runtime<F: Future + 'static>(future: F) -> impl Future<Output = F::Output> + 'static {
    let owner = current();
    let mut future = Box::pin(owner.scope_future(future));
    std::future::poll_fn(move |context| {
        with_runtime(owner.clone(), || future.as_mut().poll(context))
    })
}
