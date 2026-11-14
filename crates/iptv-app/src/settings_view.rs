//! The settings screen (Long OK menu → Settings): the pages on the left
//! ([`PAGES`], in the reference's order), the chosen page's rows with
//! their current values on the right, the focused row's description below,
//! and the editors and dialogs (`editors`). State and rules live in
//! `iptv_settings::screen`; this draws it, maps remote keys and taps to
//! it, stores what it answers through `crate::settings` (saved at once,
//! applied where the app reads it) and runs its commands.
//!
//! Rows the app does not act on yet are dimmed and say so ("later" for
//! recordings, reminders, movies and series, parental control); none of
//! the rows it acts on can be switched off by another setting yet;
//! device-level features of TV boxes are left out (`settings::offer`). Remote: Up/Down move, Right/OK
//! enter a page, OK changes a row, Left/Back go back, Back on the pages
//! closes. Touch and mouse: tap a page, tap a row. Wording is iptv-rs's
//! own (`words`). Also the short notices of the app (`notify`), such as
//! the second Back that leaves (`may_leave`).

use crate::i18n::{tr, trf};

mod editors;
pub(crate) mod remote_keys;
mod view;
mod words;

pub use view::layer;

use std::cell::Cell;
use std::time::{Duration, Instant};

use iptv_nav::Action;
use iptv_settings::schema::ValueType;
use iptv_settings::screen::{Ctx, Editor, Effect, Key, Page, Pane, Part, Row, Screen, Status};
use iptv_settings::{Section, Settings, Target, Value, keys};
use nsh_core::prelude::*;

use crate::data::stored::{self, GuideStatus};
use crate::theme::{RADIUS, TEXT_DISABLED_ALPHA};
use crate::{Ui, tint};

const WHITE: u32 = 0xFFFFFF;
/// Titles of rows the app does not act on.
const DIM_ALPHA: u8 = 0x80;
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
    // The playlists screen (another workstream) is the whole page.
    Page {
        key: "playlists",
        parts: &[Part::Keys(&["playlists.manage"])],
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
        parts: &[Part::Keys(&["general.about"])],
    },
];

thread_local! {
    /// The page shown last: the screen opens on it again.
    static LAST: Cell<&'static str> = const { Cell::new("general") };
    /// The notice line's text (made with the layer, under the app's owner).
    static NOTICE: Cell<Option<Signal<Option<String>>>> = const { Cell::new(None) };
    /// When Back last asked whether to leave.
    static LEAVE_ASKED: Cell<Option<Instant>> = const { Cell::new(None) };
}

// --- State ---------------------------------------------------------------------

pub fn open(ui: Ui) {
    if !is_open(ui) {
        ui.settings_screen
            .set(Some(Screen::new(PAGES, LAST.with(Cell::get))));
    }
}

pub fn close(ui: Ui) {
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
            if let Err(e) = crate::settings::set(ui, key, value) {
                log::warn!("iptv settings: {e}");
            }
        }
        Effect::Run(key) => run(ui, key),
        Effect::Close => close(ui),
    }
}

/// Tells the rows to read their status again.
pub(crate) fn refresh(ui: Ui) {
    ui.settings.update(|v| *v += 1);
}

/// Runs a command (`settings::COMMANDS`).
fn run(ui: Ui, key: &'static str) {
    match key {
        "general.backup" => crate::backup::export_ui(),
        "general.restore" => crate::backup::restore_ui(ui),
        "epg.update_now" => {
            if stored::update_now(ui, move || refresh(ui)) {
                refresh(ui);
            } else {
                notify(tr("This playlist has no guide source switched on"));
            }
        }
        // The playlist on screen's guide sources, over the settings screen.
        "epg.manage_sources" | "epg.add_source" => match stored::shown() {
            Some(playlist) if key == "epg.add_source" => {
                crate::epg_sources_view::open_add(ui, playlist);
            }
            Some(playlist) => crate::epg_sources_view::open(ui, playlist),
            None if crate::data::combined::active(ui) => {
                crate::playlists_view::open(ui);
                notify(tr("Choose a playlist to manage its guide sources"));
            }
            None => notify(tr("No playlist on screen")),
        },
        "epg.clear" => {
            let cleared = stored::clear(ui, move |deleted| {
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
        let rows = iptv_settings::screen::rows(page, ctx);
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
        iptv_settings::screen::rows(page, ctx).iter().any(|r| {
            r.key().is_some() && matches!(ctx.status(r), Status::Active | Status::ReadOnly)
        })
    })
}

/// What a command row shows on its right.
fn command_value(key: &str) -> String {
    match key {
        "general.restore" if crate::backup::is_pending() => {
            tr("Scheduled for next launch").to_owned()
        }
        "epg.update_now" => match stored::guide_status() {
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
    let sources = stored::shown_sources();
    if sources.is_empty() {
        return tr("No enabled playlist").to_owned();
    }
    let lines: Vec<_> = sources.into_iter().flat_map(stored::source_lines).collect();
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
    notice.set(Some(text.to_owned()));
    let shown = text.to_owned();
    // The timer runs on its own; its handle is not needed.
    drop(set_timeout(NOTICE_FOR, move || {
        if notice.with_untracked(|n| n.as_deref() == Some(shown.as_str())) {
            notice.set(None);
        }
    }));
}

/// Whether a Back press may leave the app (`general.confirm_exit`): the
/// first press asks for a second one within `general.confirm_exit_window`.
pub fn may_leave() -> bool {
    let s = crate::settings::get();
    let at = &Target::Global;
    if !s.bool(keys::CONFIRM_EXIT, at) {
        return true;
    }
    let window = Duration::from_millis(
        u64::try_from(s.duration_ms(keys::CONFIRM_EXIT_WINDOW, at)).unwrap_or(0),
    );
    let now = Instant::now();
    let asked = LEAVE_ASKED.with(|a| a.replace(Some(now)));
    if second_press(asked, now, window) {
        LEAVE_ASKED.with(|a| a.set(None));
        return true;
    }
    notify(tr("Press Back again to leave"));
    false
}

fn second_press(asked: Option<Instant>, now: Instant, window: Duration) -> bool {
    asked.is_some_and(|t| now.duration_since(t) <= window)
}

#[cfg(test)]
mod tests {
    use iptv_settings::catalogue;

    use super::*;

    fn row(page: &str, key: &str, settings: &Settings) -> Row {
        let page = PAGES.iter().find(|p| p.key == page).unwrap();
        iptv_settings::screen::rows(
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
        assert_eq!(note(&s, &pin), "In a later version");
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
            if section != Section::Playlists {
                assert!(placed(section), "{section:?} has no page");
            }
        }
        for key in ["playlists.manage", "general.about"] {
            assert!(catalogue::command(key).is_some(), "{key}");
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
            let s = catalogue::setting(key).unwrap();
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
        // Panel timeouts, channel-change info, seek keys and three steps.
        assert_eq!(active, 7);
        let parental = PAGES.iter().find(|p| p.key == "parental").unwrap();
        assert_eq!(counts(parental).0, 0);
    }

    #[test]
    fn leaving_needs_a_second_back_in_time() {
        let t = Instant::now();
        let window = Duration::from_millis(2_000);
        assert!(!second_press(None, t, window));
        assert!(second_press(
            Some(t),
            t + Duration::from_millis(1_500),
            window
        ));
        assert!(!second_press(
            Some(t),
            t + Duration::from_millis(2_500),
            window
        ));
    }
}
