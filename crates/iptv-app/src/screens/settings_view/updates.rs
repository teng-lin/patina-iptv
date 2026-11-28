//! Application-owned release discovery, inline notes and native installation.
use crate::kit::i18n::tr;
use nsh_core::prelude::*;

pub(super) use crate::host::updates::RELEASES;

use crate::host::updates::AVAILABLE;

mod view;
pub(crate) use view::UpdateUi;

pub(super) fn label() -> &'static str {
    tr("Check for updates")
}

pub(super) fn run(ui: crate::Ui) {
    if AVAILABLE {
        view::open(ui);
    }
}

pub(crate) fn bind(ui: crate::Ui) {
    if AVAILABLE {
        view::bind(ui);
    }
}

pub(crate) fn is_open(screens: crate::ScreensUi) -> bool {
    screens.update_dialog_open.get()
}

pub(crate) fn close(screens: crate::ScreensUi) {
    if AVAILABLE {
        view::close(screens);
    } else {
        screens.update_dialog_open.set(false);
    }
}

pub(crate) fn key(ui: crate::Ui, action: iptv_nav::Action) {
    if AVAILABLE {
        view::key(ui, action);
    }
}

pub(crate) fn layer(ui: crate::Ui) -> View {
    if AVAILABLE {
        view::layer(ui)
    } else {
        nsh_core::prelude::view()
    }
}

pub(crate) fn notice_open(ui: crate::Ui) -> bool {
    AVAILABLE && view::notice_open(ui)
}

pub(crate) fn dismiss_notice(screens: crate::ScreensUi) {
    if AVAILABLE {
        view::dismiss_notice(screens);
    }
}

pub(crate) fn notice_key(screens: crate::ScreensUi, action: iptv_nav::Action) {
    if AVAILABLE {
        view::notice_key(screens, action);
    }
}
