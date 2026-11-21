//! Playback links: the one provider round trip playback needs (Stalker
//! `create_link`), driven through the same [`Fetch`] as the import.

use iptv_providers::stalker::{StalkerSession, parse_create_link};

use crate::cancel::CancelToken;
use crate::ctx::Ctx;
use crate::error::{ImportError, Step};
use crate::fetch::Fetch;
use crate::progress::NoProgress;
use crate::stalker::{authenticate, call};

/// Asks the portal for a temporary stream URL for `cmd` (live unless
/// `vod`). Authenticates first when the session is not ready, and
/// re-authenticates once when the portal rejects the token; `session` then
/// holds the new token, so keep it for the next link. The URL is valid for
/// seconds: open it right away and never store it.
pub async fn stalker_link<F: Fetch>(
    fetch: &F,
    session: &mut StalkerSession,
    cmd: &str,
    vod: bool,
    cancel: &CancelToken,
) -> Result<String, ImportError> {
    let mut progress = NoProgress;
    let mut ctx = Ctx::new(fetch, &mut progress, cancel);
    ctx.check()?;
    if !session.is_authorized() {
        session.reset();
        authenticate(&mut ctx, session).await?;
    }
    let portal = session.portal().clone();
    call(
        &mut ctx,
        session,
        Step::StreamLink,
        |s| s.create_link(cmd, vod, None),
        |resp| parse_create_link(resp, &portal, cmd),
    )
    .await
}

/// Authorize playback from saved credentials without importing a catalog.
/// The session stays in memory and follows the same endpoint/auth policy as an import.
pub async fn stalker_session<F: Fetch>(
    fetch: &F,
    credentials: &iptv_providers::model::StalkerCredentials,
    cancel: &CancelToken,
) -> Result<StalkerSession, ImportError> {
    let mut progress = NoProgress;
    let mut ctx = Ctx::new(fetch, &mut progress, cancel);
    crate::stalker::discover(&mut ctx, credentials).await
}
