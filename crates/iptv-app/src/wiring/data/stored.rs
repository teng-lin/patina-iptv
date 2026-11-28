use nsh_core::prelude::*;
pub fn bind(app: crate::SharedUi) {
    effect(move || {
        app.library.version.get();
        app.data_version.get();
        iptv_runtime::data::refresh_guide_status(&crate::wiring::runtime_owner::runtime());
    });
}

pub use iptv_runtime::data::{
    clear_guide as clear, clear_source_guide as clear_source, update_guide_now as update_now,
    update_source_guide as update_source,
};
pub use iptv_runtime::model::{GuideStatus, MAX_STORED_GUIDE_PAST_DAYS, SourceLine};
