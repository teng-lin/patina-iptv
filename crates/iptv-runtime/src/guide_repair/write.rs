//! A queued command may be canceled; an in-flight atomic commit is reconciled.
use super::*;
use crate::error::AdapterError as StoreError;
use crate::store_service::Services;
use std::{
    future::poll_fn,
    pin::pin,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
    task::Poll,
};

pub(super) type Phase = Arc<AtomicU8>;
pub(super) struct CancelOnDrop(pub CancelToken);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

pub(super) fn begin(phase: &Phase, child: &CancelToken, deadline: Instant) -> Result<()> {
    if child.is_canceled() {
        return Err(Error::Canceled);
    }
    if Instant::now() >= deadline {
        return Err(InspectionError::Deadline.into());
    }
    phase
        .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
        .map_err(|_| Error::Canceled)?;
    Ok(())
}

pub(super) struct Wait {
    pub control: InspectionControl,
    pub deadline: Instant,
    pub id: String,
    pub phase: Phase,
    pub child: CancelToken,
}
pub(super) async fn wait(
    services: &Services,
    command: Wait,
    reply: impl Future<Output = std::result::Result<Result<Commit>, StoreError>> + Send + 'static,
) -> Result<Commit> {
    let Wait {
        control,
        deadline,
        id,
        phase,
        child,
    } = command;
    let timer = services.clone();
    let task = services.spawn(async move {
        let mut reply = pin!(reply);
        let mut canceled = pin!(control.cancel.canceled());
        let race = poll_fn(|cx| {
            // A received commit wins over a simultaneous cancellation.
            if let Poll::Ready(value) = reply.as_mut().poll(cx) {
                return Poll::Ready(Some(value));
            }
            if canceled.as_mut().poll(cx).is_ready() {
                return Poll::Ready(None);
            }
            Poll::Pending
        });
        match timer.timeout_at(deadline, race).await {
            Ok(Some(value)) => value.map_err(|_| {
                if phase.load(Ordering::SeqCst) == 1 {
                    Error::CompletionUnknown {
                        operation: id.clone(),
                    }
                } else {
                    Error::Storage
                }
            })?,
            stopped => {
                child.cancel();
                if phase
                    .compare_exchange(0, 3, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                    || phase.load(Ordering::SeqCst) == 3
                {
                    if stopped.is_ok() {
                        Err(Error::Canceled)
                    } else {
                        Err(InspectionError::Deadline.into())
                    }
                } else {
                    Err(Error::CompletionUnknown { operation: id })
                }
            }
        }
    });
    task.await.map_err(|_| InspectionError::OwnerClosed)?
}
