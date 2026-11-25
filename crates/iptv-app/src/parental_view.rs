//! Parental modal: the core owns PIN entry and transitions; the runtime owns
//! credentials and access. Callbacks run only after the authoritative gate.
use crate::i18n::{tr, trf};
use crate::input::dismiss_keyboard;
use crate::{Ui, scale::Scale, tint, ui_kit};
use iptv_core::parental::{ContentKind, Target};
use iptv_nav::{Action, RemoteKey};
use iptv_presentation::parental::{Dialog, Phase, Submission};
use nsh_core::element::input;
use nsh_core::prelude::*;
use std::{cell::RefCell, rc::Rc};

type Unlock = RefCell<Option<Box<dyn FnOnce()>>>;
type AfterUnlock = Rc<Unlock>;

#[derive(Clone)]
pub struct Open {
    dialog: Dialog,
    target: Target,
    done: AfterUnlock,
    request: Rc<()>,
    retired_verify: Option<Rc<()>>,
    blocks: Option<Vec<Target>>,
    recovery_input: ElementRef,
    locked_on_hide: bool,
}
impl PartialEq for Open {
    fn eq(&self, other: &Self) -> bool {
        self.dialog == other.dialog
            && self.target == other.target
            && self.blocks == other.blocks
            && self.locked_on_hide == other.locked_on_hide
            && Rc::ptr_eq(&self.done, &other.done)
            && Rc::ptr_eq(&self.request, &other.request)
            && match (&self.retired_verify, &other.retired_verify) {
                (Some(a), Some(b)) => Rc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
    }
}
pub fn is_open(ui: Ui) -> bool {
    ui.parental.with(Option::is_some)
}
/// Which prompt is open: equal for as long as it shows, unequal for any
/// later one (the handle keeps its allocation, so no address is reused).
#[derive(Clone)]
pub(crate) struct Prompt(std::rc::Weak<Unlock>);
impl PartialEq for Prompt {
    fn eq(&self, other: &Self) -> bool {
        self.0.ptr_eq(&other.0)
    }
}
/// The prompt open now (tracked).
pub(crate) fn prompt(ui: Ui) -> Option<Prompt> {
    ui.parental
        .with(|o| o.as_ref().map(|o| Prompt(Rc::downgrade(&o.done))))
}
pub fn close(ui: Ui) {
    // Credential changes can commit before their recovery notice returns.
    // Keep the modal until that result is shown instead of discarding it.
    if committing(ui) {
        return;
    }
    abort(ui);
}
/// Lifecycle teardown must invalidate an in-flight authorization even when
/// user dismissal is temporarily disabled during a credential write.
pub fn abort(ui: Ui) {
    crate::external_player::cancel_pending(ui);
    ui.parental.set(None);
    crate::runtime().parental_lock();
    dismiss_keyboard();
}
/// Hiding for background recordings retires verification immediately. A
/// credential write/notice keeps only its result dialog until the window returns.
pub fn hidden(ui: Ui) {
    let keep = committing(ui)
        || phase(ui) == Some(Phase::RecoveryNotice)
        || ui
            .parental
            .with_untracked(|o| o.as_ref().is_some_and(|o| o.dialog.has_held_notice()));
    if keep {
        ui.parental.update(|open| {
            if let Some(open) = open {
                open.locked_on_hide = true;
                // An in-flight verify may not have reached its runtime epoch
                // check yet. Retire its UI request but keep the private result;
                // its completion unlocks no UI and acknowledges when retry is safe.
                if open.dialog.busy
                    && open.dialog.phase == Phase::Unlock
                    && open.dialog.has_held_notice()
                    && open.retired_verify.is_none()
                {
                    open.retired_verify = Some(std::mem::replace(&mut open.request, Rc::new(())));
                }
                open.dialog.suspend_notice();
            }
        });
        crate::runtime().parental_lock();
        dismiss_keyboard();
    } else {
        abort(ui);
    }
}
fn committing(ui: Ui) -> bool {
    busy(ui) && phase(ui) == Some(Phase::Confirm)
}
fn recovery_input(ui: Ui) -> Option<ElementRef> {
    ui.parental
        .with_untracked(|open| open.as_ref().map(|open| open.recovery_input))
}
fn finish(ui: Ui) {
    let mut done = None;
    ui.parental.update(|open| {
        done = open.take().and_then(|o| o.done.borrow_mut().take());
    });
    dismiss_keyboard();
    if let Some(done) = done {
        done();
    }
}
fn show(ui: Ui, target: Target, phase: Phase, done: impl FnOnce() + 'static) {
    ui.parental.set(Some(Open {
        dialog: Dialog::new(phase),
        target,
        done: Rc::new(RefCell::new(Some(Box::new(done)))),
        request: Rc::new(()),
        retired_verify: None,
        blocks: None,
        recovery_input: element_ref(),
        locked_on_hide: false,
    }));
}
/// Always re-resolve durable ancestry before executing a content action.
pub fn request(ui: Ui, target: Target, done: impl FnOnce() + 'static) {
    crate::data::spawn_in_app(async move {
        let owner = crate::runtime();
        match owner.parental_check(&target).await {
            Ok(()) => done(),
            Err(crate::runtime::parental::Error::Locked) => {
                let phase = if owner.parental_has_pin() {
                    Phase::Unlock
                } else {
                    Phase::New
                };
                show(ui, target, phase, done);
            }
            Err(_) => notify_message("Parental settings are unavailable"),
        }
    });
}
pub fn protected_settings(ui: Ui, done: impl FnOnce() + 'static) {
    request(ui, Target::settings(), move || {
        if crate::runtime().parental_has_pin() {
            done();
        } else {
            show(ui, Target::settings(), Phase::New, done);
        }
    });
}
pub fn change_pin(ui: Ui) {
    crate::data::spawn_in_app(async move {
        let owner = crate::runtime();
        if owner.parental_refresh().await.is_err() {
            notify_message("Parental settings are unavailable");
            return;
        }
        show(
            ui,
            Target::settings(),
            if owner.parental_has_pin() {
                Phase::Old
            } else {
                Phase::New
            },
            move || crate::settings_view::refresh(ui),
        );
    });
}
pub fn live_target(ui: Ui, channel: usize) -> Option<Target> {
    let key = crate::schedule().channel_key(channel)?;
    if crate::data::combined::active(&crate::runtime()) {
        let (source, id) = crate::data::combined::owner(&crate::runtime(), &key)?;
        Some(Target::live(source.0, id))
    } else {
        Some(Target::live(ui.app.library.shown.get_untracked()?.0, key))
    }
}
/// Runs `start` once live `channel` may play: at once, or after its PIN
/// (never when the PIN is canceled). `start` still plays through
/// [`allow_play`], which then admits it.
pub fn admitted(ui: Ui, channel: usize, start: impl FnOnce() + 'static) {
    match live_target(ui, channel) {
        Some(target) if !crate::runtime().parental_allowed(&target) => request(ui, target, start),
        _ => start(),
    }
}
/// Synchronous gate is fed from a complete native membership snapshot at boot
/// and after every catalog change; the player also checks before resolution.
pub fn allow_play(ui: Ui, channel: usize, playback: crate::Playback) -> bool {
    let Some(target) = live_target(ui, channel) else {
        return true;
    };
    if crate::runtime().parental_allowed(&target) {
        if !crate::runtime().parental_is_blocked(&target) {
            crate::runtime().parental_unblocked_channel_played();
        }
        return true;
    }
    match playback {
        crate::Playback::CatchUp { start, offset } => {
            let schedule = crate::schedule();
            if let Some(pending) =
                crate::player::PendingArchive::capture(&*schedule, channel, start, offset)
            {
                request(ui, target, move || {
                    if let Some((channel, playback)) = pending.remap(&*crate::schedule()) {
                        ui.play(channel, playback);
                    } else {
                        crate::settings_view::notify(tr(
                            "Guide time changed. Choose the program again.",
                        ));
                    }
                });
            }
        }
        crate::Playback::Live => request(ui, target, move || ui.play(channel, playback)),
    }
    false
}
pub fn block(ui: Ui, target: Target, blocked: bool) {
    request(ui, Target::settings(), move || {
        if !crate::runtime().parental_has_pin() {
            show(ui, Target::settings(), Phase::New, move || {
                block(ui, target, blocked)
            });
            return;
        }
        crate::data::spawn_in_app(async move {
            let owner = crate::runtime();
            let key = if matches!(target.kind, ContentKind::Live | ContentKind::Group) {
                "parental.live_enabled"
            } else {
                "parental.vod_enabled"
            };
            if blocked
                && owner
                    .set_setting(key, iptv_settings::Value::Bool(true))
                    .is_err()
            {
                notify_message("Unlock parental settings first");
                return;
            }
            match owner.parental_set_blocked(target, blocked).await {
                Ok(()) => {
                    crate::settings_view::refresh(ui);
                    notify_message(if blocked {
                        "Content blocked"
                    } else {
                        "Content unblocked"
                    });
                    enforce(ui);
                }
                Err(_) => notify_message("Could not change parental protection"),
            }
        });
    });
}
pub fn manage(ui: Ui) {
    request(ui, Target::settings(), move || {
        let blocks = crate::runtime().parental_blocked_targets();
        ui.parental.set(Some(Open {
            dialog: Dialog::new(Phase::Unlock),
            target: Target::settings(),
            done: Rc::new(RefCell::new(None)),
            request: Rc::new(()),
            retired_verify: None,
            blocks: Some(blocks),
            recovery_input: element_ref(),
            locked_on_hide: false,
        }));
    });
}
fn manage_pick(ui: Ui, index: usize) {
    if ui.parental.with_untracked(|o| {
        o.as_ref()
            .and_then(|o| o.blocks.as_ref())
            .is_some_and(|blocks| index == blocks.len() + 1)
    }) {
        close(ui);
        return;
    }
    let target = ui
        .parental
        .with_untracked(|o| o.as_ref()?.blocks.as_ref()?.get(index).cloned());
    if let Some(target) = target {
        finish(ui);
        block(ui, target, false);
    } else {
        crate::runtime().parental_lock();
        close(ui);
        enforce(ui);
        notify_message("Parental controls locked");
    }
}
pub fn enforce(ui: Ui) {
    crate::external_player::enforce(ui);
    let denied = ui.vod.with_untracked(|s| {
        s.cards
            .iter()
            .filter(|c| c.plot.is_some() || c.poster.is_some())
            .map(|c| &c.key)
            .chain(s.preview.iter().map(|d| &d.item.key))
            .filter(|key| !crate::runtime().parental_allowed(&iptv_runtime::vod::target(key)))
            .cloned()
            .collect::<Vec<_>>()
    });
    if !denied.is_empty() {
        ui.vod.update(|s| s.redact_metadata(&denied));
    }
    let blocked_details = ui.vod.with_untracked(|s| {
        s.details.as_ref().and_then(|d| {
            let key = &d.item.key;
            (!crate::runtime().parental_allowed(&iptv_runtime::vod::target(key)))
                .then(|| key.clone())
        })
    });
    if let Some(key) = blocked_details {
        ui.vod.update(|s| s.close_blocked_details(&key));
    }

    if let Some((playing, _)) = ui.player.vod.get_untracked() {
        let owner = crate::runtime();
        if !owner.parental_allowed(&iptv_runtime::vod::target(&playing.key))
            || playing
                .parent
                .as_ref()
                .is_some_and(|parent| !owner.parental_allowed(&iptv_runtime::vod::target(parent)))
        {
            crate::player::stop(ui);
        }
    }
    if let Some((job, _)) = ui.player.recorded.get_untracked() {
        let target = Target::live(job.request.source, job.request.channel.as_str());
        if !crate::runtime().parental_allowed(&target) {
            crate::player::stop(ui);
        }
    }
    for surface in 0..crate::player::SURFACES {
        if let Some((channel, _)) = crate::player::target(ui, surface)
            && let Some(target) = live_target(ui, channel)
            && !crate::runtime().parental_allowed(&target)
        {
            crate::player::stop_surface(ui, surface);
        }
    }
}
pub fn bind(ui: Ui) {
    let _task = set_interval(std::time::Duration::from_secs(1), move || enforce(ui));
}
pub fn key(ui: Ui, action: Action) {
    match action {
        Action::Back => close(ui),
        Action::MoveUp => move_focus(ui, -vertical_step(ui)),
        Action::MoveDown => move_focus(ui, vertical_step(ui)),
        Action::MoveLeft => move_focus(ui, -1),
        Action::MoveRight => move_focus(ui, 1),
        Action::Select => {
            let at = ui
                .parental
                .with_untracked(|o| o.as_ref().map_or(0, |o| o.dialog.focus));
            activate(ui, at);
        }
        _ => {}
    }
}
pub fn remote(ui: Ui, key: RemoteKey) {
    ui.input.set(crate::Input::Keys);
    if committing(ui) && key != RemoteKey::Back {
        return;
    }
    if ui
        .parental
        .with_untracked(|o| o.as_ref().is_some_and(|o| o.blocks.is_some()))
    {
        match key {
            RemoteKey::Up => move_focus(ui, -1),
            RemoteKey::Down => move_focus(ui, 1),
            RemoteKey::Ok => {
                let at = ui
                    .parental
                    .with_untracked(|o| o.as_ref().map_or(0, |o| o.dialog.focus));
                manage_pick(ui, at);
            }
            RemoteKey::Back => close(ui),
            _ => {}
        }
        return;
    }
    match key {
        RemoteKey::Digit(d) => ui.parental.update(|o| {
            if let Some(o) = o {
                o.dialog.key_digit(d);
            }
        }),
        RemoteKey::Up => self::key(ui, Action::MoveUp),
        RemoteKey::Down => self::key(ui, Action::MoveDown),
        RemoteKey::Left => self::key(ui, Action::MoveLeft),
        RemoteKey::Right => self::key(ui, Action::MoveRight),
        RemoteKey::Ok => {
            if cancel_focused(ui) {
                close(ui);
            } else if phase(ui)
                .is_some_and(|p| matches!(p, Phase::Recovery | Phase::RecoveryNotice))
            {
                let at = ui
                    .parental
                    .with_untracked(|o| o.as_ref().map_or(0, |o| o.dialog.focus));
                activate(ui, at);
            } else if ui.parental.with_untracked(|o| {
                // One rule for every remote (#282): after digits typed on
                // number keys, OK on a digit key submits; otherwise it
                // presses the focused key (a digit, Delete, Continue,
                // Forgot PIN?), so the keypad enters any PIN length.
                o.as_ref()
                    .is_some_and(|o| matches!(o.dialog.focus, 0..=8 | 10) && o.dialog.ok_submits())
            }) {
                submit(ui);
            } else {
                let at = ui
                    .parental
                    .with_untracked(|o| o.as_ref().map_or(0, |o| o.dialog.focus));
                activate(ui, at);
            }
        }
        RemoteKey::Back => close(ui),
        _ => {}
    }
}
fn vertical_step(ui: Ui) -> isize {
    if managing(ui)
        || phase(ui).is_some_and(|p| matches!(p, Phase::Recovery | Phase::RecoveryNotice))
    {
        1
    } else {
        3
    }
}
fn move_focus(ui: Ui, step: isize) {
    ui.parental.update(|o| {
        if let Some(o) = o {
            let count = o.blocks.as_ref().map_or_else(
                || match o.dialog.phase {
                    Phase::Recovery => 3,
                    Phase::RecoveryNotice => 2,
                    _ => 14,
                },
                |b| b.len() + 2,
            );
            o.dialog.move_focus(step, count);
        }
    });
    if phase(ui) == Some(Phase::Recovery) && !focused(ui, 0) {
        if let Some(input) = recovery_input(ui) {
            input.blur();
        }
        dismiss_keyboard();
    }
}
fn activate(ui: Ui, index: usize) {
    if busy(ui) {
        if cancel_focused(ui) {
            close(ui);
        }
        return;
    }
    if ui
        .parental
        .with_untracked(|o| o.as_ref().is_some_and(|o| o.blocks.is_some()))
    {
        manage_pick(ui, index);
        return;
    }
    if phase(ui).is_some_and(|phase| matches!(phase, Phase::Recovery | Phase::RecoveryNotice)) {
        if phase(ui) == Some(Phase::Recovery) && index == 0 {
            if let Some(input) = recovery_input(ui) {
                input.focus();
            }
        } else if cancel_focused(ui) {
            close(ui);
        } else {
            submit(ui);
        }
        return;
    }
    match index {
        0..=8 => ui.parental.update(|o| {
            if let Some(o) = o {
                o.dialog.digit(index as u8 + 1);
            }
        }),
        9 => ui.parental.update(|o| {
            if let Some(o) = o {
                o.dialog.erase();
            }
        }),
        10 => ui.parental.update(|o| {
            if let Some(o) = o {
                o.dialog.digit(0);
            }
        }),
        11 => submit(ui),
        12 => ui.parental.update(|o| {
            if let Some(o) = o {
                o.dialog.recover();
            }
        }),
        _ => close(ui),
    }
}
fn submit(ui: Ui) {
    let mut submission = Submission::None;
    let mut target = Target::settings();
    let token = ui
        .parental
        .with_untracked(|o| o.as_ref().map(|o| o.request.clone()));
    ui.parental.update(|o| {
        if let Some(o) = o {
            target = o.target.clone();
            submission = o.dialog.submit();
        }
    });
    match submission {
        Submission::None => {}
        Submission::Done => finish(ui),
        submission => crate::data::spawn_in_app(async move {
            let owner = crate::runtime();
            let result = match submission {
                Submission::Verify(pin) => {
                    owner.parental_authorize(target, pin).await.map(|()| None)
                }
                Submission::Change { old, new } => {
                    owner.parental_change_pin(old, new).await.map(Some)
                }
                Submission::Recover { code, new } => {
                    owner.parental_recover_pin(code, new).await.map(Some)
                }
                _ => return,
            };
            if !ui.parental.with_untracked(|o| {
                o.as_ref()
                    .zip(token.as_ref())
                    .is_some_and(|(o, token)| Rc::ptr_eq(&o.request, token))
            }) {
                owner.parental_lock();
                ui.parental.update(|o| {
                    if let Some(o) = o
                        && o.retired_verify
                            .as_ref()
                            .zip(token.as_ref())
                            .is_some_and(|(retired, token)| Rc::ptr_eq(retired, token))
                    {
                        o.retired_verify = None;
                        o.dialog.fail(String::new());
                    }
                });
                return;
            }
            let locked = ui
                .parental
                .with_untracked(|o| o.as_ref().is_some_and(|o| o.locked_on_hide));
            match result {
                Ok(None) => {
                    let mut resumed = false;
                    ui.parental.update(|o| {
                        if let Some(o) = o {
                            resumed = o.dialog.resume_notice();
                            if resumed {
                                o.locked_on_hide = false;
                            }
                        }
                    });
                    if !resumed {
                        finish(ui);
                    }
                }
                Ok(Some(code)) => {
                    // Change/recovery temporarily grants administration in the
                    // runtime; revoke it in this same poll before UI publication.
                    if locked {
                        owner.parental_lock();
                    }
                    ui.parental.update(|o| {
                        if let Some(o) = o {
                            if locked {
                                o.dialog.hold_notice(code);
                            } else {
                                o.dialog.notice(code);
                            }
                        }
                    });
                }
                Err(error) => ui.parental.update(|o| {
                    if let Some(o) = o {
                        o.dialog.fail(error.to_string());
                    }
                }),
            }
        }),
    }
}
fn phase(ui: Ui) -> Option<Phase> {
    ui.parental.with(|o| o.as_ref().map(|o| o.dialog.phase))
}
fn label(ui: Ui) -> String {
    ui.parental.with(|o| {
        o.as_ref().map_or(String::new(), |o| {
            if o.blocks.is_some() {
                return "Blocked content".into();
            }
            match o.dialog.phase {
                Phase::Unlock => "Enter parental PIN",
                Phase::Old => "Enter current PIN",
                Phase::New => "Choose a new PIN",
                Phase::Confirm => "Confirm new PIN",
                Phase::Recovery => "Recover parental PIN",
                Phase::RecoveryNotice => "Save your recovery code",
            }
            .into()
        })
    })
}
fn display_target(target: &Target) -> String {
    let name = crate::runtime()
        .parental_name(target)
        .unwrap_or_else(|| tr("Unavailable content").into());
    trf("Unblock {name}", &[("name", name.as_str().into())])
}
fn notify_message(message: &str) {
    crate::settings_view::notify(tr(message));
}
fn focused(ui: Ui, index: usize) -> bool {
    ui.parental
        .with(|o| o.as_ref().is_some_and(|o| o.dialog.focus == index))
}
fn cancel_focused(ui: Ui) -> bool {
    ui.parental.with(|o| {
        o.as_ref().is_some_and(|o| {
            let cancel = o.blocks.as_ref().map_or_else(
                || match o.dialog.phase {
                    Phase::Recovery => 2,
                    Phase::RecoveryNotice => 1,
                    _ => 13,
                },
                |b| b.len() + 1,
            );
            o.dialog.focus == cancel
        })
    })
}
fn managing(ui: Ui) -> bool {
    ui.parental
        .with(|o| o.as_ref().is_some_and(|o| o.blocks.is_some()))
}
fn manager(ui: Ui, size: Scale) -> View {
    let count = move || {
        ui.parental.with(|o| {
            o.as_ref()
                .and_then(|o| o.blocks.as_ref())
                .map_or(1, |b| b.len() + 1)
        })
    };
    let list = element_ref();
    effect(move || {
        if let Some(focus) = ui.parental.with(|o| o.as_ref().map(|o| o.dialog.focus)) {
            list.scroll_to_index(focus as u32, percentage(0.5), px(0.0), false);
        }
    });
    view().child(
        virtual_list(count, move |at: Position| {
            let label = move || {
                ui.parental.with(|o| {
                    o.as_ref()
                        .and_then(|o| o.blocks.as_ref())
                        .and_then(|blocks| blocks.get(at.index()))
                        .map_or_else(|| crate::i18n::tr("Lock now").to_owned(), display_target)
                })
            };
            ui_kit::button(
                ui,
                size,
                label,
                move || focused(ui, at.index()),
                move || manage_pick(ui, at.index()),
            )
        })
        .element_ref(&list)
        .estimated_item_size(move || size.whole(52.0))
        .h(move || size.px(count().clamp(1, 6) as f32 * 52.0)),
    )
}
fn busy(ui: Ui) -> bool {
    ui.parental
        .with(|o| o.as_ref().is_some_and(|o| o.dialog.busy))
}
fn keypad(ui: Ui, size: Scale, scrolling: ui_kit::FocusScroll) -> View {
    let mut keypad = view().gap(px(6.0)).child(ui_kit::title(size, move || {
        ui.parental.with(|o| {
            o.as_ref()
                .map_or(String::new(), |o| o.dialog.masked_input())
        })
    }));
    for row in 0..4 {
        let mut cells = view().flex_row().gap(px(6.0));
        for column in 0..3 {
            let index = row * 3 + column;
            let text = match index {
                0..=8 => (index + 1).to_string(),
                9 => crate::i18n::tr("Delete").into(),
                10 => "0".into(),
                _ => crate::i18n::tr("Continue").into(),
            };
            cells = cells.child(
                ui_kit::button(
                    ui,
                    size,
                    text,
                    move || focused(ui, index),
                    move || activate(ui, index),
                )
                .disabled(move || busy(ui))
                .test_id(if index == 11 {
                    "parental-continue"
                } else {
                    "parental-key"
                })
                .flex_1(),
            );
        }
        keypad = keypad.child(scrolling.control(cells, move || {
            (row * 3..row * 3 + 3).any(|index| focused(ui, index))
        }));
    }
    keypad.child(
        scrolling.control(
            ui_kit::button(
                ui,
                size,
                crate::i18n::tr("Forgot PIN?"),
                move || focused(ui, 12),
                move || activate(ui, 12),
            )
            .disabled(move || busy(ui)),
            move || focused(ui, 12),
        ),
    )
}
fn credentials(ui: Ui, size: Scale, scrolling: ui_kit::FocusScroll) -> View {
    use crate::i18n::tr;
    let recovery = recovery_input(ui).unwrap_or_else(element_ref);
    view().gap(px(8.0))
        .child(ui_kit::body(size,move||ui.parental.with(|o|o.as_ref()
            .map_or(String::new(),|o|if o.dialog.busy {tr("Checking…").into()}
                else {let message=o.dialog.message.as_str();tr(message).into()}))))
        .child(when(move||phase(ui)==Some(Phase::RecoveryNotice),move||{
            view().gap(px(8.0))
                .child(ui_kit::body(size,tr("Keep this recovery code private. A new code replaces the previous code.")))
                .child(ui_kit::body(size,move||ui.parental.with(|o|o.as_ref()
                    .map_or(String::new(),|o|o.dialog.recovery.to_string()))))
                .child(scrolling.control(ui_kit::button(ui,size,tr("I saved the code"),move||focused(ui,0),move||finish(ui)),move||focused(ui,0)))
        }))
        .child(when(move||phase(ui)==Some(Phase::Recovery),move||{
            view().gap(px(8.0)).child(scrolling.control(view().child(ui_kit::field(size,input())
                .test_id("parental-recovery-input").element_ref(&recovery).masked(true).max_length(32)
                .on_focus(move||ui.parental.update(|o|{if let Some(o)=o{o.dialog.focus=0;}}))
                .label(move || tr("Recovery code").to_owned())
                .border(px(1.0)).border_color(move||ui_kit::field_border(ui,focused(ui,0)))
                .disabled(move || busy(ui))
                .auto_correct(false).auto_capitalize(AutoCapitalize::None)
                .placeholder(tr("Recovery code"))
                .on_change(move|text:&str|ui.parental.update(|o|{
                    if let Some(o)=o {o.dialog.recovery.clear();
                        o.dialog.recovery.push_str(&text.to_ascii_lowercase());}
                })).on_submit(move|_:&str|{submit(ui);dismiss_keyboard();})),move||focused(ui,0)))
                .child(scrolling.control(ui_kit::button(ui,size,tr("Continue"),move||focused(ui,1),move||submit(ui))
                    .disabled(move||busy(ui)),move||focused(ui,1)))
        }))
        .child(when(move||phase(ui).is_some_and(|p|
            !matches!(p,Phase::Recovery|Phase::RecoveryNotice)),move||keypad(ui,size,scrolling)))
}
pub fn layer(ui: Ui) -> View {
    let size = Scale::new();
    view()
        .test_id("parental-layer")
        .absolute()
        .inset_0()
        .items_center()
        .justify_center()
        .bg(tint(0, 0xD8))
        .opacity(move || if is_open(ui) { 1.0_f32 } else { 0.0 })
        .pointer_events(move || {
            if is_open(ui) {
                PointerEvents::Auto
            } else {
                PointerEvents::None
            }
        })
        .accessibility_elements_hidden(move || !is_open(ui))
        .child(when(
            move || is_open(ui),
            move || {
                let scrolling = ui_kit::FocusScroll::new();
                let current_phase = computed(move || phase(ui));
                effect(move || {
                    current_phase.get();
                    scrolling.reset();
                });
                view()
                    .test_id("parental-card")
                    .w(move || size.px(440.0))
                    .max_w(percentage(0.95))
                    .max_h(percentage(0.95))
                    .h(move || {
                        px((window().height.0 * 0.95)
                            .min(size.of(540.0) * crate::settings::text_factor().max(1.0)))
                    })
                    .p(move || size.px(16.0))
                    .gap(px(8.0))
                    .rounded(px(crate::theme::RADIUS_LARGE))
                    .bg(move || tint(ui.app.theme.with(|t| t.background.panel), 255))
                    .child(ui_kit::heading(size, move || {
                        let title = label(ui);
                        crate::i18n::tr(&title).to_owned()
                    }))
                    .child(when(move || managing(ui), move || manager(ui, size)))
                    .child(when(
                        move || !managing(ui),
                        move || {
                            scrolling
                                .view()
                                .test_id("parental-scroll")
                                .flex_1()
                                .child(credentials(ui, size, scrolling))
                        },
                    ))
                    .child(
                        ui_kit::button(
                            ui,
                            size,
                            crate::i18n::tr("Cancel"),
                            move || cancel_focused(ui),
                            move || close(ui),
                        )
                        .test_id("parental-cancel")
                        .disabled(move || committing(ui))
                        .flex_shrink_0(),
                    )
            },
        ))
}
