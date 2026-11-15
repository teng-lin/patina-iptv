//! Stalker session lifecycle as a small state machine.
//!
//! ```text
//!            poll → handshake            profile ok
//! NeedsHandshake ───────────▶ Authenticating ───────────▶ Authorized
//!       ▲                      │  status 2 → do_auth → profile(second step)
//!       │ reset()              │  status 1 / rejected → Failed
//!       │                      ▼
//!     Failed ◀── repeated ── TokenExpired ◀── API auth failure (check_api_response)
//!                rejection        │
//!                                 └── poll → fresh handshake (empty token)
//! ```
//!
//! The driver loop is: `while let AuthStep::Send(req) = session.poll() {
//! session.on_auth_response(&fetch(req))?; }`, then build API requests. When
//! [`StalkerSession::check_api_response`] returns
//! [`ApiError::Reauthenticate`], run the loop again and retry the request
//! once.
//!
//! Token facts (Stalker 4.9.35): the handshake is idempotent — re-presenting
//! the current token returns it unchanged — and tokens have no TTL; another
//! device's `get_profile` on the same MAC invalidates them. A resumed token
//! that comes back unchanged (and not `not_valid`) skips `get_profile` when
//! the watchdog cadence is already known.
//!
//! Simple (token-free) portals are always `Authorized`.

use serde_json::Value;

use super::parse::{
    AuthFailure, StalkerProfile, classify_auth_failure, is_device_conflict, js_payload,
    portal_text, watchdog_from,
};
use super::{PortalMode, StalkerPortal};
use crate::error::{ProviderError, StalkerError, StalkerErrorKind};
use crate::http::{HttpRequest, HttpResponse, json_body};
use crate::lenient::Obj;
use crate::model::StreamLocator;
use crate::secret::Secret;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionState {
    NeedsHandshake,
    Authenticating,
    Authorized,
    /// An API call was rejected; the next poll re-handshakes.
    TokenExpired,
    /// Terminal until [`StalkerSession::reset`].
    Failed(StalkerError),
}

/// What the driver should do next.
#[derive(Debug)]
pub enum AuthStep {
    /// Perform this request and pass the response to `on_auth_response`.
    Send(HttpRequest),
    /// A request is outstanding.
    Waiting,
    /// Authorized: API requests can be built.
    Ready,
    Failed(StalkerError),
}

/// Why an API response could not be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApiError {
    /// The token was rejected; re-run the auth loop and retry once.
    Reauthenticate,
    Provider(ProviderError),
}

impl From<ProviderError> for ApiError {
    fn from(e: ProviderError) -> Self {
        Self::Provider(e)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outstanding {
    Handshake,
    Profile { second_step: bool },
    DoAuth,
}

/// Session for one portal account. `Debug` is safe to log.
#[derive(Clone, Debug)]
pub struct StalkerSession {
    portal: StalkerPortal,
    state: SessionState,
    token: Option<Secret>,
    random: String,
    not_valid: bool,
    /// Token to re-present on the next handshake (resume).
    present: Option<Secret>,
    known_watchdog: Option<u32>,
    /// Sent, awaiting its response.
    outstanding: Option<Outstanding>,
    /// Chosen by the last response, sent by the next poll.
    queued: Option<Outstanding>,
    /// Profile text of the request that asked for a login (kept for do_auth refusals).
    login_text: Option<String>,
    profile: Option<StalkerProfile>,
    rejections_since_success: u8,
}

impl StalkerSession {
    pub fn new(portal: StalkerPortal) -> Self {
        let state = if portal.mode() == PortalMode::Simple {
            SessionState::Authorized
        } else {
            SessionState::NeedsHandshake
        };
        Self {
            portal,
            state,
            token: None,
            random: String::new(),
            not_valid: false,
            present: None,
            known_watchdog: None,
            outstanding: None,
            queued: None,
            login_text: None,
            profile: None,
            rejections_since_success: 0,
        }
    }

    /// Resumes a persisted session: the token is re-presented in the first
    /// handshake. Pass the persisted watchdog cadence so an unchanged token
    /// can skip `get_profile`. Only resume a token negotiated for the same
    /// endpoint, MAC, device identity and login: re-presenting it elsewhere
    /// would disclose it to another portal.
    pub fn resume(portal: StalkerPortal, token: Secret, watchdog_secs: Option<u32>) -> Self {
        let mut s = Self::new(portal);
        s.present = Some(token).filter(|t| !t.is_empty());
        s.known_watchdog = watchdog_secs;
        s
    }

    pub fn portal(&self) -> &StalkerPortal {
        &self.portal
    }

    pub fn state(&self) -> &SessionState {
        &self.state
    }

    pub fn is_authorized(&self) -> bool {
        self.state == SessionState::Authorized
    }

    /// The bearer token to persist after authorization (full portals).
    pub fn token(&self) -> Option<&Secret> {
        self.token.as_ref()
    }

    /// The decoded `get_profile`, unless the profile was skipped on resume.
    pub fn profile(&self) -> Option<&StalkerProfile> {
        self.profile.as_ref()
    }

    /// Seconds between watchdog pings; persist it with the token.
    pub fn watchdog_interval_secs(&self) -> u32 {
        self.profile
            .as_ref()
            .map(|p| p.watchdog_timeout)
            .or(self.known_watchdog)
            .unwrap_or(super::parse::DEFAULT_WATCHDOG)
    }

    /// Back to `NeedsHandshake` (e.g. the user retried after a failure).
    pub fn reset(&mut self) {
        self.outstanding = None;
        self.queued = None;
        self.token = None;
        self.rejections_since_success = 0;
        self.state = if self.portal.mode() == PortalMode::Simple {
            SessionState::Authorized
        } else {
            SessionState::NeedsHandshake
        };
    }

    /// The next authentication step.
    pub fn poll(&mut self) -> AuthStep {
        if self.outstanding.is_some() {
            return AuthStep::Waiting;
        }
        if let Some(step) = self.queued.take()
            && let Some(req) = self.request_for(step)
        {
            self.outstanding = Some(step);
            return AuthStep::Send(req);
        }
        match &self.state {
            SessionState::Authorized => AuthStep::Ready,
            SessionState::Failed(e) => AuthStep::Failed(e.clone()),
            SessionState::Authenticating => AuthStep::Waiting,
            SessionState::NeedsHandshake | SessionState::TokenExpired => {
                self.state = SessionState::Authenticating;
                self.outstanding = Some(Outstanding::Handshake);
                AuthStep::Send(self.portal.handshake_request(self.present.as_ref()))
            }
        }
    }

    fn fail(&mut self, err: StalkerError) -> StalkerError {
        self.outstanding = None;
        self.queued = None;
        self.state = SessionState::Failed(err.clone());
        err
    }

    fn send_next(&mut self, next: Outstanding) {
        self.queued = Some(next);
    }

    fn request_for(&self, step: Outstanding) -> Option<HttpRequest> {
        let token = self.token.as_ref()?;
        Some(match step {
            Outstanding::Handshake => self.portal.handshake_request(self.present.as_ref()),
            Outstanding::Profile { second_step } => {
                self.portal
                    .profile_request(token, &self.random, second_step, self.not_valid)
            }
            Outstanding::DoAuth => self.portal.do_auth_request(token),
        })
    }

    /// Feeds the response to the request from the last [`AuthStep::Send`].
    /// Afterwards `poll` returns the follow-up step, `Ready` or `Failed`.
    pub fn on_auth_response(&mut self, response: &HttpResponse) -> Result<(), StalkerError> {
        let Some(step) = self.outstanding.take() else {
            return Ok(());
        };
        if let Some(f) = classify_auth_failure(response) {
            return Err(self.fail(f.into_error()));
        }
        let body = match json_body(response) {
            Ok(b) => b,
            Err(e) => {
                return Err(self.fail(StalkerError::with_text(
                    StalkerErrorKind::Portal,
                    Some(e.to_string()),
                )));
            }
        };
        let empty = serde_json::Map::new();
        let js = body.get("js").and_then(Obj::of).unwrap_or(Obj(&empty));
        match step {
            Outstanding::Handshake => self.on_handshake(js),
            Outstanding::Profile { second_step } => {
                self.on_profile(js, body.get("js"), second_step)
            }
            Outstanding::DoAuth => {
                if body.get("js") == Some(&Value::Bool(true)) {
                    self.send_next(Outstanding::Profile { second_step: true });
                    Ok(())
                } else {
                    Err(self.fail(StalkerError::with_text(
                        StalkerErrorKind::LoginRejected,
                        self.login_text.clone(),
                    )))
                }
            }
        }
    }

    fn on_handshake(&mut self, js: Obj<'_>) -> Result<(), StalkerError> {
        let Some(token) = js.str("token").map(Secret::new) else {
            return Err(self.fail(StalkerError::new(StalkerErrorKind::NoToken)));
        };
        self.not_valid = js.flag("not_valid");
        // `random` only feeds `metrics`; derive one deterministically when absent.
        self.random = js
            .str("random")
            .unwrap_or_else(|| sha1_smol::Sha1::from(token.expose()).digest().to_string());
        let reused = self.present.as_ref() == Some(&token)
            && !self.not_valid
            && self.known_watchdog.is_some();
        self.token = Some(token);
        self.present = None;
        if reused {
            self.state = SessionState::Authorized;
            return Ok(());
        }
        self.send_next(Outstanding::Profile { second_step: false });
        Ok(())
    }

    fn on_profile(
        &mut self,
        js: Obj<'_>,
        raw: Option<&Value>,
        second_step: bool,
    ) -> Result<(), StalkerError> {
        if !matches!(raw, Some(Value::Object(_))) {
            return Err(self.fail(StalkerError::with_text(StalkerErrorKind::Blocked, None)));
        }
        let text = portal_text(js);
        let status = js.i64("status");
        let refused_text = js.str("block_msg").is_some_and(|b| !b.trim().is_empty());
        match status {
            Some(2) if second_step => Err(self.fail(StalkerError::with_text(
                StalkerErrorKind::LoginRejected,
                text,
            ))),
            Some(2) if self.portal.has_login() => {
                self.login_text = text;
                self.send_next(Outstanding::DoAuth);
                Ok(())
            }
            Some(2) => Err(self.fail(StalkerError::with_text(
                StalkerErrorKind::LoginRequired,
                text,
            ))),
            Some(0) | None if !refused_text => {
                self.profile = Some(StalkerProfile {
                    watchdog_timeout: watchdog_from(js),
                    timeslot: js.u32("timeslot"),
                    account_name: js.first_text(&["fname", "login", "name"]),
                    expires: js.first_text(&[
                        "expire_billing_date",
                        "end_date",
                        "tariff_expired_date",
                    ]),
                });
                self.known_watchdog = self.profile.as_ref().map(|p| p.watchdog_timeout);
                self.state = SessionState::Authorized;
                Ok(())
            }
            // status 1, any other status, or refusal text without a status.
            _ => {
                let kind = if text.as_deref().is_some_and(is_device_conflict) {
                    StalkerErrorKind::DeviceConflict
                } else {
                    StalkerErrorKind::Blocked
                };
                Err(self.fail(StalkerError::with_text(kind, text)))
            }
        }
    }

    /// Builds an API request. Errors with `NotAuthorized` before the session
    /// is ready.
    pub fn api_request(&self, params: &[(&str, &str)]) -> Result<HttpRequest, StalkerError> {
        if !self.is_authorized() {
            return Err(StalkerError::new(StalkerErrorKind::NotAuthorized));
        }
        Ok(self.portal.request(params, self.token.as_ref()))
    }

    /// Validates an API response and returns its `js` payload. An auth
    /// failure on a full portal moves the session to `TokenExpired` and asks
    /// for re-authentication; two in a row without a success in between fail
    /// the session (`AuthLoop`). On a simple portal it is an error: the mode
    /// or endpoint is probably wrong (re-run discovery).
    pub fn check_api_response(&mut self, response: &HttpResponse) -> Result<Value, ApiError> {
        if let Some(f) = classify_auth_failure(response) {
            return Err(self.on_rejected(f));
        }
        let js = js_payload(response)?;
        self.rejections_since_success = 0;
        Ok(js)
    }

    fn on_rejected(&mut self, failure: AuthFailure) -> ApiError {
        if self.portal.mode() == PortalMode::Simple {
            return ApiError::Provider(failure.into_error().into());
        }
        self.rejections_since_success += 1;
        self.token = None;
        if self.rejections_since_success >= 2 {
            let err = StalkerError::with_text(
                StalkerErrorKind::AuthLoop,
                failure.into_error().portal_text,
            );
            self.state = SessionState::Failed(err.clone());
            return ApiError::Provider(err.into());
        }
        self.state = SessionState::TokenExpired;
        ApiError::Reauthenticate
    }

    // ---- API request builders -------------------------------------------------

    /// Live genres (`itv/get_genres`).
    pub fn genres(&self) -> Result<HttpRequest, StalkerError> {
        self.api_request(&[("type", "itv"), ("action", "get_genres")])
    }

    /// One page (1-based) of a genre's channels; `"*"` for all.
    pub fn channels_page(&self, genre: &str, page: u32) -> Result<HttpRequest, StalkerError> {
        let p = page.max(1).to_string();
        self.api_request(&[
            ("type", "itv"),
            ("action", "get_ordered_list"),
            ("genre", genre),
            ("force_ch_link_check", ""),
            ("fav", "0"),
            ("sortby", "number"),
            ("hd", "0"),
            ("p", &p),
        ])
    }

    /// Every channel in one response (Ministra); fall back to pages.
    pub fn all_channels(&self) -> Result<HttpRequest, StalkerError> {
        self.api_request(&[("type", "itv"), ("action", "get_all_channels")])
    }

    pub fn vod_categories(&self) -> Result<HttpRequest, StalkerError> {
        self.api_request(&[("type", "vod"), ("action", "get_categories")])
    }

    pub fn series_categories(&self) -> Result<HttpRequest, StalkerError> {
        self.api_request(&[("type", "series"), ("action", "get_categories")])
    }

    /// One page (1-based) of a VOD category.
    pub fn vod_page(&self, category: &str, page: u32) -> Result<HttpRequest, StalkerError> {
        let p = page.max(1).to_string();
        self.api_request(&[
            ("type", "vod"),
            ("action", "get_ordered_list"),
            ("category", category),
            ("genre", "*"),
            ("sortby", "added"),
            ("p", &p),
        ])
    }

    /// Series catalogue or children, preserving opaque movie/season coordinates.
    pub fn series_page(
        &self,
        movie: &str,
        season: Option<&str>,
        page: u32,
        native: bool,
    ) -> Result<HttpRequest, StalkerError> {
        let page = page.max(1).to_string();
        let mut params = vec![
            ("type", if native { "series" } else { "vod" }),
            ("action", "get_ordered_list"),
            ("p", page.as_str()),
            ("sortby", "added"),
        ];
        if movie.is_empty() {
            params.push(("category", "*"));
        } else {
            params.push(("movie_id", movie));
        }
        if let Some(season) = season {
            params.push(("season_id", season));
        }
        self.api_request(&params)
    }

    /// Short EPG for a channel id.
    pub fn short_epg(&self, channel_id: &str, size: u32) -> Result<HttpRequest, StalkerError> {
        let size = size.to_string();
        self.api_request(&[
            ("type", "itv"),
            ("action", "get_short_epg"),
            ("ch_id", channel_id),
            ("size", &size),
        ])
    }

    /// `create_link` for a live (`vod = false`) or VOD command. `series`
    /// selects an episode server-side (VOD only).
    pub fn create_link(
        &self,
        cmd: &str,
        vod: bool,
        series: Option<u32>,
    ) -> Result<HttpRequest, StalkerError> {
        let kind = if vod || series.is_some() {
            "vod"
        } else {
            "itv"
        };
        let series = series.map(|s| s.to_string());
        let mut params = vec![
            ("type", kind),
            ("action", "create_link"),
            ("cmd", cmd),
            ("disable_ad", "0"),
            ("download", "0"),
        ];
        if let Some(s) = &series {
            params.push(("series", s));
        }
        self.api_request(&params)
    }

    /// Keep-alive ping; send `init = true` once after authorizing, then every
    /// [`StalkerSession::watchdog_interval_secs`]. Failures are not fatal.
    pub fn watchdog(&self, init: bool) -> Result<HttpRequest, StalkerError> {
        self.api_request(&[
            ("type", "watchdog"),
            ("action", "get_events"),
            ("init", if init { "1" } else { "0" }),
            ("cur_play_type", "0"),
            ("event_active_id", "0"),
        ])
    }

    /// Headers for playing `stream_url`. Portal-owned streams (same host, no
    /// https→http downgrade) get the identity cookie, MAG User-Agent, Bearer
    /// token and serial; any other host gets only the User-Agent, so portal
    /// credentials never reach a third party. No `Range` header.
    pub fn playback_headers(&self, stream_url: &str) -> Vec<(String, String)> {
        let mut headers = vec![("User-Agent".to_owned(), super::MAG_USER_AGENT.to_owned())];
        if super::parse::is_portal_owned(stream_url, &self.portal) {
            let api = self.portal.request(&[], self.token.as_ref());
            for name in ["Cookie", "X-User-Agent", "Authorization", "SN"] {
                if let Some(v) = api.header(name) {
                    headers.push((name.to_owned(), v.to_owned()));
                }
            }
            headers.push(("Referer".to_owned(), format!("{}/", self.portal.base_url())));
        }
        headers
    }

    /// Whether a stream the portal named (a `create_link` result) has to be
    /// on a public address.
    ///
    /// It does, with one exception: the portal the user entered is itself
    /// on a private address or local name (a LAN installation) and the
    /// stream is on that same host. Such a stream is as trusted as the
    /// portal, which the user chose and which already receives the session's
    /// requests there. A portal on a public address never unlocks a private
    /// destination, and a private portal does not unlock another host.
    pub fn stream_must_be_public(&self, stream_url: &str) -> bool {
        let private_portal = crate::content_url::validate(self.portal.endpoint()).is_err();
        !(private_portal && super::parse::is_portal_owned(stream_url, &self.portal))
    }

    /// The URL to play now, or the `create_link` request to send first.
    pub fn resolve_stream(
        &self,
        locator: &StreamLocator,
        vod: bool,
    ) -> Result<Resolve, StalkerError> {
        match locator {
            StreamLocator::Url { url, .. } => Ok(Resolve::Play(url.clone())),
            StreamLocator::Stalker { cmd, link } => match super::static_playback_url(cmd, *link) {
                Some(url) => Ok(Resolve::Play(url)),
                None => self.create_link(cmd, vod, None).map(Resolve::CreateLink),
            },
            StreamLocator::Xtream { .. } => Err(StalkerError::with_text(
                StalkerErrorKind::Portal,
                Some("not a Stalker stream".into()),
            )),
        }
    }
}

/// Result of [`StalkerSession::resolve_stream`].
#[derive(Debug)]
pub enum Resolve {
    Play(String),
    /// Send, then pass the response to [`super::parse_create_link`].
    CreateLink(HttpRequest),
}
