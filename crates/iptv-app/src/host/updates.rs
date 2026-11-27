//! Platform release checks and installer handoffs.
use std::{future::Future, pin::Pin};

use iptv_core::{capabilities::Platform, message::key};

pub(crate) use iptv_runtime::app_updates::{
    github::{CheckError, ReleaseStatus},
    state::{Frequency, Snapshot, Trigger},
};

mod service;
pub(crate) use service::Service;

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

#[cfg(any(
    target_os = "android",
    target_os = "linux",
    target_os = "windows",
    target_os = "macos"
))]
mod package;
#[cfg(not(any(
    target_os = "android",
    target_os = "linux",
    target_os = "windows",
    target_os = "macos"
)))]
#[path = "updates/unavailable.rs"]
mod package;
pub(crate) use package::{
    action_for, automatic_install_supported, can_cancel_install, try_cancel_current,
};

pub(crate) const AVAILABLE: bool = super::ANDROID || super::LINUX || super::MACOS || super::WINDOWS;
pub(crate) const RELEASES: &str = "https://github.com/teng-lin/patina-iptv/releases";

#[derive(Clone, Copy)]
pub(crate) struct Metadata {
    pub platform: Platform,
    pub direct_apk: Option<bool>,
    pub android_build: Option<(u64, u64)>,
}

impl Metadata {
    /// Android distinguishes distribution channels only after the activity reports them.
    pub(crate) fn ready(self) -> bool {
        self.platform != Platform::Android
            || match self.direct_apk {
                Some(false) => true,
                Some(true) => self.android_build.is_some(),
                None => false,
            }
    }
}

/// UI effects a native installer may request while its originating owner lives.
pub(crate) trait UpdatePrompts {
    fn notify(&self, key: &'static str);
    fn translate(&self, key: &'static str) -> &'static str;
    #[allow(dead_code)] // Used during native package handoff.
    fn is_open(&self) -> bool;
    fn is_live(&self) -> bool;
    fn cancelled(&self) -> bool;
    fn wait_cancelled(&self) -> Pin<Box<dyn Future<Output = ()> + '_>>;
    fn open_url<'a>(&'a self, url: &'a str) -> Pin<Box<dyn Future<Output = ()> + 'a>>;
    #[allow(dead_code)] // Windows installer handoff.
    fn close_after_shutdown(&self);
    #[allow(dead_code)] // macOS relaunch approval.
    fn flush_bookmarks(&self);
}

pub(crate) async fn open_download_url(url: &str, prompts: &impl UpdatePrompts) {
    if !nsh_core::prelude::open_url(url).await {
        let _ = nsh_base::alert_dialog()
            .title(prompts.translate(key("Release notes and downloads")))
            .description(url.to_owned())
            .close_on_escape(true)
            .cancel("close", prompts.translate(key("Close")))
            .show()
            .await;
    }
}

pub(crate) async fn until_cancelled<T>(
    operation: impl Future<Output = T>,
    cancellation: impl Future<Output = ()>,
    cancelled: impl Fn() -> bool,
) -> Option<T> {
    let mut operation = std::pin::pin!(operation);
    let mut cancellation = std::pin::pin!(cancellation);
    std::future::poll_fn(move |context| {
        // Check the flag even if the authorization wakes before the timer does.
        if cancelled() || cancellation.as_mut().poll(context).is_ready() {
            return std::task::Poll::Ready(None);
        }
        operation.as_mut().poll(context).map(Some)
    })
    .await
}

pub(crate) async fn until_update_cancelled<T>(
    prompts: &impl UpdatePrompts,
    operation: impl Future<Output = T>,
) -> Option<T> {
    until_cancelled(operation, prompts.wait_cancelled(), || {
        !prompts.is_live() || prompts.cancelled()
    })
    .await
}

// Native update tests can exercise the actual runtime methods. The other
// updater view tests use only the portable model and reactive handles.
#[cfg(test)]
macro_rules! native_update_tests {
    ($($item:item)*) => {
        $(
            #[cfg(any(target_os = "android", target_os = "linux", target_os = "windows", target_os = "macos"))]
            $item
        )*
    };
}

#[cfg(test)]
pub(crate) use native_update_tests;
