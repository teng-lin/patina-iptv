//! Exercise refresh registration and generation guards through the runtime API.
use iptv_import::CancelToken;
use iptv_runtime::{Ports, Runtime, library, model::SourceId};
use std::rc::Rc;

#[test]
fn combined_refreshes_replace_one_check_without_disturbing_other_sources() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = Rc::new(Runtime::new(Ports::new(
        directory.path().to_owned(),
        |_| Ok(iptv_runtime::model::DatabaseKey::from_bytes([0x73; 32])),
    )));
    {
        let source = SourceId(1);
        let other = SourceId(2);
        let generation = runtime.browser_generation();
        let (_, original) = runtime.schedule_guide_check(source, generation).unwrap();
        let (other_ticket, other_token) = runtime.schedule_guide_check(other, generation).unwrap();
        let first = library::import_completion(CancelToken::new());
        library::track_import(&runtime, source, &first);
        assert!(original.is_cancelled());
        assert!(!runtime.has_guide_check(source));
        first.finish();
        assert!(library::import_is_current(&runtime, source, &first));
        let (_, first_check) = runtime.schedule_guide_check(source, generation).unwrap();
        let second = library::import_completion(CancelToken::new());
        library::track_import(&runtime, source, &second);
        assert!(first_check.is_cancelled());
        assert!(!library::import_is_current(&runtime, source, &first));
        second.finish();
        let (ticket, check) = runtime.schedule_guide_check(source, generation).unwrap();
        assert!(runtime.guide_check_current(source, ticket));
        assert!(runtime.guide_check_current(other, other_ticket));
        assert!(!other_token.is_cancelled());
        // A late response from a previous screen cannot cancel the new timer.
        assert!(
            runtime
                .schedule_guide_check(source, generation.wrapping_sub(1))
                .is_none()
        );
        assert!(!check.is_cancelled());
        runtime.cancel_guide_checks();
        assert!(check.is_cancelled());
        assert!(other_token.is_cancelled());
    }
}
