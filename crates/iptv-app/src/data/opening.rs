//! Shell effects for an early cached playback start.
use crate::Ui;
use crate::runtime::model::StoredSource;
pub use crate::runtime::opening::*;
use iptv_epg::GuideStore;
use iptv_guide::EpgSchedule;
use std::rc::Rc;

/// Cold start with stored source `stored` (the store open): the guide's
/// empty shell, and the last channel when it can start at once.
pub async fn begin(_ui: Ui, stored: &StoredSource) {
    let shell = EpgSchedule::new(Vec::new(), GuideStore::default(), super::now_unix());
    crate::runtime().emit(crate::runtime::RuntimeEvent::ScheduleReady(Rc::new(shell)));
    let (done, wait) = futures_channel::oneshot::channel();
    crate::runtime().emit(crate::runtime::RuntimeEvent::EarlyResume {
        source: Box::new(stored.clone()),
        done,
    });
    let _ = wait.await;
}
