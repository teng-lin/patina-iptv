//! A queued command may be cancelled; an in-flight atomic commit is reconciled.
use super::*;
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
    if child.is_cancelled() {
        return Err(Error::Cancelled);
    }
    if Instant::now() >= deadline {
        return Err(InspectionError::Deadline.into());
    }
    phase
        .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
        .map_err(|_| Error::Cancelled)?;
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
    reply: impl Future<Output = iptv_store_sqlite::Result<Result<Commit>>> + Send + 'static,
) -> Result<Commit> {
    let Wait {
        control,
        deadline,
        id,
        phase,
        child,
    } = command;
    let task = services.spawn(async move {
        let mut reply = pin!(reply);
        let mut cancelled = pin!(control.cancel.cancelled());
        let race = poll_fn(|cx| {
            // A received commit wins over a simultaneous cancellation.
            if let Poll::Ready(value) = reply.as_mut().poll(cx) {
                return Poll::Ready(Some(value));
            }
            if cancelled.as_mut().poll(cx).is_ready() {
                return Poll::Ready(None);
            }
            Poll::Pending
        });
        match tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), race).await {
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
                        Err(Error::Cancelled)
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
