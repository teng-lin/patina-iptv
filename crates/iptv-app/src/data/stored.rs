use crate::Ui;
use nsh_core::prelude::*;
pub fn bind(ui: Ui) {
    effect(move || {
        ui.library.version.get();
        ui.data_version.get();
        crate::runtime::data::stored::refresh_status(&crate::runtime());
    });
}

pub use crate::runtime::data::stored::{clear, update_now, update_source};
pub use crate::runtime::stored::{GuideStatus, SourceLine, keep_past_days, status_of};
