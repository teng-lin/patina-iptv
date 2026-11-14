use crate::Ui;
use nsh_core::prelude::*;
pub fn bind(ui: Ui) {
    effect(move || {
        ui.library.version.get();
        ui.data_version.get();
        crate::runtime::data::stored::refresh_status();
    });
}

pub use crate::runtime::data::stored::{clear, update_now, update_source};
pub use crate::runtime::stored::{
    GuideStatus, SourceLine, cached_guide_state, guide_status, keep_past_days, shown,
    shown_sources, source_lines, status_of, updating,
};
