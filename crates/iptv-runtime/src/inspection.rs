//! UI-independent source queries and guide evidence inspection.
//! All expensive reads run on a bounded analysis lane; probes use the owner's
//! bounded worker pool and existing transport policy. No browser state is read.
mod probe;
use crate::{Runtime, store_service::Services};
use iptv_import::CancelToken;
pub use iptv_library::inspection::*;
pub(crate) use probe::Vault;
pub use probe::{ProbeLease, ProbeRequest, ProbeTarget};
use std::{
    future::{Future, poll_fn},
    pin::pin,
    task::Poll,
    time::{Duration, Instant},
};

pub const QUERY_DEADLINE: Duration = Duration::from_secs(3);

#[derive(Clone, Debug)]
pub struct InspectionControl {
    pub cancel: CancelToken,
    /// Requested end-to-end budget, capped by the operation's public maximum.
    pub timeout: Duration,
}
impl Default for InspectionControl {
    fn default() -> Self {
        Self {
            cancel: CancelToken::new(),
            timeout: QUERY_DEADLINE,
        }
    }
}
struct CancelOnDrop(CancelToken);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

pub(crate) async fn guarded<T>(
    services: &Services,
    control: &InspectionControl,
    deadline: Instant,
    future: impl Future<Output = InspectionResult<T>>,
) -> InspectionResult<T> {
    if !services.is_current() {
        return Err(InspectionError::OwnerClosed);
    }
    if control.cancel.is_cancelled() {
        return Err(InspectionError::Cancelled);
    }
    let mut work = pin!(future);
    let mut stop = pin!(control.cancel.cancelled());
    let race = poll_fn(|cx| {
        if stop.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(InspectionError::Cancelled));
        }
        if let Poll::Ready(result) = work.as_mut().poll(cx) {
            return Poll::Ready(result);
        }
        Poll::Pending
    });
    let result = tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), race)
        .await
        .map_err(|_| InspectionError::Deadline)?;
    if !services.is_current() {
        return Err(InspectionError::OwnerClosed);
    }
    result
}

fn query<T: Send + 'static, F>(
    owner: &Runtime,
    control: InspectionControl,
    operation: F,
) -> impl Future<Output = InspectionResult<T>> + use<T, F>
where
    F: FnOnce(&iptv_store_sqlite::Store) -> iptv_store_sqlite::Result<InspectionResult<T>>
        + Send
        + 'static,
{
    let services = owner.services();
    let deadline = Instant::now() + control.timeout.min(QUERY_DEADLINE);
    let token = control.cancel.clone();
    let read = services.analyze(move |store| store.inspection_read(token, deadline, operation));
    let accepted = services.clone();
    let cancel_on_drop = control.cancel.clone();
    let task = services.spawn(async move {
        guarded(&accepted, &control, deadline, async move {
            read.await.map_err(|_| InspectionError::Storage)?
        })
        .await
    });
    let guard = CancelOnDrop(cancel_on_drop);
    async move {
        let _guard = guard;
        task.await.map_err(|_| InspectionError::OwnerClosed)?
    }
}

impl Runtime {
    pub fn inspect_channel(
        &self,
        key: ChannelRef,
        control: InspectionControl,
    ) -> impl Future<Output = InspectionResult<ChannelInfo>> + use<> {
        query(self, control, move |store| store.inspection_channel(&key))
    }
    pub fn query_channels(
        &self,
        mut request: Query,
        control: InspectionControl,
    ) -> impl Future<Output = InspectionResult<Page<ChannelInfo>>> + use<> {
        let owner = self.inspection.context(self.services().epoch());
        query(self, control, move |store| {
            let cursor = request.cursor.take();
            request.cursor = match unseal(cursor, &owner) {
                Ok(c) => c,
                Err(e) => return Ok(Err(e)),
            };
            Ok(store
                .inspection_channels(&request)?
                .map(|page| seal(page, &owner)))
        })
    }
    pub fn query_programmes(
        &self,
        mut request: Query,
        window: TimeWindow,
        channel: Option<ChannelRef>,
        control: InspectionControl,
    ) -> impl Future<Output = InspectionResult<Page<ProgrammeInfo>>> + use<> {
        let owner = self.inspection.context(self.services().epoch());
        query(self, control, move |store| {
            request.cursor = match unseal(request.cursor.take(), &owner) {
                Ok(c) => c,
                Err(e) => return Ok(Err(e)),
            };
            Ok(store
                .inspection_programmes(&request, window, channel.as_ref())?
                .map(|p| seal(p, &owner)))
        })
    }
    pub fn epg_candidates(
        &self,
        key: ChannelRef,
        mut request: Query,
        control: InspectionControl,
    ) -> impl Future<Output = InspectionResult<Page<Candidate>>> + use<> {
        let owner = self.inspection.context(self.services().epoch());
        query(self, control, move |store| {
            request.cursor = match unseal(request.cursor.take(), &owner) {
                Ok(c) => c,
                Err(e) => return Ok(Err(e)),
            };
            Ok(store
                .inspection_candidates(&key, &request)?
                .map(|p| seal(p, &owner)))
        })
    }
    pub fn epg_audit(
        &self,
        mut request: Query,
        window: TimeWindow,
        control: InspectionControl,
    ) -> impl Future<Output = InspectionResult<Page<Coverage>>> + use<> {
        let owner = self.inspection.context(self.services().epoch());
        query(self, control, move |store| {
            request.cursor = match unseal(request.cursor.take(), &owner) {
                Ok(c) => c,
                Err(e) => return Ok(Err(e)),
            };
            Ok(store
                .inspection_coverage(&request, window)?
                .map(|p| seal(p, &owner)))
        })
    }
}
fn seal<T>(mut page: Page<T>, owner: &str) -> Page<T> {
    if let Some(cursor) = &mut page.next {
        cursor.context = format!("{owner}:{}", cursor.context);
    }
    page
}
fn unseal(cursor: Option<Cursor>, owner: &str) -> InspectionResult<Option<Cursor>> {
    cursor
        .map(|mut c| {
            c.context = c
                .context
                .strip_prefix(&format!("{owner}:"))
                .ok_or(InspectionError::StaleCursor)?
                .to_owned();
            Ok(c)
        })
        .transpose()
}
