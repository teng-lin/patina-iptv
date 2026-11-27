//! Live, validated remote overrides. Editor/menu input stays at defaults.
//!
//! The keys offered follow the keys connected now (`input::keys_for`): a
//! keyboard's leave out the keys only a remote has. A hold is offered
//! only where keys report their release (`input::long_press`).
use crate::chrome::i18n::{tr, trf};

use crate::chrome::scale::Scale;
use crate::screens::settings_view::remote_keys::{action_name, context_name, editable, key_name};
use crate::{Ui, chrome::ui_kit, wiring::settings};
use iptv_nav::remap::{self, Confirm, Editor, FIELDS, Field};
use iptv_nav::{Action, Binding, Context, Keymap, Press, RemoteKey};
use iptv_presentation::capabilities::Keys;
use iptv_settings::{KeyBinding, PressKind, Target, Value, keys};
use nsh_core::prelude::*;

fn saved() -> Vec<KeyBinding> {
    settings::get()
        .key_bindings(keys::KEY_OVERRIDES, &Target::Global)
        .to_vec()
}
fn validated(b: &KeyBinding) -> Option<Binding> {
    remap::checked_binding(
        b.context.as_deref()?,
        &b.key,
        b.press == PressKind::Long,
        &b.action,
    )
}
pub fn keymap() -> Keymap {
    let mut map = if idiom() == nsh_core::platform::Idiom::Tv {
        Keymap::tv()
    } else {
        Keymap::for_profile(crate::chrome::input::PROFILE)
    };
    for b in saved().iter().filter_map(validated) {
        map.set_override(b);
    }
    // Where Record now is not offered, nothing is bound to it by default
    // and a binding saved elsewhere does nothing (`recordings_view::current`).
    if !untracked(crate::wiring::capabilities::record_now).offered() {
        map.unbind_defaults(Action::Record);
    }
    map
}

/// The actions a binding can choose here: not Record where Record now is
/// not offered.
fn bindable(editor: &Editor) -> Vec<Action> {
    let record = untracked(crate::wiring::capabilities::record_now).offered();
    editor
        .actions()
        .into_iter()
        .filter(|&action| record || action != Action::Record)
        .collect()
}
pub fn overridden(context: Context, key: RemoteKey) -> bool {
    saved()
        .iter()
        .filter_map(validated)
        .any(|b| b.context == Some(context) && b.key == key)
}
/// Preserve an explicit toggle for the resolved press without changing dedicated media
/// key semantics merely because a long press has been remapped.
pub(crate) fn media_toggle_override(context: Context, key: RemoteKey, press: Press) -> bool {
    saved().iter().filter_map(validated).any(|b| {
        b.key == key
            && b.press == press
            && b.action == Action::PlayPause
            && (b.context.is_none() || b.context == Some(context))
    })
}
/// The keys set up now: a remote's where none are connected (the
/// editor opens only from the keys' page).
fn keys_now() -> Keys {
    crate::chrome::input::keys_now().unwrap_or(Keys::Remote)
}

/// The keys the editor cycles through now.
fn keys() -> Vec<RemoteKey> {
    editable(keys_now())
}

/// Whether the editor offers a hold: where keys report their release.
fn holds() -> bool {
    crate::chrome::input::long_press()
}

/// A key that reports no hold (a shortcut with a modifier) is bound for
/// a tap.
fn tap_only(s: &mut Editor) {
    if !crate::chrome::input::holds(s.key) {
        s.press = Press::Short;
    }
}

/// The editor's fields shown: without the press where a hold cannot be
/// told from a tap.
fn fields() -> impl Iterator<Item = Field> {
    FIELDS
        .iter()
        .copied()
        .filter(|field| *field != Field::Press || holds())
}

pub fn open(ui: Ui) {
    let mut editor = Editor::default();
    // A key this keyboard does not have starts on its first one.
    let keys = keys();
    if !keys.contains(&editor.key) {
        editor.key = keys[0];
    }
    ui.screens.remote_editor.set(Some(editor));
}

/// Moves the focus `by` fields, past one not shown.
fn step(s: &mut Editor, by: isize) {
    s.step(by);
    if s.focus == Field::Press && !holds() {
        s.step(by);
        // At an end, stay on the field the focus came from.
        if s.focus == Field::Press {
            s.step(-by);
        }
    }
}
pub fn is_open(ui: Ui) -> bool {
    ui.screens.remote_editor.with(Option::is_some)
}
/// Whether the editor waits for a key to bind (tracked).
pub(crate) fn capturing(ui: Ui) -> bool {
    ui.screens
        .remote_editor
        .with(|s| s.as_ref().is_some_and(|s| s.capturing))
}
fn state<R>(ui: Ui, f: impl FnOnce(&Editor) -> R) -> Option<R> {
    ui.screens.remote_editor.with(|s| s.as_ref().map(f))
}
fn update(ui: Ui, f: impl FnOnce(&mut Editor)) {
    ui.screens.remote_editor.update(|s| {
        if let Some(s) = s {
            f(s)
        }
    })
}
pub fn back(ui: Ui) -> bool {
    let mut close = false;
    update(ui, |s| close = !s.back());
    if close {
        ui.screens.remote_editor.set(None);
    }
    true
}
fn close(ui: Ui) {
    ui.screens.remote_editor.set(None);
}
pub fn capture(ui: Ui, key: RemoteKey) {
    if key == RemoteKey::Back {
        return;
    } // Platform Back cancels capture once.
    update(ui, |s| {
        s.capturing = false;
        if key != RemoteKey::Back {
            s.key = key;
            tap_only(s);
        }
    });
}
fn cycle<T: Copy + PartialEq>(value: T, values: &[T], by: isize) -> T {
    if values.is_empty() {
        return value;
    }
    let at = values.iter().position(|v| *v == value).unwrap_or(0);
    values[(at as isize + by).rem_euclid(values.len() as isize) as usize]
}
fn change(ui: Ui, by: isize) {
    if state(ui, |s| s.confirm.is_some()).unwrap_or(false) {
        update(ui, |s| s.accept = !s.accept);
        return;
    }
    update(ui, |s| match s.focus {
        Field::Context => s.context = cycle(s.context, remap::CONTEXTS, by),
        Field::Key => {
            s.key = cycle(s.key, &keys(), by);
            tap_only(s);
        }
        Field::Press if crate::chrome::input::holds(s.key) => {
            s.press = if s.press == Press::Short {
                Press::Long
            } else {
                Press::Short
            }
        }
        Field::Action => s.action = cycle(s.action, &bindable(s), by),
        _ => {}
    });
}
fn same(s: &Editor, b: &KeyBinding) -> bool {
    b.context.as_deref() == Some(s.context.token())
        && b.key == s.key.name()
        && (b.press == PressKind::Long) == (s.press == Press::Long)
}
fn store(bindings: Vec<KeyBinding>) {
    if let Err(e) = settings::set(keys::KEY_OVERRIDES.as_str(), Value::KeyBindings(bindings)) {
        log::warn!("iptv remote: {e}");
    }
}
fn save(ui: Ui) {
    let Some(s) = state(ui, Clone::clone) else {
        return;
    };
    let Some(b) = s.binding() else { return };
    let mut bindings = saved();
    bindings.retain(|b| !same(&s, b));
    bindings.push(KeyBinding::new(
        Some(s.context.token()),
        &b.key.name(),
        if b.press == Press::Long {
            PressKind::Long
        } else {
            PressKind::Short
        },
        b.action.token(),
    ));
    store(bindings);
    update(ui, |s| s.confirm = None);
}
fn activate(ui: Ui, field: Field) {
    update(ui, |s| s.focus = field);
    let Some(s) = state(ui, Clone::clone) else {
        return;
    };
    if let Some(confirm) = s.confirm {
        if s.accept {
            match confirm {
                Confirm::Replace => save(ui),
                Confirm::Reset => store(Vec::new()),
            }
        }
        update(ui, |s| s.confirm = None);
        return;
    }
    match field {
        Field::Capture => update(ui, |s| s.capturing = true),
        Field::Save => {
            if s.binding().is_none() {
                return;
            }
            if saved().iter().any(|b| same(&s, b)) {
                update(ui, |s| {
                    s.confirm = Some(Confirm::Replace);
                    s.accept = false;
                });
            } else {
                save(ui);
            }
        }
        Field::Clear => {
            let mut b = saved();
            b.retain(|b| !same(&s, b));
            store(b);
        }
        Field::Reset => update(ui, |s| {
            s.confirm = Some(Confirm::Reset);
            s.accept = false;
        }),
        _ => change(ui, 1),
    }
}
pub fn key(ui: Ui, action: Action) {
    match action {
        Action::MoveUp => update(ui, |s| step(s, -1)),
        Action::MoveDown => update(ui, |s| step(s, 1)),
        Action::MoveLeft => change(ui, -1),
        Action::MoveRight => change(ui, 1),
        Action::Select => {
            if let Some(field) = state(ui, |s| s.focus) {
                activate(ui, field)
            }
        }
        Action::Back => {
            back(ui);
        }
        _ => {}
    }
}
fn field_label(s: &Editor, field: Field) -> String {
    match field {
        Field::Context => trf(
            "Context: {context}",
            &[("context", context_name(s.context).into())],
        ),
        Field::Key => trf("Key: {key}", &[("key", (&key_name(s.key)).into())]),
        Field::Press => {
            let press = if s.press == Press::Long {
                tr("Hold")
            } else {
                tr("Short")
            };
            trf("Press: {press}", &[("press", press.into())])
        }
        Field::Action => trf(
            "Action: {action}",
            &[("action", action_name(s.action).into())],
        ),
        Field::Capture => tr("Capture next remote key").into(),
        Field::Save => {
            if s.binding().is_some() {
                tr("Save binding").into()
            } else {
                tr("Choose an allowed action (short Back is reserved)").into()
            }
        }
        Field::Clear => tr("Clear this override / use default").into(),
        Field::Reset => tr("Restore all default bindings").into(),
    }
}
/// Under the screen's title: how the rows are used.
const CAPTION: &str = crate::chrome::i18n::key(
    "Up/Down chooses a row; Left/Right or tap changes it. \
     Save applies immediately. Back remains available.",
);

/// What the chosen key does now and whether that is the user's binding;
/// while a key is being captured, what the editor waits for.
fn status(ui: Ui) -> String {
    ui.app.settings.get();
    state(ui, |s| {
        if s.capturing {
            return tr("Press a key now. Back cancels capture.").into();
        }
        let effective = keymap()
            .resolve(s.context, s.key, s.press)
            .map_or(tr("Nothing"), action_name);
        let custom = saved().iter().any(|b| same(s, b));
        let action = [("action", effective.into())];
        if custom {
            trf("Currently: {action} · custom binding", &action)
        } else {
            trf("Currently: {action} · default binding", &action)
        }
    })
    .unwrap_or_default()
}

/// The row of `field`: its value, changed or acted on by a press.
fn field_row(ui: Ui, size: Scale, field: Field) -> impl IntoElement {
    ui_kit::action(
        ui,
        size,
        move || state(ui, |s| field_label(s, field)).unwrap_or_default(),
        move || state(ui, |s| s.focus) == Some(field),
        0xFFFFFF,
        move || activate(ui, field),
    )
    .test_id(&format!("remote-{field:?}"))
    .accessibility_elements_hidden(move || contents_hidden(ui))
}

/// The question before a binding replaces another or all are restored.
fn confirm_sheet(ui: Ui, size: Scale) -> impl IntoElement {
    let question = move || {
        if state(ui, |s| s.confirm) == Some(Some(Confirm::Replace)) {
            tr("Replace the existing binding?")
        } else {
            tr("Restore all defaults?")
        }
    };
    let cancel = move || {
        update(ui, |s| s.confirm = None);
    };
    let confirm = move || {
        update(ui, |s| s.accept = true);
        if let Some(f) = state(ui, |s| s.focus) {
            activate(ui, f)
        }
    };
    view()
        .absolute()
        .inset_0()
        .items_center()
        .justify_center()
        .bg(crate::tint(0, 190))
        .child(
            ui_kit::card(ui, size, 600.0)
                .child(ui_kit::title(size, question))
                .child(ui_kit::action(
                    ui,
                    size,
                    move || tr("Cancel"),
                    move || state(ui, |s| !s.accept).unwrap_or(false),
                    0xFFFFFF,
                    cancel,
                ))
                .child(ui_kit::action(
                    ui,
                    size,
                    move || tr("Confirm"),
                    move || state(ui, |s| s.accept).unwrap_or(false),
                    0xFFFFFF,
                    confirm,
                )),
        )
}

/// The editor's title, after the keys it sets up.
fn title() -> &'static str {
    match keys_now() {
        Keys::Keyboard => tr("Keyboard shortcuts"),
        Keys::Remote => tr("Remote bindings"),
    }
}

fn contents_hidden(ui: Ui) -> bool {
    state(ui, |screen| screen.confirm.is_some()).unwrap_or(false)
}

pub fn layer(ui: Ui) -> impl IntoElement {
    let size = Scale::new();
    // The keys it sets up attached or detached while it is open: it
    // closes, as the keys' page follows them (#280), so it never offers
    // keys, or a title, of keys no longer there.
    let keys = computed(crate::chrome::input::keys_now);
    effect(move || {
        keys.with(|_| ());
        untracked(|| {
            if is_open(ui) {
                close(ui);
            }
        });
    });
    when(
        move || is_open(ui),
        move || {
            ui_kit::screen(ui, size)
                .test_id("remote-editor-screen")
                .accessibility_elements_hidden(move || {
                    crate::chrome::overlays::Overlay::RemoteEditor.accessibility_covered(ui)
                })
                .child(
                    ui_kit::header(ui, size, title(), close)
                        .accessibility_elements_hidden(move || contents_hidden(ui)),
                )
                .child(
                    ui_kit::caption(size, tr(CAPTION))
                        .accessibility_elements_hidden(move || contents_hidden(ui)),
                )
                .child(
                    ui_kit::body(size, move || status(ui))
                        .accessibility_elements_hidden(move || contents_hidden(ui)),
                )
                .children(fields().map(move |field| field_row(ui, size, field)))
                .child(when(
                    move || state(ui, |s| s.confirm.is_some()).unwrap_or(false),
                    move || confirm_sheet(ui, size),
                ))
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A shortcut with a modifier reports its press only: it is bound for
    /// a tap, whatever the press shown before (#280).
    #[test]
    fn a_shortcut_with_a_modifier_is_bound_for_a_tap() {
        let phases = crate::chrome::input::long_press();
        for key in [RemoteKey::Left, RemoteKey::Red, RemoteKey::Search] {
            assert_eq!(crate::chrome::input::holds(key), phases, "{key:?}");
        }
        for key in [
            RemoteKey::ShiftLeft,
            RemoteKey::CtrlRight,
            RemoteKey::ShiftTab,
        ] {
            assert!(!crate::chrome::input::holds(key), "{key:?}");
            let mut editor = Editor {
                key,
                press: Press::Long,
                ..Editor::default()
            };
            tap_only(&mut editor);
            assert_eq!(editor.press, Press::Short, "{key:?}");
        }
    }
    #[test]
    fn persisted_overrides_cannot_take_editor_input_or_short_back() {
        assert!(validated(&KeyBinding::new(None, "red", PressKind::Short, "stop")).is_none());
        assert!(
            validated(&KeyBinding::new(
                Some("editor"),
                "ok",
                PressKind::Short,
                "stop"
            ))
            .is_none()
        );
        assert!(
            validated(&KeyBinding::new(
                Some("guide"),
                "back",
                PressKind::Short,
                "ignore"
            ))
            .is_none()
        );
        let binding = KeyBinding::new(Some("player"), "green", PressKind::Long, "show_controls");
        let parsed = validated(&binding).unwrap();
        assert_eq!(
            (parsed.context, parsed.key, parsed.press, parsed.action),
            (
                Some(Context::Player),
                RemoteKey::Green,
                Press::Long,
                Action::ShowControls
            )
        );
    }
    #[test]
    fn override_conflicts_are_scoped_to_context_key_and_press() {
        let editor = Editor::default();
        assert!(same(
            &editor,
            &KeyBinding::new(Some("guide"), "red", PressKind::Short, "ignore")
        ));
        for other in [
            KeyBinding::new(Some("groups"), "red", PressKind::Short, "ignore"),
            KeyBinding::new(Some("guide"), "green", PressKind::Short, "ignore"),
            KeyBinding::new(Some("guide"), "red", PressKind::Long, "ignore"),
        ] {
            assert!(!same(&editor, &other));
        }
        assert_eq!(cycle(2, &[1, 2, 3], 1), 3);
        assert_eq!(cycle(1, &[1, 2, 3], -1), 3);
        assert_eq!(cycle(3, &[1, 2, 3], 1), 1);
        assert_eq!(cycle(4, &[], 1), 4);
    }
}
