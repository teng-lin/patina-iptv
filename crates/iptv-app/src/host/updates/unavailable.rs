//! Hosts without native package updates expose no update action or installer.
use std::rc::Rc;

use iptv_core::message::key;
use iptv_runtime::{Runtime, app_updates::github::ReleaseCheck};

use super::{Metadata, UpdatePrompts};

pub(crate) struct PackageAction;

impl PackageAction {
    pub(crate) fn label(&self) -> &'static str {
        key("Download package")
    }

    pub(crate) async fn run(self, _: &Rc<Runtime>, _: &impl UpdatePrompts) {}
}

pub(crate) async fn action_for(
    _: &Rc<Runtime>,
    _: Metadata,
    _: &ReleaseCheck,
) -> Result<Option<PackageAction>, &'static str> {
    Ok(None)
}

pub(crate) fn automatic_install_supported(_: Metadata) -> bool {
    false
}

pub(crate) fn can_cancel_install(_: &Runtime) -> bool {
    true
}

pub(crate) fn try_cancel_current(_: &Runtime) -> bool {
    true
}
