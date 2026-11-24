use crate::Ui;
use nsh_core::prelude::*;
pub fn bind(ui: Ui) {
    effect(move || {
        ui.app.library.version.get();
        ui.app.data_version.get();
        crate::runtime::data::stored::refresh_status(&crate::runtime());
    });
}

pub use crate::runtime::data::stored::{clear, update_now, update_source};
pub use crate::runtime::stored::{GuideStatus, SourceLine, status_of, stored_past_days};
