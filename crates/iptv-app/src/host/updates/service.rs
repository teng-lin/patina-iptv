//! The update operations shared by the settings panel and its host.
use std::{io, rc::Rc};

use iptv_runtime::Runtime;

use super::{Frequency, Metadata, Snapshot, Trigger};

#[cfg(any(
    target_os = "android",
    target_os = "linux",
    target_os = "windows",
    target_os = "macos"
))]
mod platform {
    use super::*;
    use std::future::Future;

    pub(crate) struct Service {
        owner: Rc<Runtime>,
        metadata: Metadata,
    }

    impl Service {
        pub(crate) fn new(owner: Rc<Runtime>, metadata: Metadata) -> Self {
            Self { owner, metadata }
        }

        pub(crate) async fn restore(&self) -> io::Result<()> {
            self.owner.restore_app_update_state().await
        }

        pub(crate) async fn persist(&self) -> io::Result<()> {
            self.owner.persist_app_update_state().await
        }

        pub(crate) fn snapshot(&self) -> Snapshot {
            self.owner.app_update_snapshot()
        }

        pub(crate) fn queue_manual_check(&self) {
            self.owner.queue_manual_app_update_check();
        }

        pub(crate) fn manual_check_pending(&self) -> bool {
            self.owner.app_update_manual_check_pending()
        }

        pub(crate) fn begin_check(
            &self,
            trigger: Trigger,
            frequency: Frequency,
        ) -> Option<impl Future<Output = ()> + use<>> {
            let token = self
                .owner
                .begin_app_update_check(now(), trigger, frequency)?;
            let owner = self.owner.clone();
            let installed = super::super::package::installed(self.metadata);
            Some(async move {
                let result = owner.check_github_release(installed).await;
                owner.finish_app_update_check(token, now(), result);
            })
        }

        pub(crate) fn skip(&self, version: &str) {
            self.owner.skip_app_update_version(version);
        }

        pub(crate) fn acknowledge(&self) {
            self.owner.acknowledge_app_update_notice();
        }

        pub(crate) fn pending_url(&self) -> Option<String> {
            self.owner
                .pending_app_update_asset()
                .map(|asset| asset.url().to_owned())
        }

        pub(crate) fn progress(&self) -> Option<(u64, u64)> {
            self.owner.app_update_download_progress()
        }

        pub(crate) fn cancel_download(&self) {
            self.owner.cancel_app_update_download();
            self.owner.cancel_pending_app_update();
        }
    }

    fn now() -> i64 {
        crate::host::since_epoch().map_or(0, |d| d.as_secs().min(i64::MAX as u64) as i64)
    }
}

#[cfg(not(any(
    target_os = "android",
    target_os = "linux",
    target_os = "windows",
    target_os = "macos"
)))]
mod platform {
    use super::*;

    pub(crate) struct Service;

    impl Service {
        pub(crate) fn new(_: Rc<Runtime>, _: Metadata) -> Self {
            Self
        }

        pub(crate) async fn restore(&self) -> io::Result<()> {
            Ok(())
        }

        pub(crate) async fn persist(&self) -> io::Result<()> {
            Ok(())
        }

        pub(crate) fn snapshot(&self) -> Snapshot {
            Snapshot::default()
        }

        pub(crate) fn queue_manual_check(&self) {}

        pub(crate) fn manual_check_pending(&self) -> bool {
            false
        }

        pub(crate) fn begin_check(
            &self,
            _: Trigger,
            _: Frequency,
        ) -> Option<std::future::Ready<()>> {
            None
        }

        pub(crate) fn skip(&self, _: &str) {}
        pub(crate) fn acknowledge(&self) {}

        pub(crate) fn pending_url(&self) -> Option<String> {
            None
        }

        pub(crate) fn progress(&self) -> Option<(u64, u64)> {
            None
        }

        pub(crate) fn cancel_download(&self) {}
    }
}

pub(crate) use platform::Service;
