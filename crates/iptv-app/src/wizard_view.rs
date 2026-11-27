//! The first-run / add-playlist wizard screen. `iptv_presentation::wizard` owns the
//! flow, focus and validation; this draws each step full screen, feeds it
//! remote keys and taps, and performs its effects (text entry, clipboard,
//! file picker, import through the runtime's wizard operation).
//!
//! Remote: Up/Down move, OK activates (a field opens text entry), Menu
//! pastes into the focused field, Back goes back (on the first-run empty
//! state it leaves the app). Touch: every row and button is tappable.
//! Entry points: first run without a stored source, the top bar's "Add
//! playlist" (tap), and Menu in the groups pane (remote).

use crate::i18n::tr;

mod entry;
pub(crate) mod words;

use iptv_nav::Action;
use iptv_presentation::wizard::{
    Field, Notice, Origin, PlaylistKind, Step, Target, Wizard, WizardEffect, WizardEvent,
    WizardExit,
};
use nsh_core::picker::FilePickRequest;
use nsh_core::prelude::*;

use crate::data::{self, Job, Purpose, Update};
use crate::guide::list_rows::Mark;
use crate::scale::Scale;
use crate::setup_view;
use crate::theme::{LIST_TEXT, RADIUS, RESTING_ALPHA};
use crate::ui_kit::icons::{self, Icon};
use crate::ui_kit::{self, ERROR};
use crate::{Ui, tint};

const WHITE: u32 = 0xFFFFFF;

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
    if let Some(Step::Processing { id, .. } | Step::Canceling { id }) = read(ui, Wizard::step) {
        crate::runtime_owner::runtime().cancel_wizard_import(id, false);
    }
    ui.screens.wizard.set(Some(Open {
        wizard,
        editing: None,
        prefilled: false,
    }));
    prefill(ui);
}

/// A host that cannot import does not offer the wizard, and the user
/// hears why.
fn imports_unavailable() -> bool {
    if crate::host::IMPORTS {
        return false;
    }
    crate::toast::notify(crate::i18n::tr("This device cannot import playlists yet."));
    true
}

/// No playlists yet: offer library setup.
pub fn open_first_run(ui: Ui) {
    if imports_unavailable() {
        return;
    }
    open_with(
        ui,
        Wizard::first_run().with_restore(crate::backup::available()),
    );
}

/// Opens the add-playlist form.
pub fn open_add(ui: Ui) {
    if imports_unavailable() {
        return;
    }
    if crate::runtime_owner::runtime().parental_has_pin()
        && !crate::runtime_owner::runtime().parental_settings_allowed()
    {
        crate::navigation::request(ui, iptv_core::parental::Target::settings(), move || {
            open_add(ui)
        });
        return;
    }
    if !is_open(ui) {
        open_with(ui, Wizard::add_playlist());
    }
}

pub fn is_open(ui: Ui) -> bool {
    ui.screens.wizard.with(Option::is_some)
}

fn read<R>(ui: Ui, f: impl FnOnce(&Wizard) -> R) -> Option<R> {
    ui.screens.wizard.with(|w| w.as_ref().map(|o| f(&o.wizard)))
}

/// Feeds `event` to the wizard and performs what it asks for.
pub fn send(ui: Ui, event: WizardEvent) {
    let submit = matches!(&event, WizardEvent::Tap(Target::Submit))
        || (matches!(&event, WizardEvent::Ok)
            && read(ui, |w| w.focus() == Some(Target::Submit)).unwrap_or(false));
    if submit
        && crate::runtime_owner::runtime().parental_has_pin()
        && !crate::runtime_owner::runtime().parental_settings_allowed()
    {
        crate::navigation::request(ui, iptv_core::parental::Target::settings(), move || {
            send(ui, event)
        });
        return;
    }
    let mut effect = None;
    ui.screens.wizard.update(|w| {
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
/// build has one ([`crate::runtime::build_config::DEV_PLAYLIST`]).
fn prefill(ui: Ui) {
    let Some(url) = crate::runtime::build_config::DEV_PLAYLIST.filter(|u| !u.is_empty()) else {
        return;
    };
    let due = read(ui, |w| {
        w.step() == Step::Form
            && w.kind() == PlaylistKind::M3uUrl
            && w.value(Field::PlaylistUrl).is_empty()
    })
    .unwrap_or(false)
        && ui
            .screens
            .wizard
            .with(|w| w.as_ref().is_some_and(|o| !o.prefilled));
    if due {
        ui.screens.wizard.update(|w| {
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
        WizardEffect::RestoreBackup => crate::backup::request_restore(ui),
        WizardEffect::EditField { field } => entry::begin(ui, field),
        WizardEffect::ReadClipboard { field } => crate::tasks::spawn_in_app(async move {
            let text = clipboard_string().await;
            send(
                ui,
                WizardEvent::ClipboardText {
                    field,
                    text: Some(text),
                },
            );
        }),
        WizardEffect::PickFile => crate::tasks::spawn_in_app(async move {
            let files = pick_files(FilePickRequest::new(false, 1)).await;
            let path = if let Some(file) = files.first() {
                crate::runtime_owner::runtime()
                    .keep_picked_file(&file.name, file.data.bytes())
                    .await
                    .inspect_err(|e| log::warn!("iptv wizard: keeping the file failed: {e}"))
                    .ok()
            } else {
                None
            };
            send(ui, WizardEvent::FilePicked(path));
        }),
        WizardEffect::StartImport { draft } => {
            let runtime = crate::runtime_owner::runtime();
            let fallback = iptv_presentation::i18n::translate(crate::i18n::language(), "Playlist");
            let (source, epg_url) = data::source_from_draft(&draft, fallback);
            let source_kind = source.kind();
            let job = Job {
                source,
                epg_url,
                stored: None,
                purpose: Purpose::Add,
                edit: false,
            };
            let (id, _) = runtime.start_wizard_import(
                job,
                move |id, _store_epoch| {
                    ui.wizard.update(|open| {
                        if let Some(open) = open {
                            open.wizard.admit_import(id);
                        }
                    });
                },
                move |id, update| {
                    send(
                        ui,
                        match update {
                            Update::Progress(progress) => WizardEvent::Progress { id, progress },
                            Update::Done(Ok(counts)) => WizardEvent::Succeeded { id, counts },
                            Update::Done(Err(failure)) => WizardEvent::Failed { id, failure },
                        },
                    )
                },
            );
            log::info!("iptv wizard: import {id:?} of {source_kind:?}");
        }
        WizardEffect::CancelImport { id } => {
            let wait_for_commit = read(ui, Wizard::step) == Some(Step::Canceling { id });
            crate::runtime_owner::runtime().cancel_wizard_import(id, wait_for_commit);
        }
        WizardEffect::Close(exit) => {
            let runtime = crate::runtime_owner::runtime();
            runtime.parental_content_closed();
            log::info!("iptv wizard: closed ({exit:?})");
            if let WizardExit::Added { .. } = exit {
                // Added from the playlists or settings screen: "Watch"
                // goes to the guide.
                crate::playlists_view::close(ui);
                crate::settings_view::close(ui);
            }
            ui.screens.wizard.set(None);
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
        Action::MoveLeft => send(ui, WizardEvent::Left),
        Action::MoveRight => send(ui, WizardEvent::Right),
        Action::Select => send(ui, WizardEvent::Ok),
        Action::ShowMenu => send(ui, WizardEvent::Paste),
        // On the first run's welcome screen, where Back leaves: the
        // question first, where the host asks it (`crate::confirm_leaving`).
        Action::Back => {
            let _ = back(ui) || crate::ask_before_leaving(ui);
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
    if back_leaves(ui) {
        return false;
    }
    send(ui, WizardEvent::Back);
    true
}

/// The welcome screen has no previous step. Keep the platform's Back
/// gesture available there, unless text entry still needs to close.
pub(crate) fn back_leaves(ui: Ui) -> bool {
    !entry::is_editing(ui)
        && read(ui, |w| {
            w.origin() == Origin::FirstRun && w.step() == Step::Empty
        }) == Some(true)
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
        Step::Admitting | Step::Processing { .. } | Step::Canceling { .. } => Screen::Processing,
        Step::Succeeded { .. } => Screen::Succeeded,
        Step::Failed { .. } => Screen::Failed,
        Step::Closed => return None,
    })
}

pub fn layer(ui: Ui) -> impl IntoElement {
    let screen = computed(move || {
        ui.screens
            .wizard
            .with(|w| w.as_ref().and_then(|o| screen_of(&o.wizard)))
    });
    let editing = computed(move || entry::is_editing(ui));
    when(
        move || screen.get().is_some(),
        move || {
            view()
                .absolute()
                .inset_0()
                .test_id("wizard-layer")
                .bg(move || rgb(ui.app.theme.with(|t| t.background.window)))
                .accessibility_elements_hidden(move || {
                    crate::overlays::Overlay::Wizard.accessibility_covered(ui)
                })
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

/// A button for `target`, revealed when the keys focus it.
fn button(
    ui: Ui,
    size: Scale,
    label: &'static str,
    scrolling: ui_kit::FocusScroll,
    target: Target,
) -> View {
    let has_focus = move || focused(ui, target);
    scrolling.control(
        ui_kit::button(
            ui,
            size,
            move || tr(label),
            has_focus,
            move || send(ui, WizardEvent::Tap(target)),
        ),
        has_focus,
    )
}

fn choice(
    ui: Ui,
    size: Scale,
    scrolling: ui_kit::FocusScroll,
    target: Target,
    title: impl Fn() -> &'static str + Copy + 'static,
    detail: impl Fn() -> &'static str + Copy + 'static,
    icon: Option<Icon>,
) -> View {
    let has_focus = move || focused(ui, target);
    let mark = move || Mark::of(has_focus(), false);
    let background = move || {
        if has_focus() {
            mark().bg()
        } else {
            rgb(ui.app.theme.with(|theme| theme.background.alt))
        }
    };
    scrolling.control(
        view()
            .role(AccessibilityRole::Button)
            .accessible(true)
            .label(move || title().to_owned())
            .hint(detail)
            .flex_row()
            .items_center()
            .gap(move || size.px(12.0))
            .p(move || size.px(14.0))
            .rounded(px(RADIUS))
            .bg(background)
            .on_press(move || {
                ui.touched();
                send(ui, WizardEvent::Tap(target));
            })
            .children(icon.map(|icon| {
                svg(icons::src(icon))
                    .size(move || size.px(24.0))
                    .flex_shrink_0()
                    .text_color(move || mark().text())
            }))
            .child(
                view()
                    .flex_1()
                    .min_w(px(0.0))
                    .gap(move || size.px(4.0))
                    .child(
                        ui_kit::body(size, title)
                            .font_weight(crate::fonts::MEDIUM)
                            .text_color(move || mark().text()),
                    )
                    .child(ui_kit::caption(size, detail).text_color(move || mark().secondary())),
            )
            .child(
                ui_kit::label(size, "›")
                    .accessible(false)
                    .text_color(move || mark().text()),
            ),
        has_focus,
    )
}

fn empty(ui: Ui) -> View {
    let size = Scale::new();
    let scrolling = ui_kit::FocusScroll::new();
    // Live: Android reports its document pickers after this is built.
    let can_restore = move || read(ui, Wizard::can_restore).unwrap_or(false);
    let content = view()
        .gap(move || size.px(10.0))
        .child(
            choice(
                ui,
                size,
                scrolling,
                Target::AddPlaylist,
                || tr("Add a playlist"),
                || tr("Use a link, a file, or a provider login"),
                Some(Icon::ChannelList),
            )
            .test_id("wizard-welcome-add"),
        )
        .child(when(can_restore, move || {
            choice(
                ui,
                size,
                scrolling,
                Target::RestoreBackup,
                || tr("Restore a backup"),
                || tr("Open a Patina backup file"),
                Some(Icon::BackToApp),
            )
            .test_id("wizard-welcome-restore")
        }));
    setup_view::frame(
        ui,
        size,
        || tr("Library setup"),
        scrolling,
        setup_view::columns(
            size,
            setup_view::intro(
                size,
                || tr("Set up your library"),
                move || {
                    if can_restore() {
                        tr("Add a playlist from your TV provider, or bring your library over from another Patina installation.")
                    } else {
                        tr("Add a playlist from your TV provider.")
                    }
                },
            ),
            content,
        )
        .test_id("wizard-welcome"),
    )
}

fn choose_type(ui: Ui) -> View {
    let size = Scale::new();
    let scrolling = ui_kit::FocusScroll::new();
    let content = view()
        .gap(move || size.px(8.0))
        .children(PlaylistKind::ALL.map(|kind| {
            choice(
                ui,
                size,
                scrolling,
                Target::Kind(kind),
                move || words::kind_title(kind),
                move || words::kind_detail(kind),
                None,
            )
            .accessibility_state(move || AccessibilityState {
                selected: read(ui, Wizard::kind) == Some(kind),
                ..AccessibilityState::default()
            })
        }))
        .child(
            button(ui, size, "Back", scrolling, Target::Back)
                .self_start()
                .test_id("wizard-source-back"),
        );
    setup_view::frame(
        ui,
        size,
        || tr("Library setup"),
        scrolling,
        setup_view::columns(
            size,
            setup_view::intro(
                size,
                || tr("Source type"),
                || tr("What did your provider give you?"),
            ),
            content,
        )
        .test_id("wizard-source-picker"),
    )
}

fn shown_value(field: Field, value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    match field {
        // The file field holds the app's copy; its name is what matters.
        Field::PlaylistFile => value.rsplit(['/', '\\']).next().unwrap_or(value).to_owned(),
        _ if field.is_secret() => "•".repeat(value.chars().count().clamp(4, 16)),
        _ => value.to_owned(),
    }
}

fn form_control(ui: Ui, size: Scale, target: Target) -> View {
    let has_focus = move || focused(ui, target);
    view()
        .role(AccessibilityRole::Button)
        .accessible(true)
        .min_h(move || size.px(ui_kit::TOUCH))
        .px(move || size.px(12.0))
        .py(move || size.px(8.0))
        .rounded(px(RADIUS))
        .border_1()
        .border_color(move || ui_kit::field_border(ui, has_focus()))
        .bg(move || ui_kit::field_fill(ui, has_focus()))
        .justify_center()
        .on_press(move || {
            ui.touched();
            send(ui, WizardEvent::Tap(target));
        })
}

fn source_type(ui: Ui, size: Scale, scrolling: ui_kit::FocusScroll, kind: PlaylistKind) -> View {
    let target = Target::SourceType;
    view()
        .gap(move || size.px(4.0))
        .child(ui_kit::caption(size, || tr("Source type")))
        .child(
            scrolling.control(
                form_control(ui, size, target)
                    .test_id("wizard-source-type")
                    .label(move || format!("{} · {}", tr("Source type"), words::kind_title(kind)))
                    .accessibility_state(AccessibilityState {
                        expanded: Some(false),
                        has_popup: true,
                        ..AccessibilityState::default()
                    })
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap(move || size.px(8.0))
                            .child(
                                ui_kit::body(size, move || words::kind_title(kind))
                                    .flex_1()
                                    .text_color(rgb(LIST_TEXT)),
                            )
                            .child(ui_kit::label(size, "⌄").accessible(false)),
                    ),
                move || focused(ui, target),
            ),
        )
}

fn field_row(ui: Ui, size: Scale, scrolling: ui_kit::FocusScroll, field: Field) -> View {
    let target = Target::Field(field);
    let value = move || read(ui, |w| w.value(field).to_owned()).unwrap_or_default();
    let error = move || read(ui, |w| w.error(field)).flatten();
    let field_control = form_control(ui, size, target)
        .test_id(match field {
            Field::PlaylistUrl => "wizard-field-PlaylistUrl",
            Field::PlaylistFile => "wizard-field-PlaylistFile",
            Field::EpgUrl => "wizard-field-EpgUrl",
            _ => "wizard-field",
        })
        .flex_1()
        .min_w(px(0.0))
        .label(move || {
            let label = words::label(field);
            let content = shown_value(field, &value());
            let name = if content.is_empty() {
                label.to_owned()
            } else {
                format!("{label} · {content}")
            };
            if let Some(error) = error() {
                format!("{name} · {}", words::field_error(error))
            } else {
                name
            }
        })
        .accessibility_state(move || AccessibilityState {
            invalid: error().is_some(),
            ..AccessibilityState::default()
        })
        .child(
            ui_kit::body(size, move || {
                let value = shown_value(field, &value());
                if value.is_empty() {
                    words::placeholder(field).to_owned()
                } else {
                    value
                }
            })
            .number_of_lines(move || if value().is_empty() { 0_u32 } else { 1 })
            .text_color(move || {
                if value().is_empty() {
                    ui_kit::secondary()
                } else {
                    rgb(LIST_TEXT)
                }
            }),
        );
    view()
        .gap(move || size.px(4.0))
        .child(ui_kit::caption(size, move || words::label(field)))
        .child(
            view()
                .flex_row()
                .gap(move || size.px(8.0))
                .child(scrolling.control(field_control, move || focused(ui, target)))
                .children((field == Field::PlaylistUrl).then(|| {
                    button(ui, size, "Paste", scrolling, Target::PastePlaylistLink)
                        .label(move || format!("{} · {}", tr("Paste"), words::label(field)))
                        .test_id("wizard-paste")
                        .flex_shrink_0()
                })),
        )
        .child(when(
            move || error().is_some(),
            move || {
                ui_kit::caption(size, move || error().map_or("", words::field_error))
                    .text_color(rgb(ERROR))
            },
        ))
}

fn form(ui: Ui, kind: PlaylistKind) -> View {
    let banner = move || {
        read(ui, |w| match (w.notice(), w.failure()) {
            (Some(Notice::ClipboardEmpty), _) => Some(tr("There is no text to paste.")),
            (None, Some(f)) => Some(words::failure(kind, f)),
            (None, None) => None,
        })
        .flatten()
    };
    let expanded = move || read(ui, Wizard::optional_expanded).unwrap_or_default();
    let size = Scale::new();
    let scrolling = ui_kit::FocusScroll::new();
    let options = Target::OptionalDetails;
    let submit = Target::Submit;
    let content = view()
        .gap(move || size.px(12.0))
        .child(source_type(ui, size, scrolling, kind))
        .children(
            kind.fields()
                .iter()
                .copied()
                .filter(|field| field.is_required())
                .map(|field| field_row(ui, size, scrolling, field)),
        )
        .child(
            scrolling.control(
                form_control(ui, size, options)
                    .test_id("wizard-options")
                    .label(move || tr("Name and guide link (optional)").to_owned())
                    .accessibility_state(move || AccessibilityState {
                        expanded: Some(expanded()),
                        ..AccessibilityState::default()
                    })
                    .flex_row()
                    .items_center()
                    .justify_start()
                    .gap(move || size.px(8.0))
                    .child(
                        ui_kit::label(size, move || if expanded() { "⌄" } else { "›" })
                            .accessible(false),
                    )
                    .child(
                        ui_kit::body(size, || tr("Name and guide link (optional)"))
                            .text_color(rgb(LIST_TEXT)),
                    ),
                move || focused(ui, options),
            ),
        )
        .child(when(expanded, move || {
            view().gap(move || size.px(12.0)).children(
                kind.fields()
                    .iter()
                    .copied()
                    .filter(|field| !field.is_required())
                    .map(|field| field_row(ui, size, scrolling, field)),
            )
        }))
        .child(when(
            move || banner().is_some(),
            move || ui_kit::caption(size, move || banner().unwrap_or("")).text_color(rgb(ERROR)),
        ))
        .child(when(
            move || {
                read(ui, |w| {
                    kind.fields().iter().any(|field| {
                        field.is_url()
                            && w.value(*field)
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
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .gap(move || size.px(10.0))
                .child(
                    scrolling.control(
                        setup_view::primary(
                            ui,
                            size,
                            || tr("Add playlist"),
                            move || focused(ui, submit),
                            move || send(ui, WizardEvent::Tap(submit)),
                        )
                        .test_id("wizard-submit"),
                        move || focused(ui, submit),
                    ),
                )
                .child(button(ui, size, "Back", scrolling, Target::Back)),
        );
    setup_view::frame(
        ui,
        size,
        || tr("Library setup"),
        scrolling,
        setup_view::columns(
            size,
            setup_view::intro(
                size,
                || tr("Add a playlist"),
                move || match kind {
                    PlaylistKind::M3uUrl => {
                        tr("Use the M3U or M3U8 link your TV provider gave you.")
                    }
                    _ => words::kind_detail(kind),
                },
            ),
            content,
        )
        .accessibility_elements_hidden(move || entry::is_editing(ui))
        .test_id("wizard-form"),
    )
}

fn processing(ui: Ui) -> View {
    let canceling = move || matches!(read(ui, Wizard::step), Some(Step::Canceling { .. }));
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
    let scrolling = ui_kit::FocusScroll::new();
    let content = view()
        .gap(move || size.px(12.0))
        .child(
            ui_kit::body(size, move || {
                if canceling() {
                    tr("Canceling…").to_owned()
                } else {
                    words::stage(progress())
                }
            })
            .text_color(rgb(LIST_TEXT)),
        )
        .child(when(
            move || percent().is_some(),
            move || {
                view()
                    .h(px(4.0))
                    .rounded(px(2.0))
                    .bg(tint(WHITE, RESTING_ALPHA * 2))
                    .child(
                        view()
                            .h_full()
                            .rounded(px(2.0))
                            .w(move || percentage(f32::from(percent().unwrap_or(0)) / 100.0))
                            .bg(move || rgb(ui.app.theme.with(|t| t.accent.light))),
                    )
            },
        ))
        // OK does nothing here (no accidental cancel); Back or a tap cancels.
        .child(when(
            move || !canceling(),
            move || button(ui, size, "Cancel", scrolling, Target::CancelImport).self_start(),
        ));
    setup_view::frame(
        ui,
        size,
        || tr("Library setup"),
        scrolling,
        setup_view::columns(
            size,
            setup_view::intro(
                size,
                || tr("Adding the playlist"),
                move || words::kind_title(kind),
            ),
            content,
        ),
    )
}

fn succeeded(ui: Ui) -> View {
    let counts = move || {
        read(ui, |w| match w.step() {
            Step::Succeeded { counts, .. } => words::counts(counts),
            _ => String::new(),
        })
        .unwrap_or_default()
    };
    let size = Scale::new();
    let scrolling = ui_kit::FocusScroll::new();
    let done = Target::Done;
    setup_view::frame(
        ui,
        size,
        || tr("Library setup"),
        scrolling,
        setup_view::columns(
            size,
            setup_view::intro(
                size,
                || tr("Playlist added"),
                || tr("The guide now shows its channels."),
            ),
            view()
                .gap(move || size.px(12.0))
                .child(ui_kit::body(size, counts).text_color(rgb(LIST_TEXT)))
                .child(
                    scrolling.control(
                        setup_view::primary(
                            ui,
                            size,
                            || tr("Watch"),
                            move || focused(ui, done),
                            move || send(ui, WizardEvent::Tap(done)),
                        )
                        .self_start(),
                        move || focused(ui, done),
                    ),
                ),
        ),
    )
}

fn failed(ui: Ui) -> View {
    let message = move || {
        read(ui, |w| match w.step() {
            Step::Failed { failure } => words::failure(w.kind(), failure),
            _ => "",
        })
        .unwrap_or("")
    };
    let size = Scale::new();
    let scrolling = ui_kit::FocusScroll::new();
    setup_view::frame(
        ui,
        size,
        || tr("Library setup"),
        scrolling,
        setup_view::columns(
            size,
            setup_view::intro(size, || tr("This source cannot be used"), message),
            view().child(
                button(
                    ui,
                    size,
                    "Choose another type",
                    scrolling,
                    Target::ChooseAnotherType,
                )
                .self_start(),
            ),
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screens_follow_steps() {
        let mut w = Wizard::first_run();
        assert!(screen_of(&w) == Some(Screen::Empty));
        w.handle(WizardEvent::Ok);
        assert!(screen_of(&w) == Some(Screen::Form(PlaylistKind::M3uUrl)));
        w.handle(WizardEvent::Ok);
        assert!(screen_of(&w) == Some(Screen::ChooseType));
        w.handle(WizardEvent::Tap(Target::Kind(PlaylistKind::Stalker)));
        assert!(screen_of(&w) == Some(Screen::Form(PlaylistKind::Stalker)));
    }

    #[test]
    fn secrets_and_file_paths_are_not_shown_in_full() {
        assert_eq!(shown_value(Field::Password, ""), "");
        assert_eq!(shown_value(Field::Username, ""), "");
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
