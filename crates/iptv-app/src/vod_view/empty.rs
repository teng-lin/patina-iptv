//! The empty Movies or Shows catalogue: why it is empty and its one action.
//! The state is the core's ([`VodState`]), worked out from every enabled
//! playlist, the ones the catalogue reads; the actions are the ones the playlists screen and the wizard
//! already run (refresh a playlist, open its actions, add a playlist).
use super::*;
use crate::data::Activity;
use crate::runtime::model::{ImportErrorKind, SourceId};
use iptv_core::vod::{EmptyAction, ImportProblem, SourceVod, VodState};

fn problem(kind: ImportErrorKind) -> ImportProblem {
    match kind {
        ImportErrorKind::Network | ImportErrorKind::Timeout => ImportProblem::Network,
        ImportErrorKind::HttpStatus => ImportProblem::Refused,
        ImportErrorKind::AccountRejected
        | ImportErrorKind::AuthFailed
        | ImportErrorKind::Blocked => ImportProblem::Access,
        ImportErrorKind::InvalidBody => ImportProblem::Format,
        ImportErrorKind::Cancelled => ImportProblem::Stopped,
        _ => ImportProblem::Other,
    }
}

/// Every enabled playlist, the ones the catalogue reads whichever is on
/// screen, as the core reads them for `kind`. Tracks what they depend on.
fn sources(ui: Ui, kind: Kind) -> Vec<SourceVod> {
    ui.app.library.version.get();
    let settings = crate::settings::read(ui);
    let playlists = crate::runtime().playlists().unwrap_or_default();
    ui.app.library.activity.with(|activity| {
        playlists
            .iter()
            .filter(|p| p.policy.enabled)
            .map(|p| {
                let target = iptv_settings::Target::playlist(p.id.0.to_string());
                let session = activity.get(&p.id);
                // A whole import that failed, else the titles of this kind
                // that the last successful one could not download.
                let (has_titles, list_failed) = if kind == Kind::Series {
                    (p.has_series, p.series_failed)
                } else {
                    (p.has_movies, p.movies_failed)
                };
                let stored = p.failed.map(|(kind, _)| kind).or(list_failed).map(problem);
                SourceVod {
                    source: p.id.0,
                    eligible: p.kind != iptv_providers::model::SourceKind::M3u,
                    imports_enabled: settings
                        .bool(iptv_settings::keys::PLAYLISTS_INCLUDE_VOD, &target),
                    importing: matches!(
                        session,
                        Some(Activity::Running(_) | Activity::Checking(_))
                    ),
                    // The store's kind is the precise one: the runtime reads
                    // it again after each failure, and a success clears it.
                    // A failure it did not record is this session's.
                    failed: stored.or(match session {
                        Some(Activity::Done(Err(f))) => Some(ImportProblem::from(f.kind)),
                        _ => None,
                    }),
                    has_titles,
                }
            })
            .collect()
    })
}

/// Keeps the catalogue's [`VodState`] current. When it changes while no
/// titles are shown (an import started or finished, an import was switched
/// on), the catalogue is read again: titles may have arrived. The first
/// state only joins the read [`open`] started.
pub(super) fn bind(ui: Ui) {
    let kind = computed(move || ui.vod.with(|s| s.query.kind));
    let state = computed(move || VodState::of(&sources(ui, kind.get())));
    let first = std::cell::Cell::new(true);
    effect(move || {
        let state = state.get();
        let Some(untitled) = ui.vod.with_untracked(|s| {
            (s.library != state).then(|| s.cards.is_empty() && s.details.is_none())
        }) else {
            first.set(false);
            return;
        };
        ui.vod.update(|s| s.set_library(state));
        if untitled && !first.replace(false) {
            reload(ui, false);
        }
    });
}

fn reason(problem: ImportProblem) -> &'static str {
    match problem {
        ImportProblem::Network => tr("no answer from the server"),
        ImportProblem::Refused => tr("the server turned the request down"),
        ImportProblem::Busy => tr("too many playlists are updating; try again shortly"),
        ImportProblem::Access => tr("access was refused"),
        ImportProblem::Format => tr("the answer was not a playlist"),
        ImportProblem::Stopped => tr("it was stopped"),
        ImportProblem::Other => tr("something went wrong"),
    }
}

fn message(state: Option<VodState>, kind: Kind) -> String {
    let Some(state) = state else {
        return tr("No titles found.").to_owned();
    };
    match state {
        VodState::Available => tr("No titles found.").to_owned(),
        VodState::NoEligibleSource => tr(
            "Movies and shows come from Xtream Codes and Stalker playlists. M3U playlists have live channels only.",
        )
        .to_owned(),
        VodState::ImportsDisabled { .. } => {
            tr("Movie and series imports are off for this playlist.").to_owned()
        }
        VodState::Importing => tr("Importing movies and shows…").to_owned(),
        VodState::Failed { problem, .. } => trf(
            "Last refresh failed: {reason}",
            &[("reason", reason(problem).into())],
        ),
        VodState::Empty { .. } if kind == Kind::Series => {
            tr("No shows found. Refresh the playlist to look for new ones.").to_owned()
        }
        VodState::Empty { .. } => {
            tr("No movies found. Refresh the playlist to look for new ones.").to_owned()
        }
    }
}

fn label(action: EmptyAction) -> &'static str {
    match action {
        EmptyAction::AddPlaylist => tr("Add playlist"),
        EmptyAction::PlaylistSettings(_) => tr("Playlist settings"),
        EmptyAction::Retry(_) => tr("Retry"),
        EmptyAction::Refresh(_) => tr("Refresh now"),
    }
}

/// Runs the empty screen's action (OK where the titles would be, a tap
/// or a click). Never switches an import on by itself.
pub(super) fn run(ui: Ui) {
    let Some(action) = ui.vod.with_untracked(|s| s.empty_action()) else {
        return;
    };
    match action {
        EmptyAction::AddPlaylist => {
            close(ui);
            crate::wizard_view::open_add(ui);
        }
        EmptyAction::PlaylistSettings(id) => {
            close(ui);
            crate::playlists_view::open_on_demand(ui, SourceId(id));
        }
        EmptyAction::Retry(id) | EmptyAction::Refresh(id) => {
            crate::data::refresh(&crate::runtime(), SourceId(id));
        }
    }
}

pub(super) fn screen(ui: Ui, size: Scale) -> impl IntoElement {
    when(
        move || {
            ui.vod
                .with(|s| s.cards.is_empty() && !s.busy && s.error.is_none())
        },
        move || {
            let text = move || ui.vod.with(|s| message(s.empty_state(), s.query.kind));
            let action = move || ui.vod.with(|s| s.empty_action());
            view()
                .gap(move || size.px(10.0))
                .items_start()
                .test_id("vod-empty")
                .child(ui_kit::body(size, text))
                .child(when(
                    move || action().is_some(),
                    move || {
                        button(
                            ui,
                            size,
                            Control::Catalogue,
                            move || action().map_or("", label),
                            move || run(ui),
                        )
                        .test_id("vod-empty-action")
                    },
                ))
        },
    )
}
