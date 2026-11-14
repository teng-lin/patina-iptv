//! Which guide work leaves the next guide check to what replaced it: work
//! that newer work for the source replaced (another import, an update asked
//! for). A guide action only revokes the handoff, and an Add that learns
//! its source after another job of it has finished was never replaced: both
//! still answer for the playlist's check themselves.
use crate::{Ports, Runtime, library, model::SourceId};
use iptv_import::CancelToken;
use std::rc::Rc;

#[test]
fn only_newer_work_takes_the_next_guide_check_over() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = Rc::new(Runtime::new(Ports::new(
        directory.path().to_owned(),
        |_| Ok(crate::model::DatabaseKey::from_bytes([0x74; 32])),
    )));
    {
        let source = SourceId(1);
        let work = || library::import_completion(CancelToken::new());

        // A guide action while the playlist is fetched: the guide phase
        // is stopped at once, the import goes on and is not waited for.
        let fetching = work();
        library::track_import(&runtime, source, &fetching);
        assert!(library::supersede_import(&runtime, source).is_none());
        assert!(fetching.guide.is_cancelled() && !fetching.cancel.is_cancelled());
        assert!(!fetching.superseded.get());
        assert!(!library::import_is_current(&runtime, source, &fetching));

        // In its guide phase: cancelled, and returned to be waited for.
        let downloading = work();
        downloading.playlist_done();
        library::track_import(&runtime, source, &downloading);
        let cancelled = library::supersede_import(&runtime, source).expect("the cancelled work");
        assert!(Rc::ptr_eq(&cancelled, &downloading));
        assert!(downloading.cancel.is_cancelled() && !downloading.superseded.get());

        // Newer work: an update asked for, another import over a tracked
        // one, an import that replaces a running one.
        let updated = work();
        library::track_import(&runtime, source, &updated);
        library::replace_import(&runtime, source);
        assert!(updated.superseded.get());
        let (older, newer) = (work(), work());
        library::track_import(&runtime, source, &older);
        library::track_import(&runtime, source, &newer);
        assert!(older.superseded.get() && !newer.superseded.get());
        library::replace_running(&runtime, source);
        assert!(newer.superseded.get() && newer.guide.is_cancelled());

        // A playlist that is being deleted or switched off: its work is
        // cancelled, but nothing replaces it. When the deletion fails the
        // playlist stays, and that work still keeps its check.
        let deleted = work();
        library::track_import(&runtime, source, &deleted);
        library::cancel_running(&runtime, source);
        assert!(deleted.guide.is_cancelled() && !deleted.superseded.get());
        assert!(!library::import_is_current(&runtime, source, &deleted));

        // An Add learns its source after a job of that source has come and
        // gone: it does not own the source, and nothing replaced it.
        let finished = work();
        library::track_import(&runtime, source, &finished);
        finished.finish();
        drop(finished);
        let added = work();
        library::track_added_import(&runtime, source, &added);
        assert!(!library::import_is_current(&runtime, source, &added));
        assert!(!added.superseded.get());
    };
}
