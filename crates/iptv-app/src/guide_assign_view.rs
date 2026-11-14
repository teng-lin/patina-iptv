//! "Choose guide data…" (Long OK menu on a channel): which guide channel a
//! playlist channel shows, from the guide channels the stored guides list,
//! or tr("Automatic") (matched by id and name) or tr("No guide data"). A search
//! field narrows the list as one types; the filter shows every guide
//! channel or only those no channel uses. The choice is saved by channel
//! key (`data::assign`) and shows at once when the stored guide has the
//! programmes, else after the guide download it starts.
//!
//! Remote: Up/Down (Page Up/Down) move, OK chooses (on the field: type,
//! with the keyboard), Back ends typing, then closes. Touch and mouse:
//! tap a row, tap the field to type; a tap beside the panel closes it.
//! State: `state.rs`.

use iptv_library::guide_assign as state;

use crate::i18n::{tr, trf};
use crate::runtime::model::{GuideChannel, SourceId};
use iptv_nav::Action;
use nsh_core::element::input;
use nsh_core::prelude::*;

pub use state::Picker as Open;
use state::{Effect, Key, Picker, Row, Subject};

use crate::data::assign::{self, Applied};
use crate::list_rows::{self, Mark, ROW_TWO};
use crate::scale::Scale;
use crate::ui_kit;
use crate::{Ui, tint};

const PANEL_W: f32 = 680.0;
thread_local! { static REQUEST: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; }
fn next_request() -> u64 {
    REQUEST.with(|request| {
        let next = request.get().wrapping_add(1);
        request.set(next);
        next
    })
}

/// Opens the chooser for catalog channel `channel` of the playlist on
/// screen; a notice when there is none or it has no guide at all.
pub fn open(ui: Ui, channel: usize) {
    let request = next_request();
    let schedule = crate::schedule();
    let Some((playlist, key)) = schedule
        .channel_key(channel)
        .and_then(|k| crate::data::combined::owner(ui, &k))
    else {
        crate::settings_view::notify(tr("No playlist on screen"));
        return;
    };
    ui.guide_assign.set(Some(Picker::new(Subject {
        playlist,
        current: assign::Current::default(),
        name: schedule.channel_name(channel),
        key: key.clone(),
        channels: Vec::new(),
        feeds: Vec::new(),
    })));
    crate::data::spawn_in_app(async move {
        let mut channels = assign::guide_channels(playlist).await;
        if REQUEST.with(|current| current.get()) != request {
            return;
        }
        if channels.is_empty() {
            close(ui);
            crate::settings_view::notify(tr("No guide data is stored yet: update the guide first"));
            return;
        }
        let feeds: Vec<_> = crate::data::stored::source_lines(playlist)
            .iter()
            .map(|line| {
                (
                    line.source.feed,
                    crate::epg_sources_view::source_title(&line.source),
                )
            })
            .collect();
        // The sources in the user's order (the first one wins), each by name.
        channels.sort_by_key(|c| {
            feeds
                .iter()
                .position(|(f, _)| *f == c.feed)
                .unwrap_or(usize::MAX)
        });
        let subject = Subject {
            playlist,
            current: assign::current(playlist, &key).await,
            name: schedule.channel_name(channel),
            key,
            channels,
            feeds,
        };
        if REQUEST.with(|current| current.get()) == request && is_open(ui) {
            ui.guide_assign.set(Some(Picker::new(subject)));
        }
    });
}

pub fn close(ui: Ui) {
    next_request();
    if ui.guide_assign.with_untracked(Option::is_some) {
        ui.guide_assign.set(None);
        dismiss_keyboard();
    }
}

pub fn is_open(ui: Ui) -> bool {
    ui.guide_assign.with(Option::is_some)
}

fn state<R>(ui: Ui, f: impl FnOnce(&Picker) -> R) -> Option<R> {
    ui.guide_assign.with(|s| s.as_ref().map(f))
}

fn update(ui: Ui, f: impl FnOnce(&mut Picker) -> Option<Effect>) {
    let mut effect = None;
    ui.guide_assign.update(|s| {
        if let Some(s) = s {
            let subject = s.subject();
            let (playlist, key) = (subject.playlist, subject.key.clone());
            effect = f(s).map(|e| (playlist, key, e));
        }
    });
    if let Some((playlist, key, effect)) = effect {
        perform(ui, playlist, key, effect);
    }
}

/// A remote action while the chooser is open.
pub fn key(ui: Ui, action: Action) {
    let key = match action {
        Action::MoveUp => Key::Up,
        Action::MoveDown => Key::Down,
        Action::PageUp | Action::ChannelUp => Key::PageUp,
        Action::PageDown | Action::ChannelDown => Key::PageDown,
        Action::Select => Key::Ok,
        Action::Back | Action::ShowMenu => Key::Back,
        _ => return,
    };
    update(ui, |s| s.key(key));
}

/// The platform's Back: ends typing, then closes.
pub fn back(ui: Ui) -> bool {
    key(ui, Action::Back);
    true
}

/// Carries out `effect` of the chooser of channel `key` of `playlist`.
fn perform(ui: Ui, playlist: SourceId, key: String, effect: Effect) {
    match effect {
        Effect::Type => {}
        Effect::Close => close(ui),
        Effect::Choose(choice) => {
            log::info!("iptv guide choice: {}", choice_name(&choice));
            assign::choose(ui, playlist, key, choice, move |applied| {
                crate::settings_view::notify(applied_words(applied));
                close(ui);
            });
        }
    }
}

/// For the log: the kind of choice, no ids.
fn choice_name(choice: &assign::Choice) -> &'static str {
    match choice {
        assign::Choice::Automatic => "automatic",
        assign::Choice::NoGuide => "no guide",
        assign::Choice::Channel { .. } => "a guide channel",
    }
}

fn applied_words(applied: Applied) -> &'static str {
    match applied {
        Applied::Now => tr("Guide data changed"),
        Applied::Downloading => tr("Saved; the guide is downloading to show it"),
        Applied::Later => tr("Saved; it shows after the next guide update"),
        Applied::Failed => tr("The choice could not be saved"),
    }
}

// --- View ----------------------------------------------------------------------

pub fn layer(ui: Ui) -> impl IntoElement {
    when(move || is_open(ui), move || panel(ui))
}

fn panel(ui: Ui) -> impl IntoElement {
    let size = Scale::new();
    view()
        .absolute()
        .inset_0()
        .flex_row()
        // The guide stays visible, dimmed; a tap there closes.
        .child(view().flex_1().bg(tint(0x000000, 0x90)).on_press(move || {
            ui.touched();
            close(ui);
        }))
        .child(
            view()
                .w(move || size.px(PANEL_W))
                .max_w(percentage(0.92))
                .pt_safe_area()
                .px(move || size.px(20.0))
                .py(move || size.px(16.0))
                .gap(move || size.px(8.0))
                .bg(move || rgb(ui.theme.with(|t| t.background.panel)))
                .child(header(ui, size))
                .child(list(ui, size)),
        )
}

/// What the channel's guide is now.
fn now_line(p: &Picker) -> String {
    let s = p.subject();
    let bound = s.current.bound.as_ref().map(|(feed, id, kind)| {
        let name = s
            .channels
            .iter()
            .find(|c| c.feed == *feed && c.epg_channel == *id)
            .and_then(|c| c.name.clone())
            .unwrap_or_else(|| id.clone());
        let args = [
            ("name", (&name).into()),
            ("source", p.feed_name(*feed).into()),
        ];
        match kind {
            iptv_epg::MatchKind::Override => trf("Now: {name} ({source}, chosen by you)", &args),
            _ => trf("Now: {name} ({source}, matched automatically)", &args),
        }
    });
    match (&s.current.choice, bound) {
        (Some(assign::Choice::NoGuide), _) => tr("Now: no guide data (chosen by you)").to_owned(),
        (_, Some(bound)) => bound,
        (Some(assign::Choice::Channel { .. }), None) => {
            tr("Now: chosen by you; shows after the next guide update").to_owned()
        }
        _ => tr("Now: no guide channel matches it").to_owned(),
    }
}

fn header(ui: Ui, size: Scale) -> impl IntoElement {
    view()
        .gap(move || size.px(4.0))
        .child(ui_kit::title(size, move || {
            state(ui, |p| {
                trf(
                    "Guide data for {channel}",
                    &[("channel", (&p.subject().name).into())],
                )
            })
            .unwrap_or_default()
        }))
        .child(
            ui_kit::caption(size, move || state(ui, now_line).unwrap_or_default())
                .number_of_lines(1),
        )
        .child(ui_kit::caption(size, move || {
            let busy = state(ui, Picker::busy).unwrap_or(false);
            match (busy, ui.keys_shown()) {
                (true, _) => tr("Applying…"),
                (false, true) => tr("▲▼ move   OK choose   Back close"),
                (false, false) => tr("Tap the field to search, a row to choose it"),
            }
        }))
}

fn list(ui: Ui, size: Scale) -> impl IntoElement {
    let rows = element_ref();
    let count = move || state(ui, Picker::len).unwrap_or(0);
    let focused = computed(move || state(ui, Picker::focus_index));
    effect(move || {
        if let Some(i) = focused.get()
            && ui.keys_shown()
        {
            rows.scroll_to_index(i as u32, percentage(0.5), px(0.0), false);
        }
    });
    virtual_list(count, move |at| row(ui, size, at.index(), focused))
        .estimated_item_size(list_rows::pitch(size, ROW_TWO))
        .element_ref(&rows)
        .flex_1()
}

fn row(ui: Ui, size: Scale, index: usize, focused: Memo<Option<usize>>) -> AnyElement {
    if index == 0 {
        return search_field(ui, size, focused).into_element();
    }
    let item = move || state(ui, |p| p.row(index)).flatten();
    let channel = move || state(ui, |p| p.channel(index).cloned()).flatten();
    let current = move || state(ui, |p| item().is_some_and(|r| p.is_current(&r))).unwrap_or(false);
    let title = move || match item() {
        Some(Row::Filter) => {
            if state(ui, Picker::all).unwrap_or(true) {
                tr("Showing every guide channel").to_owned()
            } else {
                tr("Showing only guide channels no channel uses").to_owned()
            }
        }
        Some(Row::Automatic) => tr("Automatic").to_owned(),
        Some(Row::NoGuide) => tr("No guide data").to_owned(),
        Some(Row::Channel(..)) => channel()
            .map(|c| c.name.clone().unwrap_or_else(|| c.epg_channel.clone()))
            .unwrap_or_default(),
        _ => String::new(),
    };
    let detail = move || match item() {
        Some(Row::Filter) => state(ui, |p| {
            trf(
                "{count} shown · OK to switch",
                &[("count", p.matches().into())],
            )
        })
        .unwrap_or_default(),
        Some(Row::Automatic) => tr("Matched by its guide id and name").to_owned(),
        Some(Row::NoGuide) => tr("The guide shows no programmes for it").to_owned(),
        Some(Row::Channel(..)) => channel()
            .map(|c| channel_detail(ui, &c))
            .unwrap_or_default(),
        _ => String::new(),
    };
    let mark = move || Mark::of(focused.get() == Some(index) && ui.keys_shown(), false);
    list_rows::row(size, ROW_TWO, mark)
        .role(AccessibilityRole::Option)
        .accessibility_state(move || AccessibilityState {
            selected: current(),
            ..Default::default()
        })
        .flex_row()
        .items_center()
        .justify_start()
        .gap(move || size.px(12.0))
        .on_press(move || {
            ui.touched();
            update(ui, |p| p.tap(index));
        })
        .child(
            view()
                .flex_1()
                .child(ui_kit::label(size, title).text_color(move || mark().text()))
                .child(ui_kit::detail(size, detail).text_color(move || mark().secondary())),
        )
        .child(
            ui_kit::caption(size, move || if current() { tr("Current") } else { "" }).text_color(
                move || match mark() {
                    Mark::Focused => Mark::Focused.text(),
                    _ => rgb(ui.theme.with(|t| t.accent.light)),
                },
            ),
        )
        .into_element()
}

fn channel_detail(ui: Ui, c: &GuideChannel) -> String {
    let feed = state(ui, |p| p.feed_name(c.feed).to_owned()).unwrap_or_default();
    let args = [("id", (&c.epg_channel).into()), ("source", (&feed).into())];
    if c.assigned {
        trf("{id} · {source} · in use", &args)
    } else {
        format!("{} · {feed}", c.epg_channel)
    }
}

/// The first row: the search field. OK on it (or a tap) types; Return
/// goes to the first match.
fn search_field(ui: Ui, size: Scale, focused: Memo<Option<usize>>) -> impl IntoElement {
    let field = element_ref();
    let typing = computed(move || state(ui, Picker::typing).unwrap_or(false));
    effect(move || {
        if typing.get() {
            field.focus();
        } else {
            field.blur();
        }
    });
    view().h(move || size.px(ROW_TWO)).justify_center().child(
        ui_kit::field(size, input())
            .element_ref(&field)
            .value(move || state(ui, |p| p.query().to_owned()).unwrap_or_default())
            .placeholder(tr("Search guide channels by name or id"))
            .auto_capitalize(AutoCapitalize::None)
            .auto_correct(false)
            .return_key_type(ReturnKeyType::Search)
            .bg(move || ui_kit::field_fill(ui, focused.get() == Some(0)))
            .border(move || size.px(2.0))
            .px(move || size.px(12.0))
            .py(move || size.px(ui_kit::FIELD_PAD_Y - 2.0))
            .border_color(move || ui_kit::field_border(ui, focused.get() == Some(0)))
            .on_change(move |text: &str| {
                if state(ui, |p| p.query() != text).unwrap_or(false) {
                    update(ui, |p| {
                        p.set_query(text);
                        None
                    });
                }
            })
            .on_submit(move |_: &str| {
                update(ui, |p| {
                    p.submit();
                    None
                });
                dismiss_keyboard();
            })
            .on_focus(move || {
                if !state(ui, Picker::typing).unwrap_or(true) {
                    update(ui, |p| {
                        p.set_typing(true);
                        None
                    });
                }
            })
            .on_blur(move || {
                if state(ui, Picker::typing).unwrap_or(false) {
                    update(ui, |p| {
                        p.set_typing(false);
                        None
                    });
                }
            }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_names_no_channel_and_every_outcome_has_words() {
        let chosen = assign::Choice::Channel {
            feed: iptv_epg::FeedId(1),
            epg_channel: "private.id".into(),
        };
        assert_eq!(choice_name(&chosen), "a guide channel");
        assert_eq!(choice_name(&assign::Choice::Automatic), "automatic");
        assert_eq!(choice_name(&assign::Choice::NoGuide), "no guide");
        assert_eq!(applied_words(Applied::Now), "Guide data changed");
        assert_eq!(
            applied_words(Applied::Failed),
            "The choice could not be saved"
        );
    }
}
