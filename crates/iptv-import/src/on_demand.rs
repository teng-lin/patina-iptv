//! Bounded provider detail requests using the same authentication and limits as import.
use crate::ctx::Ctx;
use crate::error::Step;
use crate::{CancelToken, Fetch, ImportError, NoProgress};
use iptv_providers::StalkerError;
use iptv_providers::model::{ContentKind, StalkerCredentials};
use iptv_providers::stalker::StalkerSession;
use iptv_providers::{HttpRequest, HttpResponse};

pub async fn stalker_session<F: Fetch>(
    fetch: &F,
    credentials: &StalkerCredentials,
    cancel: &CancelToken,
) -> Result<StalkerSession, ImportError> {
    crate::stalker::discover(&mut Ctx::new(fetch, &mut NoProgress, cancel), credentials).await
}
pub async fn detail<F: Fetch>(
    fetch: &F,
    request: HttpRequest,
    cancel: &CancelToken,
) -> Result<HttpResponse, ImportError> {
    Ctx::new(fetch, &mut NoProgress, cancel)
        .send(Step::Items(ContentKind::Movie), request)
        .await
}
pub async fn stalker_detail<F: Fetch>(
    fetch: &F,
    session: &mut StalkerSession,
    build: impl Fn(&StalkerSession) -> Result<HttpRequest, StalkerError>,
    cancel: &CancelToken,
) -> Result<HttpResponse, ImportError> {
    let mut progress = NoProgress;
    let mut ctx = Ctx::new(fetch, &mut progress, cancel);
    if !session.is_authorized() {
        crate::stalker::authenticate(&mut ctx, session).await?;
    }
    crate::stalker::call(
        &mut ctx,
        session,
        Step::Items(ContentKind::Series),
        build,
        |r| {
            if iptv_providers::stalker::classify_auth_failure(r).is_some() {
                Err(iptv_providers::ProviderError::Stalker(StalkerError::new(
                    iptv_providers::StalkerErrorKind::AuthFailed,
                )))
            } else {
                Ok(r.clone())
            }
        },
    )
    .await
}
pub async fn stalker_episode_link<F: Fetch>(
    fetch: &F,
    session: &mut StalkerSession,
    cmd: &str,
    episode: Option<u32>,
    cancel: &CancelToken,
) -> Result<String, ImportError> {
    let portal = session.portal().clone();
    let response = stalker_detail(
        fetch,
        session,
        |s| s.create_link(cmd, true, episode),
        cancel,
    )
    .await?;
    iptv_providers::stalker::parse_create_link(&response, &portal, cmd)
        .map_err(|_| ImportError::InvalidConfig("Invalid episode link".into()))
}
