//! The first-run / add-playlist wizard screen. `iptv_core::wizard` owns the
//! flow, focus and validation; this draws each step full screen, feeds it
//! remote keys and taps, and performs its effects (text entry, clipboard,
//! file picker, import through `data::start_import`).
//!
//! Remote: Up/Down move, OK activates (a field opens text entry), Menu
//! pastes into the focused field, Back goes back (on the first-run empty
//! state it leaves the app). Touch: every row and button is tappable.
//! Entry points: first run without a stored source, the top bar's "Add
//! playlist" (tap), and Menu in the groups pane (remote).

use crate::i18n::tr;

pub(crate) mod entry;
pub(crate) mod words;

use iptv_core::wizard::{
    Field, Notice, Origin, PlaylistKind, Step, Target, Wizard, WizardEffect, WizardEvent,
    WizardExit,
};
use iptv_nav::Action;
use nsh_core::picker::FilePickRequest;
use nsh_core::prelude::*;

use crate::data::{self, Job, Purpose, Update};
use crate::list_rows::{self, Mark, ROW, ROW_TWO};
use crate::scale::Scale;
use crate::theme::{LIST_TEXT, RADIUS, RESTING_ALPHA};
use crate::ui_kit::{self, ERROR};
use crate::{Ui, tint};

const WHITE: u32 = 0xFFFFFF;
const CARD_W: f32 = 720.0;
/// A form row's label column (dp at full size).
const LABEL_W: f32 = 150.0;

/// The open wizard and what the screen adds to it.
#[derive(Clone, PartialEq)]
pub struct Open {
    pub wizard: Wizard,
    /// Text entry, while open.
    editing: Option<entry::Editing>,
    /// The example URL went into the M3U form once.
    prefilled: bool,
}

fn open_with(ui: Ui, wizard: Wizard) {
    ui.wizard.set(Some(Open {
        wizard,
        editing: None,
        prefilled: false,
    }));
}

/// No playlists yet: the empty state.
pub fn open_first_run(ui: Ui) {
    open_with(ui, Wizard::first_run());
}

/// "Add playlist" from the guide: the type list.
pub fn open_add(ui: Ui) {
    if !is_open(ui) {
        open_with(ui, Wizard::add_playlist());
    }
}

pub fn is_open(ui: Ui) -> bool {
    ui.wizard.with(Option::is_some)
}

fn read<R>(ui: Ui, f: impl FnOnce(&Wizard) -> R) -> Option<R> {
    ui.wizard.with(|w| w.as_ref().map(|o| f(&o.wizard)))
}

/// Feeds `event` to the wizard and performs what it asks for.
pub fn send(ui: Ui, event: WizardEvent) {
    let mut effect = None;
    ui.wizard.update(|w| {
        if let Some(open) = w {
            effect = open.wizard.handle(event);
        }
    });
    if let Some(effect) = effect {
        perform(ui, effect);
    }
    prefill(ui);
}

/// The M3U link form starts with the development playlist's URL, when the
/// build has one ([`data::DEV_PLAYLIST`]).
fn prefill(ui: Ui) {
    let Some(url) = data::DEV_PLAYLIST.filter(|u| !u.is_empty()) else {
        return;
    };
    let due = read(ui, |w| {
        w.step() == Step::Form
            && w.kind() == PlaylistKind::M3uUrl
            && w.value(Field::PlaylistUrl).is_empty()
    })
    .unwrap_or(false)
        && ui.wizard.with(|w| w.as_ref().is_some_and(|o| !o.prefilled));
    if due {
        ui.wizard.update(|w| {
            if let Some(open) = w {
                open.prefilled = true;
                open.wizard.handle(WizardEvent::SetField {
                    field: Field::PlaylistUrl,
                    value: url.to_owned(),
                });
            }
        });
    }
}

fn perform(ui: Ui, effect: WizardEffect) {
    match effect {
        WizardEffect::EditField { field } => entry::begin(ui, field),
        WizardEffect::ReadClipboard { field } => data::spawn_in_app(async move {
            let text = clipboard_string().await;
            send(
                ui,
                WizardEvent::ClipboardText {
                    field,
                    text: Some(text),
                },
            );
        }),
        WizardEffect::PickFile => data::spawn_in_app(async move {
            let files = pick_files(FilePickRequest::new(false, 1)).await;
            let path = if let Some(file) = files.first() {
                data::keep_picked_file(&file.name, file.data.bytes())
                    .await
                    .inspect_err(|e| log::warn!("iptv wizard: keeping the file failed: {e}"))
                    .ok()
            } else {
                None
            };
            send(ui, WizardEvent::FilePicked(path));
        }),
        WizardEffect::StartImport { id, draft } => {
            let (source, epg_url) = data::source_from_draft(&draft);
            log::info!("iptv wizard: import {id:?} of {:?}", source.kind());
            let job = Job {
                source,
                epg_url,
                stored: None,
                purpose: Purpose::Add,
                edit: false,
            };
            let token = data::start_import(ui, job, move |update| {
                send(
                    ui,
                    match update {
                        Update::Progress(progress) => WizardEvent::Progress { id, progress },
                        Update::Done(Ok(counts)) => WizardEvent::Succeeded { id, counts },
                        Update::Done(Err(failure)) => WizardEvent::Failed { id, failure },
                    },
                )
            });
            crate::runtime()
                .wizard_import()
                .with(|r| *r.borrow_mut() = Some((id, token)));
        }
        WizardEffect::CancelImport { id } => crate::runtime().wizard_import().with(|r| {
            let mut running = r.borrow_mut();
            if running.as_ref().is_some_and(|(at, _)| *at == id)
                && let Some((_, token)) = running.take()
            {
                log::info!("iptv wizard: import {id:?} cancelled");
                token.cancel();
            }
        }),
        WizardEffect::Close(exit) => {
            log::info!("iptv wizard: closed ({exit:?})");
            if let WizardExit::Added { .. } = exit {
                crate::runtime()
                    .wizard_import()
                    .with(|r| r.borrow_mut().take());
                // Added from the playlists screen: "Watch" goes to the guide.
                crate::playlists_view::close(ui);
            }
            ui.wizard.set(None);
        }
    }
}

/// A remote action while the wizard is open.
pub fn key(ui: Ui, action: Action) {
    if entry::is_editing(ui) {
        // The input has the keys while focused; these come once it lost it.
        if matches!(action, Action::Select | Action::Back) {
            entry::commit(ui);
        }
        return;
    }
    match action {
        Action::MoveUp => send(ui, WizardEvent::Up),
        Action::MoveDown => send(ui, WizardEvent::Down),
        Action::Select => send(ui, WizardEvent::Ok),
        Action::ShowMenu => send(ui, WizardEvent::Paste),
        Action::Back => {
            back(ui);
        }
        _ => {}
    }
}

/// Back: ends text entry, else goes back a step. On the first-run empty
/// state it is not handled, so the app is left (there is nothing behind).
pub fn back(ui: Ui) -> bool {
    if entry::is_editing(ui) {
        entry::commit(ui);
        return true;
    }
    let root = read(ui, |w| {
        w.origin() == Origin::FirstRun && w.step() == Step::Empty
    });
    if root == Some(true) {
        return false;
    }
    send(ui, WizardEvent::Back);
    true
}

/// The top bar's entry point.
pub fn add_button(ui: Ui) -> impl IntoElement {
    let size = Scale::new();
    ui_kit::caption(size, tr("+ Add playlist"))
        .role(AccessibilityRole::Button)
        .px(move || size.px(12.0))
        .py(move || size.px(5.0))
        .rounded(px(RADIUS))
        .text_color(rgb(LIST_TEXT))
        .bg(tint(WHITE, RESTING_ALPHA * 2))
        .on_press(move || {
            ui.touched();
            open_add(ui);
        })
}

/// Which screen shows: rebuilt only when this changes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Screen {
    Empty,
    ChooseType,
    Form(PlaylistKind),
    Processing,
    Succeeded,
    Failed,
}

fn screen_of(w: &Wizard) -> Option<Screen> {
    Some(match w.step() {
        Step::Empty => Screen::Empty,
        Step::ChooseType { .. } => Screen::ChooseType,
        Step::Form => Screen::Form(w.kind()),
        Step::Processing { .. } => Screen::Processing,
        Step::Succeeded { .. } => Screen::Succeeded,
        Step::Failed { .. } => Screen::Failed,
        Step::Closed => return None,
    })
}

pub fn layer(ui: Ui) -> impl IntoElement {
    let screen = computed(move || {
        ui.wizard
            .with(|w| w.as_ref().and_then(|o| screen_of(&o.wizard)))
    });
    let editing = computed(move || entry::is_editing(ui));
    when(
        move || screen.get().is_some(),
        move || {
            view()
                .absolute()
                .inset_0()
                .pt_safe_area()
                .items_center()
                .justify_center()
                .bg(move || rgb(ui.theme.with(|t| t.background.window)))
                .child(dynamic(move || {
                    let shown = screen.get();
                    // Built untracked: only a change of screen rebuilds it.
                    untracked(move || match shown {
                        Some(Screen::Empty) => empty(ui).into_element(),
                        Some(Screen::ChooseType) => choose_type(ui).into_element(),
                        Some(Screen::Form(kind)) => form(ui, kind).into_element(),
                        Some(Screen::Processing) => processing(ui).into_element(),
                        Some(Screen::Succeeded) => succeeded(ui).into_element(),
                        Some(Screen::Failed) | None => failed(ui).into_element(),
                    })
                }))
                .child(when(move || editing.get(), move || entry::sheet(ui)))
        },
    )
}

fn focused(ui: Ui, target: Target) -> bool {
    ui.keys_shown() && read(ui, |w| w.focus() == Some(target)) == Some(true)
}

/// A focusable row's mark: the pill while focused with the keys shown.
fn mark(ui: Ui, target: Target) -> Mark {
    Mark::of(focused(ui, target), false)
}

fn card(ui: Ui, size: Scale) -> View {
    ui_kit::card(ui, size, CARD_W).gap(move || size.px(10.0))
}

/// Remote hints, shown only while keys drive the UI.
fn hint(ui: Ui, size: Scale, keys: &'static str) -> impl IntoElement {
    ui_kit::caption(size, move || if ui.keys_shown() { tr(keys) } else { "" })
        .mt(move || size.px(6.0))
}

/// A step that Back leaves: the keys' hint and, on pointer and touch
/// devices, a visible Back button.
fn hint_or_back(ui: Ui, size: Scale, keys: &'static str) -> impl IntoElement {
    view().child(hint(ui, size, keys)).child(when(
        move || ui.back_controls(),
        move || {
            ui_kit::button(
                ui,
                size,
                tr("‹ Back"),
                || false,
                move || send(ui, WizardEvent::Back),
            )
            .self_start()
            .mt(move || size.px(4.0))
        },
    ))
}

/// A focusable button for `target`; a tap sends `Tap(target)`.
fn button(ui: Ui, size: Scale, label: &'static str, target: Target) -> impl IntoElement {
    ui_kit::button(
        ui,
        size,
        move || tr(label),
        move || focused(ui, target),
        move || send(ui, WizardEvent::Tap(target)),
    )
    .self_start()
    .mt(move || size.px(8.0))
}

fn empty(ui: Ui) -> impl IntoElement {
    let size = Scale::new();
    card(ui, size)
        .child(ui_kit::title(size, move || tr("No playlists yet")))
        .child(ui_kit::body(
            size,
            tr(
                "Patina has no channels of its own. Add the playlist or account \
                your TV provider gave you, and the guide fills with its channels.",
            ),
        ))
        .child(button(ui, size, "Add playlist", Target::AddPlaylist))
        .child(hint(ui, size, "OK add a playlist   Back leave"))
}

fn choose_type(ui: Ui) -> impl IntoElement {
    let size = Scale::new();
    let row = move |kind: PlaylistKind| {
        let target = Target::Kind(kind);
        let mark = move || mark(ui, target);
        list_rows::row(size, ROW_TWO, mark)
            .role(AccessibilityRole::Button)
            .on_press(move || {
                ui.touched();
                send(ui, WizardEvent::Tap(target));
            })
            .child(
                ui_kit::label(size, move || words::kind_title(kind))
                    .text_color(move || mark().text()),
            )
            .child(
                ui_kit::detail(size, move || words::kind_detail(kind))
                    .text_color(move || mark().secondary()),
            )
    };
    card(ui, size)
        .child(ui_kit::title(size, move || {
            tr("What did your provider give you?")
        }))
        .children(PlaylistKind::ALL.map(row))
        .child(hint_or_back(ui, size, "▲▼ choose   OK next   Back back"))
}

fn shown_value(field: Field, value: &str) -> String {
    match field {
        // The file field holds the app's copy; its name is what matters.
        Field::PlaylistFile => value.rsplit(['/', '\\']).next().unwrap_or(value).to_owned(),
        _ if field.is_secret() => "•".repeat(value.chars().count().clamp(4, 16)),
        _ => value.to_owned(),
    }
}

fn field_row(ui: Ui, size: Scale, field: Field) -> impl IntoElement {
    let target = Target::Field(field);
    let value = move || read(ui, |w| w.value(field).to_owned()).unwrap_or_default();
    let error = move || read(ui, |w| w.error(field)).flatten();
    let mark = move || mark(ui, target);
    let paste = field.accepts_paste().then(|| {
        ui_kit::row_button(
            ui,
            size,
            move || tr("Paste"),
            mark,
            move || send(ui, WizardEvent::PasteInto(field)),
        )
    });
    list_rows::row(size, ROW, mark)
        .role(AccessibilityRole::Button)
        .flex_row()
        .items_center()
        .justify_start()
        .gap(move || size.px(12.0))
        .on_press(move || {
            ui.touched();
            send(ui, WizardEvent::Tap(target));
        })
        .child(
            ui_kit::detail(size, move || words::label(field))
                .w(move || size.px(LABEL_W))
                .text_color(move || mark().secondary()),
        )
        .child(
            view()
                .flex_1()
                .child(
                    ui_kit::label(size, move || {
                        let v = value();
                        if v.is_empty() {
                            words::placeholder(field).to_owned()
                        } else {
                            shown_value(field, &v)
                        }
                    })
                    .text_color(move || match (mark(), value().is_empty()) {
                        (Mark::Focused, false) => Mark::Focused.text(),
                        (Mark::Focused, true) => Mark::Focused.disabled(),
                        (_, false) => rgb(LIST_TEXT),
                        (_, true) => tint(WHITE, 0x70),
                    }),
                )
                .child(
                    ui_kit::caption(size, move || error().map_or("", words::field_error))
                        .number_of_lines(1)
                        .text_color(rgb(ERROR))
                        .h(move || size.px(if error().is_some() { 18.0 } else { 0.0 })),
                ),
        )
        .children(paste)
}

fn form(ui: Ui, kind: PlaylistKind) -> impl IntoElement {
    let banner = move || {
        read(ui, |w| match (w.notice(), w.failure()) {
            (Some(Notice::ClipboardEmpty), _) => Some(tr("There is no text to paste.")),
            (None, Some(f)) => Some(words::failure(kind, f)),
            (None, None) => None,
        })
        .flatten()
    };
    let size = Scale::new();
    card(ui, size)
        .test_id("wizard-form")
        .accessibility_elements_hidden(move || entry::is_editing(ui))
        .gap(move || size.px(4.0))
        .child(ui_kit::title(size, move || words::kind_title(kind)))
        .child(
            ui_kit::caption(size, move || banner().unwrap_or(""))
                .text_color(rgb(ERROR))
                .h(move || size.px(if banner().is_some() { 22.0 } else { 0.0 })),
        )
        .children(kind.fields().iter().map(|f| field_row(ui, size, *f)))
        .child(when(
            move || {
                read(ui, |w| {
                    kind.fields().iter().any(|field| {
                        matches!(
                            field,
                            Field::PlaylistUrl | Field::Server | Field::PortalUrl | Field::EpgUrl
                        ) && w
                            .value(*field)
                            .trim()
                            .to_ascii_lowercase()
                            .starts_with("http://")
                    })
                })
                .unwrap_or(false)
            },
            move || {
                ui_kit::caption(
                    size,
                    tr("Not encrypted: HTTP can expose your login and viewing activity."),
                )
            },
        ))
        .child(button(ui, size, "Add playlist", Target::Submit))
        .child(hint_or_back(
            ui,
            size,
            "▲▼ move   OK type   Menu paste   Back other types",
        ))
}

fn processing(ui: Ui) -> impl IntoElement {
    let progress = move || {
        read(ui, |w| match w.step() {
            Step::Processing { progress, .. } => progress,
            _ => None,
        })
        .flatten()
    };
    let percent = move || progress().and_then(|p| p.percent());
    let kind = read(ui, Wizard::kind).unwrap_or(PlaylistKind::M3uUrl);
    let size = Scale::new();
    card(ui, size)
        .child(ui_kit::title(size, move || tr("Adding the playlist")))
        .child(ui_kit::body(size, move || words::kind_title(kind)))
        .child(
            ui_kit::body(size, move || words::stage(progress()))
                .text_color(rgb(LIST_TEXT))
                .mt(move || size.px(8.0)),
        )
        .child(
            view()
                .h(px(4.0))
                .rounded(px(2.0))
                .bg(tint(WHITE, RESTING_ALPHA * 2))
                .opacity(move || if percent().is_some() { 1.0_f32 } else { 0.0 })
                .child(
                    view()
                        .h_full()
                        .rounded(px(2.0))
                        .w(move || percentage(f32::from(percent().unwrap_or(0)) / 100.0))
                        .bg(move || rgb(ui.theme.with(|t| t.accent.light))),
                ),
        )
        // OK does nothing here (no accidental cancel); Back or a tap cancels.
        .child(button(ui, size, "Cancel", Target::CancelImport))
        .child(hint(ui, size, "Back cancel"))
}

fn succeeded(ui: Ui) -> impl IntoElement {
    let counts = move || {
        read(ui, |w| match w.step() {
            Step::Succeeded { counts, .. } => words::counts(counts),
            _ => String::new(),
        })
        .unwrap_or_default()
    };
    let size = Scale::new();
    card(ui, size)
        .child(ui_kit::title(size, move || tr("Playlist added")))
        .child(ui_kit::body(size, counts).text_color(rgb(LIST_TEXT)))
        .child(ui_kit::body(size, move || {
            tr("The guide now shows its channels.")
        }))
        .child(button(ui, size, "Watch", Target::Done))
        .child(hint(ui, size, "OK continue"))
}

fn failed(ui: Ui) -> impl IntoElement {
    let message = move || {
        read(ui, |w| match w.step() {
            Step::Failed { failure } => words::failure(w.kind(), failure),
            _ => "",
        })
        .unwrap_or("")
        .to_owned()
    };
    let size = Scale::new();
    card(ui, size)
        .child(ui_kit::title(size, move || {
            tr("This source cannot be used")
        }))
        .child(ui_kit::body(size, message))
        .child(button(
            ui,
            size,
            "Choose another type",
            Target::ChooseAnotherType,
        ))
        .child(hint(ui, size, "OK choose another type"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screens_follow_steps() {
        let mut w = Wizard::first_run();
        assert!(screen_of(&w) == Some(Screen::Empty));
        w.handle(WizardEvent::Ok);
        assert!(screen_of(&w) == Some(Screen::ChooseType));
        w.handle(WizardEvent::Tap(Target::Kind(PlaylistKind::Stalker)));
        assert!(screen_of(&w) == Some(Screen::Form(PlaylistKind::Stalker)));
    }

    #[test]
    fn secrets_and_file_paths_are_not_shown_in_full() {
        assert_eq!(shown_value(Field::Password, "ab"), "••••");
        assert_eq!(shown_value(Field::Mac, "00:1A:79:00:00:01"), "•".repeat(16));
        assert_eq!(
            shown_value(Field::PlaylistFile, "/data/files/playlists/1/list.m3u"),
            "list.m3u"
        );
        assert_eq!(
            shown_value(Field::Server, "http://panel.example.test"),
            "http://panel.example.test"
        );
    }
}
