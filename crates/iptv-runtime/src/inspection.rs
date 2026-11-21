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
#[cfg(feature = "test-support")]
pub(crate) type ScanHook = Box<dyn FnMut() + Send>;

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
pub(crate) async fn guarded<T>(
    services: &Services,
    control: &InspectionControl,
    deadline: Instant,
    future: impl Future<Output = InspectionResult<T>>,
) -> InspectionResult<T> {
    if !services.is_current() {
        return Err(InspectionError::OwnerClosed);
    }
    if control.cancel.is_canceled() {
        return Err(InspectionError::Canceled);
    }
    let mut work = pin!(future);
    let mut stop = pin!(control.cancel.canceled());
    let mut closed = pin!(services.closed());
    let race = poll_fn(|cx| {
        if closed.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(InspectionError::OwnerClosed));
        }
        if stop.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(InspectionError::Canceled));
        }
        if let Poll::Ready(result) = work.as_mut().poll(cx) {
            return Poll::Ready(result);
        }
        Poll::Pending
    });
    let result = services
        .timeout_at(deadline, race)
        .await
        .unwrap_or(Err(InspectionError::Deadline));
    if !services.is_current() {
        return Err(InspectionError::OwnerClosed);
    }
    result
}

pub(crate) fn query_with_deadline<T: Send + 'static, F>(
    owner: &Runtime,
    control: InspectionControl,
    maximum: Duration,
    operation: F,
) -> impl Future<Output = InspectionResult<T>> + use<T, F>
where
    F: FnOnce(&iptv_store_sqlite::Store) -> iptv_store_sqlite::Result<InspectionResult<T>>
        + Send
        + 'static,
{
    let ready = owner.inspection.ready();
    let services = owner.services();
    let deadline = Instant::now() + control.timeout.min(maximum);
    let token = CancelToken::new();
    let operation_cancel = token.clone();
    let slot = owner.inspection.admit();
    #[cfg(feature = "test-support")]
    let mut scan = owner.inspection_scan.borrow_mut().take();
    let read: std::pin::Pin<
        Box<dyn Future<Output = iptv_store_sqlite::Result<InspectionResult<T>>> + Send>,
    > = match slot {
        Some(slot) => Box::pin(services.analyze(move |store| {
            let _slot = slot;
            #[cfg(feature = "test-support")]
            {
                store.inspection_read_observed(
                    token,
                    deadline,
                    move || {
                        if let Some(mut hook) = scan.take() {
                            hook();
                        }
                    },
                    operation,
                )
            }
            #[cfg(not(feature = "test-support"))]
            {
                store.inspection_read(token, deadline, operation)
            }
        })),
        None => Box::pin(async { Ok(Err(InspectionError::Busy)) }),
    };
    let accepted = services.clone();
    let guard = owner.inspection.register(operation_cancel.clone());
    let task = services.spawn(async move {
        let result = guarded(&accepted, &control, deadline, async move {
            if !ready {
                return Err(InspectionError::Storage);
            }
            read.await.map_err(|_| InspectionError::Storage)?
        })
        .await;
        if result.is_err() {
            operation_cancel.cancel();
        }
        result
    });
    async move {
        let _guard = guard;
        task.await.map_err(|_| InspectionError::OwnerClosed)?
    }
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
    query_with_deadline(owner, control, QUERY_DEADLINE, operation)
}

fn authorized_query<T, F, K>(
    owner: &Runtime,
    control: InspectionControl,
    authorization: crate::parental::Snapshot,
    operation: F,
    channels: K,
) -> impl Future<Output = InspectionResult<T>> + use<T, F, K>
where
    T: Send + 'static,
    F: FnOnce(&iptv_store_sqlite::Store) -> iptv_store_sqlite::Result<InspectionResult<T>>
        + Send
        + 'static,
    K: FnOnce(&T) -> Vec<ChannelRef> + Send + 'static,
{
    let services = owner.services();
    let reading = authorization.clone();
    let read = query(owner, control, move |store| {
        store.inspection_with_access(reading.inspection_access(), |store| {
            let value = match operation(store)? {
                Ok(value) => value,
                Err(error) => return Ok(Err(error)),
            };
            let mut grants = Vec::new();
            for channel in channels(&value) {
                grants.extend(reading.inspection_dependencies(store, &channel)?);
            }
            Ok(Ok((value, grants)))
        })
    });
    async move {
        let (value, grants) = read.await?;
        if !services.is_current() {
            return Err(InspectionError::OwnerClosed);
        }
        if !authorization.inspection_current(&grants) {
            return Err(InspectionError::PermissionDenied);
        }
        Ok(value)
    }
}

impl Runtime {
    pub fn inspect_channel(
        &self,
        key: ChannelRef,
        control: InspectionControl,
    ) -> impl Future<Output = InspectionResult<ChannelInfo>> + use<> {
        let authorization = self.parental_snapshot();
        authorized_query(
            self,
            control,
            authorization,
            move |store| store.inspection_channel(&key),
            |channel| vec![channel.key.clone()],
        )
    }
    pub fn query_channels(
        &self,
        mut request: Query,
        control: InspectionControl,
    ) -> impl Future<Output = InspectionResult<Page<ChannelInfo>>> + use<> {
        let authorization = self.parental_snapshot();
        let owner = format!(
            "{}:{}",
            self.inspection.context(self.services().epoch()),
            authorization.inspection_context()
        );
        authorized_query(
            self,
            control,
            authorization,
            move |store| {
                let cursor = request.cursor.take();
                request.cursor = match unseal(cursor, &owner) {
                    Ok(c) => c,
                    Err(e) => return Ok(Err(e)),
                };
                Ok(store
                    .inspection_channels(&request)?
                    .map(|page| seal(page, &owner)))
            },
            |page| {
                page.items
                    .iter()
                    .map(|channel| channel.key.clone())
                    .collect()
            },
        )
    }
    pub fn query_programs(
        &self,
        mut request: Query,
        window: TimeWindow,
        channel: Option<ChannelRef>,
        control: InspectionControl,
    ) -> impl Future<Output = InspectionResult<Page<ProgramInfo>>> + use<> {
        let subject = channel.clone();
        let authorization = self.parental_snapshot();
        let owner = format!(
            "{}:{}",
            self.inspection.context(self.services().epoch()),
            authorization.inspection_context()
        );
        authorized_query(
            self,
            control,
            authorization,
            move |store| {
                request.cursor = match unseal(request.cursor.take(), &owner) {
                    Ok(c) => c,
                    Err(e) => return Ok(Err(e)),
                };
                Ok(store
                    .inspection_programs(&request, window, channel.as_ref())?
                    .map(|p| seal(p, &owner)))
            },
            move |page| {
                subject
                    .into_iter()
                    .chain(page.items.iter().map(|program| program.channel.clone()))
                    .collect()
            },
        )
    }
    pub fn epg_candidates(
        &self,
        key: ChannelRef,
        mut request: Query,
        control: InspectionControl,
    ) -> impl Future<Output = InspectionResult<Page<Candidate>>> + use<> {
        let subject = key.clone();
        let authorization = self.parental_snapshot();
        let owner = format!(
            "{}:{}",
            self.inspection.context(self.services().epoch()),
            authorization.inspection_context()
        );
        authorized_query(
            self,
            control,
            authorization,
            move |store| {
                request.cursor = match unseal(request.cursor.take(), &owner) {
                    Ok(c) => c,
                    Err(e) => return Ok(Err(e)),
                };
                Ok(store
                    .inspection_candidates(&key, &request)?
                    .map(|p| seal(p, &owner)))
            },
            move |_| vec![subject],
        )
    }
    pub fn epg_audit(
        &self,
        mut request: Query,
        window: TimeWindow,
        control: InspectionControl,
    ) -> impl Future<Output = InspectionResult<Page<Coverage>>> + use<> {
        let authorization = self.parental_snapshot();
        let owner = format!(
            "{}:{}",
            self.inspection.context(self.services().epoch()),
            authorization.inspection_context()
        );
        authorized_query(
            self,
            control,
            authorization,
            move |store| {
                request.cursor = match unseal(request.cursor.take(), &owner) {
                    Ok(c) => c,
                    Err(e) => return Ok(Err(e)),
                };
                Ok(store
                    .inspection_coverage(&request, window)?
                    .map(|p| seal(p, &owner)))
            },
            |page| {
                page.items
                    .iter()
                    .map(|coverage| coverage.channel.key.clone())
                    .collect()
            },
        )
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
