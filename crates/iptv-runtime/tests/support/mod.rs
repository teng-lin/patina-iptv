//! Drives a future on the calling thread, as a host's executor would.
use std::{
    future::Future,
    sync::Arc,
    task::{Context, Poll, Wake},
};

struct Unpark(std::thread::Thread);
impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

pub fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Arc::new(Unpark(std::thread::current())).into();
    let mut context = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
        std::thread::park();
    }
}
