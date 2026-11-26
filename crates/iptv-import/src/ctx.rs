//! Shared request plumbing: cancellation, counters, progress.

use iptv_core::import_limits::{ImportLimits, LimitExceeded, LimitKind};
use iptv_providers::{HttpRequest, HttpResponse};

use crate::cancel::{CancelToken, check, or_cancel};
use crate::catalog::ImportStats;
use crate::error::{ImportError, Step};
use crate::fetch::{BodyStream, Fetch, StreamingResponse};
use crate::progress::{Phase, Progress, ProgressSink, REPORT_EVERY_BYTES};

pub(crate) struct Ctx<'a, F, P> {
    pub fetch: &'a F,
    pub cancel: &'a CancelToken,
    pub progress: &'a mut P,
    pub stats: ImportStats,
    pub catalog_sink: Option<&'a mut dyn crate::CatalogSink>,
    pub limits: ImportLimits,
    /// The step named when the time budget runs out.
    pub limit_step: Step,
    #[cfg(debug_assertions)]
    pub allow_private_fixture: bool,
    started: std::time::Duration,
}

impl<'a, F: Fetch, P: ProgressSink> Ctx<'a, F, P> {
    pub fn new(fetch: &'a F, progress: &'a mut P, cancel: &'a CancelToken) -> Self {
        Self {
            fetch,
            cancel,
            progress,
            stats: ImportStats::default(),
            catalog_sink: None,
            limits: ImportLimits::default(),
            limit_step: Step::Playlist,
            #[cfg(debug_assertions)]
            allow_private_fixture: false,
            started: fetch.elapsed(),
        }
    }

    pub fn report(&mut self, progress: Progress) {
        self.progress.report(&progress);
    }

    pub fn phase(&mut self, phase: Phase) {
        self.report(Progress::phase(phase));
    }

    pub fn check(&self) -> Result<(), ImportError> {
        check(self.cancel)?;
        LimitExceeded::check(
            LimitKind::Elapsed,
            self.fetch
                .elapsed()
                .saturating_sub(self.started)
                .as_millis()
                .min(u64::MAX as u128) as u64,
            self.limits.elapsed.as_millis().min(u64::MAX as u128) as u64,
        )
        .map_err(|error| ImportError::Limit {
            step: self.limit_step,
            error,
        })
    }

    /// Before the same download is attempted again: its bytes count from
    /// zero (the first attempt's are discarded), its time does not.
    pub fn restart_bytes(&mut self) {
        self.stats.bytes = 0;
    }

    /// A buffered request. Network failures carry `step`; HTTP status is
    /// left to the caller (probes treat errors as "try the next").
    pub async fn send(
        &mut self,
        step: Step,
        mut req: HttpRequest,
    ) -> Result<HttpResponse, ImportError> {
        self.check()?;
        req.timeout = Some(
            self.limits
                .elapsed
                .saturating_sub(self.fetch.elapsed().saturating_sub(self.started)),
        );
        let resp = or_cancel(self.cancel, self.fetch.fetch_stream(req))
            .await?
            .map_err(|error| ImportError::Network { step, error })?;
        self.stats.requests += 1;
        let status = resp.status;
        let mut body = resp.body;
        let mut bytes = Vec::new();
        loop {
            self.check()?;
            let Some(chunk) = or_cancel(self.cancel, body.next_chunk())
                .await?
                .map_err(|error| ImportError::Network { step, error })?
            else {
                break;
            };
            self.account_bytes(step, chunk.len())?;
            // Only small probes and metadata use this path; list endpoints stream.
            LimitExceeded::check(
                LimitKind::RecordBytes,
                bytes.len().saturating_add(chunk.len()) as u64,
                self.limits.input_bytes.min(128 << 20),
            )
            .map_err(|error| ImportError::Limit { step, error })?;
            bytes.extend_from_slice(&chunk);
        }
        Ok(HttpResponse::new(status, bytes))
    }

    fn account_bytes(&mut self, step: Step, bytes: usize) -> Result<(), ImportError> {
        self.check()?;
        self.stats.bytes = self.stats.bytes.saturating_add(bytes as u64);
        LimitExceeded::check(
            LimitKind::InputBytes,
            self.stats.bytes,
            self.limits.input_bytes,
        )
        .map_err(|error| ImportError::Limit { step, error })
    }

    /// A streamed request whose body goes to `sink` chunk by chunk.
    /// Non-success statuses are errors ([`ImportError::http`]).
    pub async fn stream(
        &mut self,
        step: Step,
        phase: Phase,
        mut req: HttpRequest,
        mut sink: impl FnMut(&[u8], bool) -> Result<u64, ImportError>,
    ) -> Result<u64, ImportError> {
        self.check()?;
        req.timeout = Some(
            self.limits
                .elapsed
                .saturating_sub(self.fetch.elapsed().saturating_sub(self.started)),
        );
        let resp: StreamingResponse<F::Body> = or_cancel(self.cancel, self.fetch.fetch_stream(req))
            .await?
            .map_err(|error| ImportError::Network { step, error })?;
        self.stats.requests += 1;
        if !resp.is_success() {
            return Err(ImportError::http(step, resp.status));
        }
        let mut body = resp.body;
        let mut progress = Progress::phase(phase);
        progress.total_bytes = resp.content_length;
        let mut last_report = 0;
        loop {
            self.check()?;
            let chunk = or_cancel(self.cancel, body.next_chunk())
                .await?
                .map_err(|error| ImportError::Network { step, error })?;
            let Some(chunk) = chunk else { break };
            progress.bytes += chunk.len() as u64;
            self.account_bytes(step, chunk.len())?;
            progress.items = sink(&chunk, resp.same_request_origin)?;
            if progress.bytes - last_report >= REPORT_EVERY_BYTES {
                last_report = progress.bytes;
                self.report(progress.clone());
            }
        }
        self.report(progress.clone());
        Ok(progress.bytes)
    }
}
