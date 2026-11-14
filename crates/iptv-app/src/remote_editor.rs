//! Live, validated remote overrides. Editor/menu input stays at defaults.
use crate::i18n::{tr, trf};

use crate::scale::Scale;
use crate::settings_view::remote_keys::{KEYBOARD_KEYS, KEYS, action_name, context_name, key_name};
use crate::{Ui, settings, ui_kit};
use iptv_nav::remap::{self, Confirm, Editor, FIELDS, Field};
use iptv_nav::{Action, Binding, Context, Keymap, Press, RemoteKey};
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
    let mut map = Keymap::for_profile(crate::input::PROFILE);
    for b in saved().iter().filter_map(validated) {
        map.set_override(b);
    }
    map
}
pub fn overridden(context: Context, key: RemoteKey) -> bool {
    saved()
        .iter()
        .filter_map(validated)
        .any(|b| b.context == Some(context) && b.key == key)
}
pub fn open(ui: Ui) {
    ui.remote_editor.set(Some(Editor::default()));
}
pub fn is_open(ui: Ui) -> bool {
    ui.remote_editor.with(Option::is_some)
}
pub fn captures(ui: Ui) -> bool {
    ui.remote_editor
        .with_untracked(|s| s.as_ref().is_some_and(|s| s.capturing))
}
fn state<R>(ui: Ui, f: impl FnOnce(&Editor) -> R) -> Option<R> {
    ui.remote_editor.with(|s| s.as_ref().map(f))
}
fn update(ui: Ui, f: impl FnOnce(&mut Editor)) {
    ui.remote_editor.update(|s| {
        if let Some(s) = s {
            f(s)
        }
    })
}
pub fn back(ui: Ui) -> bool {
    let mut close = false;
    update(ui, |s| close = !s.back());
    if close {
        ui.remote_editor.set(None);
    }
    true
}
fn close(ui: Ui) {
    ui.remote_editor.set(None);
}
pub fn capture(ui: Ui, key: RemoteKey) {
    if key == RemoteKey::Back {
        return;
    } // Platform Back cancels capture once.
    update(ui, |s| {
        s.capturing = false;
        if key != RemoteKey::Back {
            s.key = key;
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
            let keys: Vec<_> = KEYS
                .iter()
                .chain(KEYBOARD_KEYS.iter())
                .copied()
                .chain((0..=9).map(RemoteKey::Digit))
                .collect();
            s.key = cycle(s.key, &keys, by);
        }
        Field::Press => {
            s.press = if s.press == Press::Short {
                Press::Long
            } else {
                Press::Short
            }
        }
        Field::Action => s.action = cycle(s.action, &s.actions(), by),
        _ => {}
    });
}
fn same(s: &Editor, b: &KeyBinding) -> bool {
    b.context.as_deref() == Some(s.context.token())
        && b.key == s.key.name()
        && (b.press == PressKind::Long) == (s.press == Press::Long)
}
fn store(ui: Ui, bindings: Vec<KeyBinding>) {
    if let Err(e) = settings::set(ui, keys::KEY_OVERRIDES.0, Value::KeyBindings(bindings)) {
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
    store(ui, bindings);
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
                Confirm::Reset => store(ui, Vec::new()),
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
            store(ui, b);
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
        Action::MoveUp => update(ui, |s| s.step(-1)),
        Action::MoveDown => update(ui, |s| s.step(1)),
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
const CAPTION: &str = crate::i18n::key(
    "Up/Down chooses a row; Left/Right or tap changes it. \
     Save applies immediately. Back remains available.",
);

/// What the chosen key does now and whether that is the user's binding;
/// while a key is being captured, what the editor waits for.
fn status(ui: Ui) -> String {
    ui.settings.get();
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

pub fn layer(ui: Ui) -> impl IntoElement {
    let size = Scale::new();
    when(
        move || is_open(ui),
        move || {
            ui_kit::screen(ui, size)
                .test_id("remote-editor-screen")
                .child(ui_kit::header(ui, size, tr("Remote bindings"), close))
                .child(ui_kit::caption(size, tr(CAPTION)))
                .child(ui_kit::body(size, move || status(ui)))
                .children(
                    FIELDS
                        .iter()
                        .copied()
                        .map(move |field| field_row(ui, size, field)),
                )
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
