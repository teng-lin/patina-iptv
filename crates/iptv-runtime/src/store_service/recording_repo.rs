//! Typed durable recording operation repository.

use super::{Services, StoreError};
use iptv_core::recording::{Job, JobId, Status};
use iptv_store_sqlite::RecordingCompletion;

pub(crate) struct RecordingRepo {
    pub(super) services: Services,
}

impl RecordingRepo {
    pub(crate) async fn transition(
        self,
        id: JobId,
        from: Status,
        to: Status,
        bytes: u64,
        message: &'static str,
    ) -> Result<bool, StoreError> {
        self.services
            .call(move |store| store.transition_recording(id, from, to, bytes, message))
            .await
    }

    #[cfg(all(test, feature = "native"))]
    pub(crate) async fn job(self, id: JobId) -> Result<Option<Job>, StoreError> {
        self.services
            .read(move |store| store.recording_job(id))
            .await
    }

    pub(crate) async fn completion(
        self,
        id: JobId,
    ) -> Result<Option<RecordingCompletion>, StoreError> {
        self.services
            .read(move |store| store.recording_completion(id))
            .await
    }

    pub(crate) async fn prepare_completion(
        self,
        job: Job,
        completion: RecordingCompletion,
    ) -> Result<bool, StoreError> {
        self.services
            .call(move |store| store.prepare_recording_completion(&job, &completion))
            .await
    }

    pub(crate) async fn commit_completion(
        self,
        completion: RecordingCompletion,
    ) -> Result<bool, StoreError> {
        self.services
            .call(move |store| store.commit_recording_completion(&completion))
            .await
    }

    pub(crate) async fn retain_partial_completion(
        self,
        completion: RecordingCompletion,
    ) -> Result<bool, StoreError> {
        self.services
            .call(move |store| store.retain_partial_recording_completion(&completion))
            .await
    }
}
