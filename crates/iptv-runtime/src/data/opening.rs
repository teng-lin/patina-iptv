//! Shell effects for an early cached playback start.
use crate::model::StoredSource;
pub use crate::opening::Held;
pub(crate) use crate::opening::send_stored;
use iptv_epg::GuideStore;
use iptv_guide::EpgSchedule;
use std::rc::Rc;

/// Cold start with stored source `stored` (the store open): the guide's
/// empty shell, and the last channel when it can start at once.
pub async fn begin(owner: &Rc<crate::Runtime>, stored: &StoredSource) {
    let shell = EpgSchedule::new(Vec::new(), GuideStore::default(), super::now_unix());
    owner.emit(crate::RuntimeEvent::ScheduleReady(Rc::new(shell)));
    let (done, wait) = futures_channel::oneshot::channel();
    owner.emit(crate::RuntimeEvent::EarlyResume {
        source: Box::new(stored.clone()),
        done,
    });
    let _ = wait.await;
}
