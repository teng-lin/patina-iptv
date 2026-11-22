//! Stalker/Ministra portals in a build without them (feature `stalker`
//! off): every entry point the import, playback links and detail
//! requests use fails with [`ImportError::Unsupported`] before any
//! request, so a stored Stalker source shows as unsupported.

use iptv_providers::model::StalkerCredentials;
use iptv_providers::stalker::StalkerSession;
use iptv_providers::{HttpRequest, HttpResponse, ProviderError, StalkerError};

use crate::catalog::{Catalog, ImportOptions};
use crate::ctx::Ctx;
use crate::error::{ImportError, Step};
use crate::fetch::Fetch;
use crate::progress::ProgressSink;

/// What [`ImportError::Unsupported`] names.
const STALKER: &str = "Stalker portals";

pub(crate) async fn import_stalker<F: Fetch, P: ProgressSink>(
    _ctx: &mut Ctx<'_, F, P>,
    _creds: &StalkerCredentials,
    _opts: &ImportOptions,
) -> Result<Catalog, ImportError> {
    Err(ImportError::Unsupported(STALKER))
}

pub(crate) async fn discover<F: Fetch, P: ProgressSink>(
    _ctx: &mut Ctx<'_, F, P>,
    _creds: &StalkerCredentials,
) -> Result<StalkerSession, ImportError> {
    Err(ImportError::Unsupported(STALKER))
}

pub(crate) async fn authenticate<F: Fetch, P: ProgressSink>(
    _ctx: &mut Ctx<'_, F, P>,
    _session: &mut StalkerSession,
) -> Result<(), ImportError> {
    Err(ImportError::Unsupported(STALKER))
}

pub(crate) async fn call<F: Fetch, P: ProgressSink, T>(
    _ctx: &mut Ctx<'_, F, P>,
    _session: &mut StalkerSession,
    _step: Step,
    _build: impl Fn(&StalkerSession) -> Result<HttpRequest, StalkerError>,
    _parse: impl Fn(&HttpResponse) -> Result<T, ProviderError>,
) -> Result<T, ImportError> {
    Err(ImportError::Unsupported(STALKER))
}
