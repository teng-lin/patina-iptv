//! The empty Movies or Shows catalog: why it is empty and its one action.
//! The state is the core's ([`VodState`]), worked out from every enabled
//! playlist, the ones the catalog reads; the actions are the ones the playlists screen and the wizard
//! already run (refresh a playlist, open its actions, add a playlist).
use super::*;
use crate::runtime::model::{ImportErrorKind, SourceId};
use crate::wiring::data::Activity;
use iptv_presentation::vod::{EmptyAction, ImportProblem, SourceVod, VodState};

fn problem(kind: ImportErrorKind) -> ImportProblem {
    match kind {
        ImportErrorKind::Network | ImportErrorKind::Timeout => ImportProblem::Network,
        ImportErrorKind::HttpStatus => ImportProblem::Refused,
        ImportErrorKind::AccountRejected
        | ImportErrorKind::AuthFailed
        | ImportErrorKind::Blocked => ImportProblem::Access,
        ImportErrorKind::InvalidBody => ImportProblem::Format,
        ImportErrorKind::Canceled => ImportProblem::Stopped,
        _ => ImportProblem::Other,
    }
}

/// Every enabled playlist, the ones the catalog reads whichever is on
/// screen, as the core reads them for `kind`. Tracks what they depend on.
fn sources(app: crate::SharedUi, kind: Kind) -> Vec<SourceVod> {
    app.library.version.get();
    let settings = crate::wiring::settings_wiring::read(app);
    let playlists = crate::wiring::runtime_owner::runtime()
        .playlists()
        .unwrap_or_default();
    app.library.activity.with(|activity| {
        playlists
            .iter()
            .filter(|p| p.policy.enabled)
            .map(|p| {
                let target = iptv_settings::Target::playlist(p.id.0.to_string());
                let imports = settings.bool(iptv_settings::keys::PLAYLISTS_INCLUDE_VOD, &target);
                source_vod(p, kind, imports, activity.get(&p.id))
            })
            .collect()
    })
}

/// What playlist `p` says about its titles of `kind`, its movie and series
/// import `imports`, and this session's refresh of it.
fn source_vod(
    p: &crate::wiring::data::Playlist,
    kind: Kind,
    imports: bool,
    activity: Option<&Activity>,
) -> SourceVod {
    use iptv_providers::model::SourceKind;
    // A whole import that failed, else the titles of this kind that the
    // last successful one could not download. A Stalker movie list also
    // carries embedded series.
    let (has_titles, list_failed) = if kind == Kind::Series {
        let embedded = (p.kind == SourceKind::Stalker)
            .then_some(p.movies_failed)
            .flatten();
        (p.has_series, p.series_failed.or(embedded))
    } else {
        (p.has_movies, p.movies_failed)
    };
    let stored = p.failed.map(|(kind, _)| kind).or(list_failed);
    let session = match activity {
        Some(Activity::Done(Err(f))) => Some(ImportProblem::from(f.kind)),
        _ => None,
    };
    SourceVod {
        source: p.id.0,
        eligible: p.kind != SourceKind::M3u,
        imports_enabled: imports,
        importing: matches!(activity, Some(Activity::Running(_) | Activity::Checking(_))),
        // The store's kind is the precise one: the runtime reads it again
        // after each failure, and records one that never reached the store
        // (refused before it ran) as `Other`; this session's failure then
        // tells why.
        failed: match (stored, session) {
            (Some(ImportErrorKind::Other) | None, Some(now)) => Some(now),
            (stored, _) => stored.map(problem),
        },
        has_titles,
    }
}

/// Keeps the catalog's [`VodState`] current. When it changes while no
/// titles are shown (an import started or finished, an import was switched
/// on), the catalog is read again: titles may have arrived. The first
/// state only joins the read [`open`] started.
pub(super) fn bind(ui: Ui) {
    let kind = computed(move || ui.screens.vod.with(|s| s.query.kind));
    let state = computed(move || VodState::of(&sources(ui.app, kind.get())));
    let first = std::cell::Cell::new(true);
    effect(move || {
        let state = state.get();
        let Some(untitled) = ui.screens.vod.with_untracked(|s| {
            (s.library != state).then(|| s.cards.is_empty() && s.details.is_none())
        }) else {
            first.set(false);
            return;
        };
        ui.screens.vod.update(|s| s.set_library(state));
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
    let Some(action) = ui.screens.vod.with_untracked(|s| s.empty_action()) else {
        return;
    };
    match action {
        EmptyAction::AddPlaylist => {
            close(ui);
            crate::library_ui::wizard_view::open_add(ui);
        }
        EmptyAction::PlaylistSettings(id) => {
            close(ui);
            crate::library_ui::playlists_view::open_on_demand(ui, SourceId(id));
        }
        EmptyAction::Retry(id) | EmptyAction::Refresh(id) => {
            crate::wiring::data::refresh(&crate::wiring::runtime_owner::runtime(), SourceId(id));
        }
    }
}

pub(super) fn screen(ui: Ui, size: Scale) -> impl IntoElement {
    when(
        move || {
            ui.screens
                .vod
                .with(|s| s.cards.is_empty() && !s.busy && s.error.is_none())
        },
        move || {
            let text = move || {
                ui.screens
                    .vod
                    .with(|s| message(s.empty_state(), s.query.kind))
            };
            let action = move || ui.screens.vod.with(|s| s.empty_action());
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
                            Control::Catalog,
                            move || action().map_or("", label),
                            move || run(ui),
                        )
                        .test_id("vod-empty-action")
                    },
                ))
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use iptv_presentation::wizard::{FailureKind, ImportFailure};
    use iptv_providers::model::SourceKind;

    fn playlist(kind: SourceKind) -> crate::wiring::data::Playlist {
        crate::wiring::data::Playlist {
            id: SourceId(1),
            name: "Synthetic".into(),
            kind,
            channels: 1,
            has_movies: false,
            has_series: false,
            movies_failed: None,
            series_failed: None,
            updated_at: None,
            failed: None,
            guides: Vec::new(),
            guide_failed: false,
            user_agent: None,
            from_file: false,
            policy: Default::default(),
        }
    }

    fn failed(kind: FailureKind) -> Activity {
        Activity::Done(Err(ImportFailure::new(kind)))
    }

    #[test]
    fn a_stalker_movie_list_failure_also_leaves_shows_failed() {
        let mut p = playlist(SourceKind::Stalker);
        p.movies_failed = Some(ImportErrorKind::HttpStatus);
        let shows = source_vod(&p, Kind::Series, true, None);
        assert_eq!(shows.failed, Some(ImportProblem::Refused));
        // An Xtream movie list carries no series.
        p.kind = SourceKind::Xtream;
        assert_eq!(source_vod(&p, Kind::Series, true, None).failed, None);
        assert_eq!(
            source_vod(&p, Kind::Movie, true, None).failed,
            Some(ImportProblem::Refused)
        );
    }

    #[test]
    fn the_precise_stored_kind_wins_unless_the_attempt_never_reached_the_store() {
        let mut p = playlist(SourceKind::Xtream);
        p.failed = Some((ImportErrorKind::HttpStatus, None));
        // The same attempt, told coarsely by the session: the store's kind.
        let same = failed(FailureKind::Network);
        assert_eq!(
            source_vod(&p, Kind::Movie, true, Some(&same)).failed,
            Some(ImportProblem::Refused)
        );
        // A later attempt refused before it ran, recorded as `Other`.
        p.failed = Some((ImportErrorKind::Other, None));
        let busy = failed(FailureKind::Busy);
        assert_eq!(
            source_vod(&p, Kind::Movie, true, Some(&busy)).failed,
            Some(ImportProblem::Busy)
        );
        // Nothing stored yet: the session's failure.
        p.failed = None;
        assert_eq!(
            source_vod(&p, Kind::Movie, true, Some(&busy)).failed,
            Some(ImportProblem::Busy)
        );
        assert!(source_vod(&p, Kind::Movie, true, Some(&Activity::Running(None))).importing);
    }
}
