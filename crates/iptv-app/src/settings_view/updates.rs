//! Application-owned release discovery, inline notes and native installation.
use crate::i18n::tr;
use nsh_core::prelude::*;

pub(super) use crate::host::updates::RELEASES;

use crate::host::updates::view as native;

pub(super) fn label() -> &'static str {
    tr("Check for updates")
}

pub(super) fn run(ui: crate::Ui) {
    native::open(ui);
}

pub(crate) fn bind(ui: crate::Ui) {
    native::bind(ui);
}

pub(crate) fn is_open(ui: crate::Ui) -> bool {
    ui.update_dialog_open.get()
}

pub(crate) fn close(ui: crate::Ui) {
    native::close(ui);
}

pub(crate) fn key(ui: crate::Ui, action: iptv_nav::Action) {
    native::key(ui, action);
}

pub(crate) fn layer(ui: crate::Ui) -> View {
    native::layer(ui)
}

pub(crate) fn notice_open(ui: crate::Ui) -> bool {
    native::notice_open(ui)
}

pub(crate) fn dismiss_notice(ui: crate::Ui) {
    native::dismiss_notice(ui);
}

pub(crate) fn notice_key(ui: crate::Ui, action: iptv_nav::Action) {
    native::notice_key(ui, action);
}
