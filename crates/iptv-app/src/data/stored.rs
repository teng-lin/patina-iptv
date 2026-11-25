use crate::Ui;
use nsh_core::prelude::*;
pub fn bind(ui: Ui) {
    effect(move || {
        ui.app.library.version.get();
        ui.app.data_version.get();
        crate::runtime::data::refresh_guide_status(&crate::runtime_owner::runtime());
    });
}

pub use crate::runtime::data::{
    clear_guide as clear, clear_source_guide as clear_source, update_guide_now as update_now,
    update_source_guide as update_source,
};
pub use crate::runtime::stored::{GuideStatus, SourceLine, status_of, stored_past_days};
