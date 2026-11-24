//! The settings screen (Long OK menu → Settings): the pages on the left
//! ([`PAGES`], in the reference's order), the chosen page's rows with
//! their current values on the right, the focused row's description below,
//! and the editors and dialogs (`editors`). State and rules live in
//! `iptv_presentation::settings::screen`; this draws it, maps remote keys and taps to
//! it, stores what it answers through `crate::settings` (saved at once,
//! applied where the app reads it) and runs its commands.
//!
//! Globally unimplemented settings are hidden consistently on every build; none of
//! the rows it acts on can be switched off by another setting yet;
//! device-level features of TV boxes are left out (`settings::offer`). Remote: Up/Down move, Right/OK
//! enter a page, OK changes a row, Left/Back go back, Back on the pages
//! closes. Touch and mouse: tap a page, tap a row. Wording is iptv-rs's
//! own (`words`). Also the short notices of the app (`notify`).

use crate::i18n::{tr, trf};
use crate::input::dismiss_keyboard;

mod editors;
pub(crate) mod remote_keys;
mod view;
mod words;

pub use view::layer;
pub(crate) use view::notice_layer;

use std::cell::Cell;
use std::time::Duration;

use iptv_nav::Action;
use iptv_presentation::settings::screen::{
    Ctx, Editor, Effect, Key, Page, Pane, Part, Row, Screen, Status,
};
use iptv_settings::schema::ValueType;
use iptv_settings::{Section, Settings, Value};
use nsh_core::prelude::*;

use crate::data::stored::{self, GuideStatus};
use crate::theme::{RADIUS, TEXT_DISABLED_ALPHA};
use crate::{Ui, tint};

const WHITE: u32 = 0xFFFFFF;
/// Titles of rows the app does not act on.
const DIM_ALPHA: u8 = 0x80;
const PRIVACY_POLICY_URL: &str = "https://teng-lin.github.io/patina-iptv/privacy-policy/";
const NOTICE_FOR: Duration = Duration::from_millis(2_500);

/// The pages, in the order the reference shows its settings.
pub static PAGES: &[Page] = &[
    Page {
        key: "general",
        parts: &[
            Part::Section(Section::General),
            Part::Section(Section::History),
        ],
    },
    // The playlists screen, which holds each playlist's own settings,
    // and the User-Agent its requests send unless the playlist names one.
    Page {
        key: "playlists",
        parts: &[Part::Keys(&["playlists.manage", "general.user_agent"])],
    },
    Page {
        key: "epg",
        parts: &[Part::Section(Section::Epg)],
    },
    Page {
        key: "appearance",
        parts: &[
            Part::Section(Section::Appearance),
            Part::Section(Section::Guide),
            Part::Section(Section::Channels),
            Part::Section(Section::Logos),
        ],
    },
    Page {
        key: "playback",
        parts: &[
            Part::Section(Section::Player),
            Part::Section(Section::Playback),
            Part::Section(Section::Catchup),
        ],
    },
    Page {
        key: "remote",
        parts: &[Part::Section(Section::Remote)],
    },
    Page {
        key: "parental",
        parts: &[Part::Section(Section::Parental)],
    },
    Page {
        key: "other",
        parts: &[
            Part::Section(Section::Search),
            Part::Section(Section::Recordings),
            Part::Section(Section::Vod),
        ],
    },
    Page {
        key: "about",
        parts: &[Part::Keys(&["general.about", "about.privacy_policy"])],
    },
];

thread_local! {
    /// The page shown last: the screen opens on it again.
    static LAST: Cell<&'static str> = const { Cell::new("general") };
    /// The notice line of the layer made last (made with it, under its owner).
    static NOTICE: Cell<Option<Notice>> = const { Cell::new(None) };
}

/// The settings layer's notice line: its text, how many notices it has
/// shown, and the layer's owner, which its timers run under.
#[derive(Clone, Copy)]
pub(crate) struct Notice {
    pub(crate) text: Signal<Option<String>>,
    shown: Signal<u64>,
    owner: Owner,
}

impl Notice {
    /// Made with the layer, under its owner.
    pub(crate) fn new() -> Self {
        Self {
            text: signal(None),
            shown: signal(0),
            owner: Owner::current(),
        }
    }
}

// --- State ---------------------------------------------------------------------

pub fn open(ui: Ui) {
    if crate::settings::choice(iptv_settings::keys::PARENTAL_REQUIRE_PIN_FOR)
        == "blocked_content_and_settings"
        && !crate::runtime().parental_settings_allowed()
    {
        crate::parental_view::request(ui, iptv_core::parental::Target::settings(), move || {
            open(ui)
        });
        return;
    }
    if !is_open(ui) {
        // A document picker may have been installed or removed since the
        // host last said which resolve (the backup rows follow).
        crate::backup::probe_pickers();
        ui.settings_screen
            .set(Some(Screen::new(PAGES, LAST.with(Cell::get))));
    }
}

pub fn close(ui: Ui) {
    crate::runtime().parental_content_closed();
    if let Some(page) = ui
        .settings_screen
        .with_untracked(|s| s.as_ref().map(|s| s.page().key))
    {
        LAST.with(|l| l.set(page));
    }
    let typing = editing_text(ui);
    ui.settings_screen.set(None);
    if typing {
        dismiss_keyboard();
    }
}

/// The screen is shown and takes the remote.
pub fn is_open(ui: Ui) -> bool {
    ui.settings_screen.with(Option::is_some)
}

fn editing_text(ui: Ui) -> bool {
    ui.settings_screen.with_untracked(|s| {
        s.as_ref()
            .is_some_and(|s| matches!(s.editor(), Some(Editor::Text { .. })))
    })
}

fn screen_key(action: Action) -> Option<Key> {
    Some(match action {
        Action::MoveUp => Key::Up,
        Action::MoveDown => Key::Down,
        Action::MoveLeft => Key::Left,
        Action::MoveRight => Key::Right,
        Action::Select => Key::Ok,
        Action::Back => Key::Back,
        Action::PageUp | Action::ChannelUp => Key::PageUp,
        Action::PageDown | Action::ChannelDown => Key::PageDown,
        _ => return None,
    })
}

/// A remote action while the screen is shown. While text is typed, the
/// field has the keys; OK and Back arrive once it let them go.
pub fn key(ui: Ui, action: Action) {
    if let Some(key) = screen_key(action) {
        update(ui, |screen, ctx| screen.key(key, ctx));
    }
}

/// Back: closes an editor, leaves the rows, then closes the screen.
pub fn back(ui: Ui) -> bool {
    key(ui, Action::Back);
    true
}

fn with_ctx<R>(f: impl FnOnce(&Ctx) -> R) -> R {
    let settings = crate::settings::get();
    f(&Ctx {
        settings: &settings,
        offer: &crate::settings::offer,
    })
}

/// Changes the screen with what the app knows now, then carries out what
/// it answers.
fn update(ui: Ui, f: impl FnOnce(&mut Screen, &Ctx) -> Effect) {
    let typing = editing_text(ui);
    let mut effect = Effect::None;
    with_ctx(|ctx| {
        ui.settings_screen.update(|s| {
            if let Some(s) = s {
                effect = f(s, ctx);
            }
        });
    });
    if typing && !editing_text(ui) {
        dismiss_keyboard();
    }
    match effect {
        Effect::None => {}
        Effect::Set { key, value } => {
            if (key.starts_with("parental.")
                || crate::settings::choice(iptv_settings::keys::PARENTAL_REQUIRE_PIN_FOR)
                    == "blocked_content_and_settings")
                && (!crate::runtime().parental_has_pin()
                    || !crate::runtime().parental_settings_allowed())
            {
                crate::parental_view::protected_settings(ui, move || {
                    if let Err(e) = crate::settings::set(ui, key, value) {
                        notify_setting_error(e);
                    }
                });
            } else if let Err(e) = crate::settings::set(ui, key, value) {
                notify_setting_error(e);
            }
        }
        Effect::Run(key) => run(ui, key),
        Effect::Close => close(ui),
    }
}

pub(crate) fn notify_setting_error(error: crate::settings::SaveError) {
    use crate::runtime::settings::SaveFailure;
    match error {
        crate::settings::SaveError::ParentalLocked => {
            notify(tr("Unlock parental controls before changing settings"))
        }
        crate::settings::SaveError::Storage(_) => {
            notify(words::save_failure(SaveFailure::NotPublished))
        }
        crate::settings::SaveError::Durability(_) => {
            notify(words::save_failure(SaveFailure::DurabilityUnconfirmed))
        }
        crate::settings::SaveError::Invalid(_) => notify(&error.to_string()),
    }
}

/// Tells the rows to read their status again.
pub(crate) fn refresh(ui: Ui) {
    ui.app.settings.update(|v| *v += 1);
}

/// Runs a command (`settings::COMMANDS`).
fn run(ui: Ui, key: &'static str) {
    if key != "parental.change_pin"
        && crate::settings::choice(iptv_settings::keys::PARENTAL_REQUIRE_PIN_FOR)
            == "blocked_content_and_settings"
        && !crate::runtime().parental_settings_allowed()
    {
        crate::parental_view::protected_settings(ui, move || run(ui, key));
        return;
    }
    match key {
        "history.clear" => {
            crate::player::watch_time::reset(ui);
            let owner = crate::runtime();
            let saved = owner.clear_watch_history();
            ui.player
                .history
                .set(iptv_presentation::playback::ChannelHistory::default());
            crate::data::spawn_in_app(async move {
                if saved.await {
                    crate::lists::favorites_changed(ui);
                    owner.refresh_watch_order();
                } else {
                    notify(tr("Recent channel changes could not be saved; try again"));
                }
            });
        }
        "search.clear_history" => crate::search_view::clear_history(ui),
        "parental.change_pin" => crate::parental_view::change_pin(ui),
        "recordings.quit_host" => crate::desktop::quit(ui),
        "recordings.background_status" => crate::host::recording_command(key),
        "about.privacy_policy" => {
            let opened = open_url(PRIVACY_POLICY_URL);
            crate::data::spawn_in_app(async move {
                if !opened.await {
                    notify(PRIVACY_POLICY_URL);
                }
            });
        }
        "general.backup" => crate::parental_view::request(
            ui,
            iptv_core::parental::Target::settings(),
            crate::backup::export_ui,
        ),
        "general.restore" => crate::backup::request_restore(ui),
        "epg.update_now" => {
            if stored::update_now(&crate::runtime(), move || refresh(ui)) {
                refresh(ui);
            } else {
                notify(tr("This playlist has no guide source switched on"));
            }
        }
        // The playlist on screen's guide sources, over the settings screen.
        "epg.manage_sources" | "epg.add_source" => match crate::runtime().shown_source() {
            Some(playlist) if key == "epg.add_source" => {
                crate::epg_sources_view::open_add(ui, playlist);
            }
            Some(playlist) => crate::epg_sources_view::open(ui, playlist),
            None if crate::data::combined::active(&crate::runtime()) => {
                crate::playlists_view::open(ui);
                notify(tr("Choose a playlist to manage its guide sources"));
            }
            None => notify(tr("No playlist on screen")),
        },
        "epg.clear" => {
            let cleared = stored::clear(&crate::runtime(), move |deleted| {
                notify(if deleted {
                    tr("Guide data deleted")
                } else {
                    tr("The guide data could not be deleted")
                });
                refresh(ui);
            });
            if !cleared {
                notify(tr("No playlist on screen"));
            }
        }
        "remote.customize" => crate::remote_editor::open(ui),
        // The playlists screen, over the settings screen.
        "playlists.manage" => crate::playlists_view::open(ui),
        other => log::info!("iptv settings: {other} is not implemented yet"),
    }
}

fn state<R>(ui: Ui, f: impl FnOnce(&Screen) -> R) -> Option<R> {
    ui.settings_screen.with(|s| s.as_ref().map(f))
}

fn status(settings: &Settings, row: &Row) -> Status {
    Ctx {
        settings,
        offer: &crate::settings::offer,
    }
    .status(row)
}

/// Why a row does not do what it says, or what to expect from it.
fn note(settings: &Settings, row: &Row) -> String {
    let restart = match row {
        Row::Setting(s) => s.flags.requires_restart,
        Row::Command(c) => c.requires_restart,
        Row::Heading { .. } => false,
    };
    match (status(settings, row), row) {
        (Status::NotYet, _) => tr("Not available yet").to_owned(),
        (Status::Later, _) => tr("In a later version").to_owned(),
        (Status::Active, _) if restart => tr("Takes effect after a restart").to_owned(),
        _ => String::new(),
    }
}

/// How many rows of `page` the app acts on, of how many.
fn counts(page: &Page) -> (usize, usize) {
    with_ctx(|ctx| {
        let rows = iptv_presentation::settings::screen::rows(page, ctx);
        let keyed = rows.iter().filter(|r| r.key().is_some());
        let active = keyed
            .clone()
            .filter(|r| ctx.status(r) == Status::Active)
            .count();
        (active, keyed.count())
    })
}

/// Whether `page` has rows that show something (active or read-only).
fn lit_page(page: &Page) -> bool {
    with_ctx(|ctx| {
        iptv_presentation::settings::screen::rows(page, ctx)
            .iter()
            .any(|r| {
                r.key().is_some() && matches!(ctx.status(r), Status::Active | Status::ReadOnly)
            })
    })
}

/// What a command row shows on its right.
fn command_value(key: &str) -> String {
    match key {
        "general.restore" if crate::runtime().restore_is_pending() => {
            tr("Scheduled for next launch").to_owned()
        }
        "epg.update_now" => match crate::runtime().guide_status() {
            GuideStatus::NoSource => tr("No playlist").to_owned(),
            GuideStatus::Updating => tr("Updating…").to_owned(),
            GuideStatus::Empty => tr("Not downloaded").to_owned(),
            GuideStatus::Updated { at, channels } => trf(
                "{count, plural, one {Updated {when} · # channel} \
                 other {Updated {when} · # channels}}",
                &[
                    ("when", (&time_label(at)).into()),
                    ("count", channels.into()),
                ],
            ),
            GuideStatus::Failed { at, .. } => {
                trf("Failed {when}", &[("when", (&time_label(at)).into())])
            }
        },
        "epg.manage_sources" => sources_value(),
        "general.about" => format!("v{}({})", env!("CARGO_PKG_VERSION"), env!("IPTV_GIT_HASH")),
        _ => String::new(),
    }
}

/// The "Guide sources" row's value: how many, switched on, and failed at
/// their last download.
fn sources_value() -> String {
    let sources = crate::runtime().shown_sources();
    if sources.is_empty() {
        return tr("No enabled playlist").to_owned();
    }
    let lines: Vec<_> = sources
        .into_iter()
        .flat_map(|source| crate::runtime().source_lines(source))
        .collect();
    let on = lines.iter().filter(|l| l.source.enabled).count();
    let failed = lines
        .iter()
        .filter(|l| l.source.enabled && l.result.as_ref().is_some_and(|r| r.error.is_some()))
        .count();
    let mut value = match (lines.len(), on) {
        (0, _) => tr("None").to_owned(),
        (all, on) if on == all => trf(
            "{count, plural, one {# source} other {# sources}}",
            &[("count", all.into())],
        ),
        (all, on) => trf(
            "{on} of {all} on",
            &[("on", on.into()), ("all", all.into())],
        ),
    };
    if failed > 0 {
        let failed = trf("{count} failed", &[("count", failed.into())]);
        value = format!("{value} · {failed}");
    }
    value
}

/// A past time as the guide writes it (`14:05`, `Tue 10 · 14:05`).
fn time_label(unix: i64) -> String {
    crate::time::label(unix, crate::time::wall_unix())
}

/// The runtime reports publication semantics; the shell chooses the wording
/// in the current interface language, including changes made since the save.
pub(crate) fn save_failed(failure: crate::runtime::settings::SaveFailure) {
    notify(words::save_failure(failure));
}

/// Shows `text` briefly at the bottom of the screen.
pub fn notify(text: &str) {
    announce_for_accessibility(text);
    let Some(notice) = NOTICE.with(Cell::get) else {
        return;
    };
    // A layer already gone (its window closed) shows nothing more.
    if !notice.text.try_set(Some(text.to_owned())) {
        return;
    }
    // The notice is told apart by when it was shown, not by its text: an
    // earlier notice with the same words (blocking a channel, then its
    // group) must not cut this one short when its own time is up.
    notice.shown.update(|n| *n = n.wrapping_add(1));
    let shown = notice.shown.get_untracked();
    // Under the layer's owner, not whatever runs now (a row that closes, a
    // task): NSH cancels the timer with its owner, so it neither stops early
    // nor writes the line after the layer is gone. Its handle is not needed.
    notice.owner.run(|| {
        drop(set_timeout(NOTICE_FOR, move || {
            if notice.shown.get_untracked() == shown {
                notice.text.set(None);
            }
        }));
    });
}

#[cfg(test)]
mod tests {
    use iptv_settings::catalog;

    use super::*;

    fn row(page: &str, key: &str, settings: &Settings) -> Row {
        let page = PAGES.iter().find(|p| p.key == page).unwrap();
        iptv_presentation::settings::screen::rows(
            page,
            &Ctx {
                settings,
                offer: &crate::settings::offer,
            },
        )
        .into_iter()
        .find(|r| r.key() == Some(key))
        .unwrap()
    }

    #[test]
    fn rows_say_why_they_do_nothing() {
        let s = Settings::new();
        let language = row("general", "general.language", &s);
        assert_eq!(note(&s, &language), "");
        let resume = row("general", "general.resume_last_channel", &s);
        assert_eq!(note(&s, &resume), "");
        let pin = row("parental", "parental.live_enabled", &s);
        assert_eq!(note(&s, &pin), "");
    }

    #[test]
    fn every_setting_has_a_page_or_is_not_offered() {
        // Playlist settings live on the playlists screen.
        let placed = |section: Section| {
            PAGES
                .iter()
                .any(|p| p.parts.contains(&Part::Section(section)))
        };
        for section in Section::ALL {
            if section != Section::Playlists && !catalog::section(section).is_empty() {
                assert!(placed(section), "{section:?} has no page");
            }
        }
        let privacy = row("about", "about.privacy_policy", &Settings::new());
        assert_eq!(status(&Settings::new(), &privacy), Status::Active);
        for key in ["playlists.manage", "general.about"] {
            assert!(catalog::command(key).is_some(), "{key}");
        }
        for p in PAGES {
            assert!(!words::page(p.key).is_empty(), "{}", p.key);
        }
    }

    /// No row the app acts on can be switched off by another setting, so
    /// `note` has nothing to say for `Status::Disabled`; wiring such a
    /// setting needs a note that names the switch.
    #[test]
    fn no_wired_setting_is_switched_off_by_another() {
        use iptv_settings::schema::Effect as DependencyEffect;
        for key in crate::settings::WIRED {
            let s = catalog::setting(key).unwrap();
            assert!(
                s.depends
                    .iter()
                    .all(|d| d.effect != DependencyEffect::Enabled),
                "{key}"
            );
        }
    }

    #[test]
    fn remote_keys_map_to_the_screen() {
        assert_eq!(screen_key(Action::MoveLeft), Some(Key::Left));
        assert_eq!(screen_key(Action::Select), Some(Key::Ok));
        assert_eq!(screen_key(Action::ChannelDown), Some(Key::PageDown));
        assert_eq!(screen_key(Action::Digit(3)), None);
        assert_eq!(screen_key(Action::ShowMenu), None);
    }

    #[test]
    fn pages_count_what_they_offer() {
        let playback = PAGES.iter().find(|p| p.key == "playback").unwrap();
        let (active, _) = counts(playback);
        // Panel timeouts, channel-change info, multiview's sound following
        // the selected tile, seek keys, three steps and swiping to change
        // channel.
        assert_eq!(active, 9);
        let parental = PAGES.iter().find(|p| p.key == "parental").unwrap();
        // Only the two enable switches and PIN command are visible by default;
        // dependent preferences appear after live or VOD protection is enabled.
        assert_eq!(counts(parental), (3, 3));
    }
}
