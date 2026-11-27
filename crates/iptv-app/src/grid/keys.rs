//! A desktop keyboard's own guide keys (`iptv_nav::Profile::Keyboard`
//! binds them; a remote never sends them): Home back to now, Shift + Left
//! and Right an hour, Ctrl (Cmd) + Left and Right a day, Tab the groups.

use iptv_nav::Action;
use iptv_presentation::guide::moves::{DAY, HOUR};

use crate::Ui;

pub(crate) fn apply(ui: Ui, action: Action) {
    let view = crate::lists::view(ui.guide);
    let step = |minutes: i32| ui.guide.focus.update(|f| f.step_time(minutes, &view));
    match action {
        Action::JumpToNow => ui.guide.focus.update(|f| f.jump_to_now()),
        Action::HourBack => step(-HOUR),
        Action::HourForward => step(HOUR),
        Action::DayBack => step(-DAY),
        Action::DayForward => step(DAY),
        Action::ToggleGroups => ui.guide.focus.update(|f| {
            f.toggle_groups(&view);
        }),
        _ => {}
    }
}
