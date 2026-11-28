//! Actions for the guide after playback and overlays have had their turn.
use super::{Action, Move, Pane, RemoteKey, Ui, ViewMode};

pub(super) fn apply(ui: Ui, key: RemoteKey, action: Action) {
    let mv = |m: Move| crate::guide::lists::navigate(ui, m);
    match action {
        Action::Record => crate::screens::recordings_view::current(ui),
        Action::ShowMenu if ui.guide.focus.with(|f| f.pane) == Pane::Groups => {
            let entry = ui.guide.focus.with_untracked(|f| f.group);
            let combined =
                crate::wiring::data::combined::active(&crate::wiring::runtime_owner::runtime());
            if (key == RemoteKey::Menu && !combined && entry == iptv_presentation::lists::ALL)
                || (!crate::shell::menu_view::open_group_options(ui, entry)
                    && ui.app.library.shown.get_untracked().is_none()
                    && !combined)
            {
                crate::library_ui::wizard_view::open_add(ui);
            }
        }
        Action::ShowMenu if ui.guide.focus.with(|f| f.pane) == Pane::Rail => {}
        Action::ShowMenu
            if ui.chrome.mode.get() != ViewMode::Fullscreen
                && ui.guide.focus.with(|f| f.pane) == Pane::Guide =>
        {
            crate::shell::menu_view::open_timeline_context(ui);
        }
        Action::ShowMenu => crate::shell::menu_view::open(ui),
        Action::MoveUp => mv(Move::Up),
        Action::MoveDown => mv(Move::Down),
        Action::MoveLeft => mv(Move::Left),
        Action::MoveRight => mv(Move::Right),
        Action::Select => {
            if !ui
                .guide
                .focus
                .with(|f| matches!(f.pane, Pane::Guide | Pane::Groups))
                || !crate::shell::menu_view::open_empty_group_settings(ui)
            {
                crate::guide::lists::ok(ui);
            }
        }
        Action::ShowGuide => ui.chrome.mode.apply(crate::kit::mode::Action::ShowGuide),
        Action::ShowGroups => crate::guide::lists::open_groups(ui.guide),
        Action::OpenSearch => crate::screens::search_view::open(ui),
        a @ (Action::JumpToNow
        | Action::HourBack
        | Action::HourForward
        | Action::DayBack
        | Action::DayForward
        | Action::ToggleGroups) => crate::guide::grid::keys::apply(ui.guide, a),
        Action::ReturnToPlayer if ui.playback.playing.get().is_some() => {
            ui.chrome
                .mode
                .apply(crate::kit::mode::Action::ReturnToPlayer);
        }
        Action::ToggleFavorite => {
            if let Some(channel) = if ui.chrome.mode.get() == ViewMode::Fullscreen {
                ui.playback.playing.get()
            } else {
                crate::guide::lists::focused_channel(ui.guide)
            } {
                crate::guide::favorites::toggle(ui, channel);
            }
        }
        Action::PageUp => page(ui, -1),
        Action::PageDown => page(ui, 1),
        other => log::info!("iptv action {other:?} not implemented yet"),
    }
}

/// Page up/down: moves the focus by the rows on screen.
fn page(ui: Ui, dir: isize) {
    let rows = ui
        .guide
        .visible_rows
        .get()
        .map_or(8, |(a, b)| (b - a).max(1));
    let len = ui.guide.list.with(|l| l.len());
    ui.guide.focus.update(|f| {
        f.channel = iptv_presentation::guide::page_row(f.channel, len, dir * rows as isize);
    });
}
