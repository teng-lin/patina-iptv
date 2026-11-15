//! Parental modal: the core owns PIN entry and transitions; the runtime owns
//! credentials and access. Callbacks run only after the authoritative gate.
use crate::{Ui, scale::Scale, tint, ui_kit};
use iptv_core::parental::{ContentKind, Dialog, Phase, Submission, Target};
use iptv_nav::{Action, RemoteKey};
use nsh_core::element::input;
use nsh_core::prelude::*;
use std::{cell::RefCell, rc::Rc};

#[derive(Clone)]
pub struct Open {
    dialog: Dialog,
    target: Target,
    done: Rc<RefCell<Option<Box<dyn FnOnce()>>>>,
    blocks: Option<Vec<Target>>,
}
impl PartialEq for Open {
    fn eq(&self, other: &Self) -> bool {
        self.dialog == other.dialog
            && self.target == other.target
            && self.blocks == other.blocks
            && Rc::ptr_eq(&self.done, &other.done)
    }
}
pub fn is_open(ui: Ui) -> bool {
    ui.parental.with(Option::is_some)
}
pub fn close(ui: Ui) {
    ui.parental.set(None);
    crate::runtime().parental_lock();
    dismiss_keyboard();
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
        blocks: None,
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
            Err(_) => crate::settings_view::notify("Parental settings are unavailable"),
        }
    });
}
pub fn change_pin(ui: Ui) {
    crate::data::spawn_in_app(async move {
        let owner = crate::runtime();
        if owner.parental_refresh().await.is_err() {
            crate::settings_view::notify("Parental settings are unavailable");
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
        Some(Target::live(ui.library.shown.get_untracked()?.0, key))
    }
}
/// Synchronous gate is fed from a complete native membership snapshot at boot
/// and after every catalogue change; the player also checks before resolution.
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
    request(ui, target, move || ui.play(channel, playback));
    false
}
pub fn block_channel(ui: Ui, channel: usize) {
    if let Some(target) = live_target(ui, channel) {
        block(ui, target, true);
    }
}
pub fn block_group(ui: Ui) {
    let group = ui
        .focus
        .with_untracked(|f| f.group.saturating_sub(iptv_core::lists::FIRST_GROUP));
    if let Some(source) = ui.library.shown.get_untracked() {
        let id = crate::schedule().group_key(group);
        if !id.is_empty() {
            block(
                ui,
                Target {
                    source: source.0,
                    kind: ContentKind::Group,
                    id,
                },
                true,
            );
        }
    }
}
fn block(ui: Ui, target: Target, blocked: bool) {
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
                crate::settings_view::notify("Unlock parental settings first");
                return;
            }
            match owner.parental_set_blocked(target, blocked).await {
                Ok(()) => {
                    crate::settings_view::refresh(ui);
                    crate::settings_view::notify(if blocked {
                        "Content blocked"
                    } else {
                        "Content unblocked"
                    });
                    enforce(ui);
                }
                Err(_) => crate::settings_view::notify("Could not change parental protection"),
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
            blocks: Some(blocks),
        }));
    });
}
fn manage_pick(ui: Ui, index: usize) {
    let target = ui
        .parental
        .with_untracked(|o| o.as_ref()?.blocks.as_ref()?.get(index).cloned());
    if let Some(target) = target {
        close(ui);
        block(ui, target, false);
    } else {
        crate::runtime().parental_lock();
        close(ui);
        enforce(ui);
        crate::settings_view::notify("Parental controls locked");
    }
}
pub fn enforce(ui: Ui) {
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
        Action::MoveUp => move_focus(ui, -3),
        Action::MoveDown => move_focus(ui, 3),
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
                o.dialog.digit(d);
            }
        }),
        RemoteKey::Up => self::key(ui, Action::MoveUp),
        RemoteKey::Down => self::key(ui, Action::MoveDown),
        RemoteKey::Left => self::key(ui, Action::MoveLeft),
        RemoteKey::Right => self::key(ui, Action::MoveRight),
        RemoteKey::Ok => {
            if phase(ui).is_some_and(|p| matches!(p, Phase::Recovery | Phase::RecoveryNotice))
                || crate::settings::choice(iptv_settings::keys::EnumKey("parental.pin_input"))
                    == "number_keys"
            {
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
fn move_focus(ui: Ui, step: isize) {
    ui.parental.update(|o| {
        if let Some(o) = o {
            let count = o.blocks.as_ref().map_or(15, |b| b.len() + 1);
            o.dialog.move_focus(step, count);
        }
    });
}
fn activate(ui: Ui, index: usize) {
    if ui
        .parental
        .with_untracked(|o| o.as_ref().is_some_and(|o| o.blocks.is_some()))
    {
        manage_pick(ui, index);
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
                o.dialog.input.erase();
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
        .with_untracked(|o| o.as_ref().map(|o| o.done.clone()));
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
                    .is_some_and(|(o, token)| Rc::ptr_eq(&o.done, token))
            }) {
                owner.parental_lock();
                return;
            }
            match result {
                Ok(None) => finish(ui),
                Ok(Some(code)) => ui.parental.update(|o| {
                    if let Some(o) = o {
                        o.dialog.notice(code);
                    }
                }),
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
    let owner = crate::runtime();
    let schedule = crate::schedule();
    for ch in 0..schedule.channel_count() {
        if let Some(key) = schedule.channel_key(ch) {
            let found = if crate::data::combined::active(&owner) {
                crate::data::combined::owner(&owner, &key).map(|(s, id)| Target::live(s.0, id))
            } else {
                owner.shown_source().map(|s| Target::live(s.0, key))
            };
            if found.as_ref() == Some(target) {
                return format!("Unblock {}", schedule.channel_name(ch));
            }
        }
    }
    format!("Unblock {:?} · {}", target.kind, target.id)
}
pub fn layer(ui: Ui) -> View {
    let size = Scale::new();
    view().test_id("parental-layer").absolute().inset_0().items_center().justify_center().bg(tint(0,0xD8)).opacity(move||if is_open(ui){1.0_f32}else{0.0}).pointer_events(move||if is_open(ui){PointerEvents::Auto}else{PointerEvents::None}).accessibility_elements_hidden(move||!is_open(ui)).child(
    view().w(move||size.px(440.0)).p(px(24.0)).gap(px(10.0)).rounded(px(crate::theme::RADIUS_LARGE)).bg(move||tint(ui.theme.with(|t|t.background.panel),255))
    .child(ui_kit::heading(size,move||label(ui)))
    .child(when(move||ui.parental.with(|o|o.as_ref().is_some_and(|o|o.blocks.is_some())),move||{
        let blocks=ui.parental.with_untracked(|o|o.as_ref().and_then(|o|o.blocks.clone()).unwrap_or_default());let mut rows=view().gap(px(4.0));for (i,target) in blocks.iter().enumerate(){let label=display_target(target);rows=rows.child(ui_kit::button(ui,size,label,move||ui.parental.with(|o|o.as_ref().is_some_and(|o|o.dialog.focus==i)),move||manage_pick(ui,i)));}let i=blocks.len();rows.child(ui_kit::button(ui,size,"Lock now",move||ui.parental.with(|o|o.as_ref().is_some_and(|o|o.dialog.focus==i)),move||manage_pick(ui,i)))
    }))
    .child(when(move||ui.parental.with(|o|o.as_ref().is_some_and(|o|o.blocks.is_none())),move||view().gap(px(8.0))
        .child(ui_kit::body(size,move||ui.parental.with(|o|o.as_ref().map_or(String::new(),|o|if o.dialog.busy{"Checking…".into()}else{o.dialog.message.clone()}))))
        .child(when(move||phase(ui)==Some(Phase::RecoveryNotice),move||view().gap(px(8.0)).child(ui_kit::body(size,"Keep this code somewhere private. It replaces a forgotten PIN. A new code replaces the previous code." )).child(ui_kit::body(size,move||ui.parental.with(|o|o.as_ref().map_or(String::new(),|o|o.dialog.recovery.to_string())))).child(ui_kit::button(ui,size,"I saved the code",||true,move||finish(ui)))))
        .child(when(move||phase(ui)==Some(Phase::Recovery),move||view().gap(px(8.0)).child(ui_kit::field(size,input()).test_id("parental-recovery-input").masked(true).max_length(32).auto_correct(false).auto_capitalize(AutoCapitalize::None).placeholder("Recovery code").on_change(move|text:&str|ui.parental.update(|o|{if let Some(o)=o{o.dialog.recovery.clear();o.dialog.recovery.push_str(&text.to_ascii_lowercase());}})).on_submit(move|_:&str|submit(ui))).child(ui_kit::button(ui,size,"Continue",||true,move||submit(ui)))))
        .child(when(move||phase(ui).is_some_and(|p|!matches!(p,Phase::Recovery|Phase::RecoveryNotice)),move||{
            let mut keypad=view().gap(px(6.0)).child(ui_kit::title(size,move||ui.parental.with(|o|o.as_ref().map_or(String::new(),|o|o.dialog.input.masked()))));
            for row in 0..4{let mut cells=view().flex_row().gap(px(6.0));for column in 0..3{let index=row*3+column;let text=match index{0..=8=>(index+1).to_string(),9=>"Delete".into(),10=>"0".into(),_=>"Continue".into()};cells=cells.child(ui_kit::button(ui,size,text,move||ui.parental.with(|o|o.as_ref().is_some_and(|o|o.dialog.focus==index)),move||activate(ui,index)).flex_1());}keypad=keypad.child(cells);}keypad.child(ui_kit::button(ui,size,"Forgot PIN?",move||ui.parental.with(|o|o.as_ref().is_some_and(|o|o.dialog.focus==12)),move||activate(ui,12)))
        }))
    ))
    .child(ui_kit::button(ui,size,"Cancel",||false,move||close(ui)))
)
}
