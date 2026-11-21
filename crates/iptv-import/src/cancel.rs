//! Cooperative cancellation without a runtime.
//!
//! [`CancelToken::cancel`] stops an import at the next check: before each
//! request, between body chunks, and *while a request is in flight* (the
//! request future is dropped, which aborts it in reqwest). Dropping the
//! import future also cancels it.

use std::future::{Future, poll_fn};
use std::pin::{Pin, pin};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use crate::error::ImportError;

#[derive(Default, Debug)]
struct Inner {
    canceled: AtomicBool,
    wakers: Mutex<Vec<Option<Waker>>>,
}

/// Shared cancellation flag. Clone it into the UI; call [`CancelToken::cancel`].
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<Inner>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests cancellation and wakes any import waiting on the network.
    pub fn cancel(&self) {
        if !self.0.canceled.swap(true, Ordering::SeqCst) {
            let wakers = std::mem::take(&mut *self.lock());
            for w in wakers.into_iter().flatten() {
                w.wake();
            }
        }
    }

    pub fn is_canceled(&self) -> bool {
        self.0.canceled.load(Ordering::SeqCst)
    }

    /// Resolves once [`CancelToken::cancel`] is called.
    pub fn canceled(&self) -> Canceled<'_> {
        Canceled {
            token: self,
            registration: None,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Option<Waker>>> {
        self.0.wakers.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Future returned by [`CancelToken::canceled`].
#[derive(Debug)]
pub struct Canceled<'a> {
    token: &'a CancelToken,
    registration: Option<usize>,
}

impl Future for Canceled<'_> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = self.get_mut();
        let token = this.token;
        if token.is_canceled() {
            this.unregister();
            return Poll::Ready(());
        }
        let incoming = cx.waker().clone();
        let mut wakers = token.lock();
        // Cancellation sets the flag before taking this lock and draining it.
        if token.is_canceled() {
            drop(wakers);
            this.unregister();
            return Poll::Ready(());
        }
        let slot = *this.registration.get_or_insert_with(|| {
            if let Some(slot) = wakers.iter().position(Option::is_none) {
                slot
            } else {
                wakers.push(None);
                wakers.len() - 1
            }
        });
        let previous = wakers[slot].replace(incoming);
        // User-supplied wakers may run code when dropped, too.
        drop(wakers);
        drop(previous);
        Poll::Pending
    }
}

impl Canceled<'_> {
    fn unregister(&mut self) {
        if let Some(slot) = self.registration.take() {
            let mut wakers = self.token.lock();
            let previous = wakers.get_mut(slot).and_then(Option::take);
            while wakers.last().is_some_and(Option::is_none) {
                wakers.pop();
            }
            drop(wakers);
            drop(previous);
        }
    }
}
impl Drop for Canceled<'_> {
    fn drop(&mut self) {
        self.unregister();
    }
}

/// Runs `fut` unless cancellation comes first.
pub(crate) async fn or_cancel<T>(
    cancel: &CancelToken,
    fut: impl Future<Output = T>,
) -> Result<T, ImportError> {
    if cancel.is_canceled() {
        return Err(ImportError::Canceled);
    }
    let mut fut = pin!(fut);
    let mut stop = pin!(cancel.canceled());
    poll_fn(|cx| {
        if let Poll::Ready(v) = fut.as_mut().poll(cx) {
            return Poll::Ready(Ok(v));
        }
        if stop.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(ImportError::Canceled));
        }
        Poll::Pending
    })
    .await
}

/// `Err(Cancelled)` once cancellation was requested.
pub(crate) fn check(cancel: &CancelToken) -> Result<(), ImportError> {
    if cancel.is_canceled() {
        Err(ImportError::Canceled)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::task::Wake;

    #[derive(Default)]
    struct Count(AtomicUsize);
    impl Wake for Count {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    fn poll(waiter: &mut Canceled<'_>, count: &Arc<Count>) -> Poll<()> {
        let waker = Waker::from(count.clone());
        Pin::new(waiter).poll(&mut Context::from_waker(&waker))
    }

    #[test]
    fn every_live_waiter_wakes_even_after_shared_token_churn() {
        let token = CancelToken::new();
        let counts: Vec<_> = (0..40).map(|_| Arc::new(Count::default())).collect();
        let mut waiters: Vec<_> = counts.iter().map(|_| token.canceled()).collect();
        for (waiter, count) in waiters.iter_mut().zip(&counts) {
            assert!(poll(waiter, count).is_pending());
        }
        for _ in 0..100 {
            assert!(poll(&mut token.canceled(), &Arc::new(Count::default())).is_pending());
        }
        token.cancel();
        for (waiter, count) in waiters.iter_mut().zip(&counts) {
            assert_eq!(count.0.load(Ordering::SeqCst), 1);
            assert!(poll(waiter, count).is_ready());
        }
    }

    #[test]
    fn a_repolled_waiter_only_wakes_its_latest_task() {
        let token = CancelToken::new();
        let previous = Arc::new(Count::default());
        let current = Arc::new(Count::default());
        let mut waiter = token.canceled();
        assert!(poll(&mut waiter, &previous).is_pending());
        assert!(poll(&mut waiter, &current).is_pending());
        token.cancel();
        assert_eq!(previous.0.load(Ordering::SeqCst), 0);
        assert_eq!(current.0.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn dropped_waiters_are_unregistered() {
        let token = CancelToken::new();
        let abandoned = Arc::new(Count::default());
        for _ in 0..100 {
            assert!(poll(&mut token.canceled(), &abandoned).is_pending());
        }
        token.cancel();
        assert_eq!(abandoned.0.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn cancellation_racing_registration_always_completes_or_wakes() {
        for _ in 0..100 {
            let token = CancelToken::new();
            let count = Arc::new(Count::default());
            std::thread::scope(|scope| {
                let token_ref = &token;
                let cancellation = scope.spawn(move || token_ref.cancel());
                let mut waiter = token.canceled();
                let first = poll(&mut waiter, &count);
                cancellation.join().unwrap();
                assert!(first.is_ready() || count.0.load(Ordering::SeqCst) > 0);
                assert!(poll(&mut waiter, &count).is_ready());
            });
        }
    }
}
