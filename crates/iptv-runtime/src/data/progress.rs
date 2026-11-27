use super::*;
use crate::imports::{Imported, Msg, pipeline};
use crate::store_service::Services;
use crate::time::now_unix;
use iptv_core::add_playlist::{FailureKind, ImportFailure};
use iptv_import::CancelToken;
use std::cell::Cell;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::mpsc::unbounded_channel;

pub fn start_import(
    owner: &std::rc::Rc<crate::Runtime>,
    job: Job,
    on_update: impl Fn(Update) + 'static,
) -> CancelToken {
    let cancel = CancelToken::new();
    start_import_with(owner, job, cancel.clone(), owner.services(), on_update);
    cancel
}

/// [`start_import`] with the caller's cancel token and storage admission.
pub(crate) fn start_import_with(
    owner: &std::rc::Rc<crate::Runtime>,
    job: Job,
    cancel: CancelToken,
    services: Services,
    on_update: impl Fn(Update) + 'static,
) {
    if !owner.has_tasks() {
        on_update(Update::Done(Err(ImportFailure::new(
            FailureKind::Unavailable,
        ))));
        return;
    }
    let purpose = job.purpose;
    let mut import_source = job.stored;
    let completion = crate::library::import_completion(cancel.clone());
    if let Some(source) = import_source {
        crate::library::track_import(owner, source, &completion);
    }
    let previous = Cell::new(
        (purpose == Purpose::Add)
            .then(|| ready_single(owner))
            .flatten(),
    );
    let shows = purpose.shows() && !combined::active(owner);
    let generation = if shows {
        next_generation(owner, owner.catalog.shown_source())
    } else {
        owner.imports.generation.with(Cell::get)
    };
    // Only the startup refresh may start the last channel.
    let resume = purpose == Purpose::Startup;
    let (tx, mut rx) = unbounded_channel();
    {
        let cancel = cancel.clone();
        let revoked = completion.guide.clone();
        let visible = job
            .stored
            .is_some_and(|source| source_is_visible(owner, source))
            || matches!(
                purpose,
                Purpose::Add | Purpose::Startup | Purpose::Switch | Purpose::Refresh
            );
        owner.spawn({
            let owner = owner.clone();
            async move {
                let opens_stored = job.purpose.shows_stored_first() && job.stored.is_some();
                if !services.can_import() && !opens_stored {
                    // A new import cannot run on this host; nothing is read
                    // for it.
                    let _ = tx.send(Msg::Playlist(Err(ImportFailure::new(
                        FailureKind::Unavailable,
                    ))));
                    return;
                }
                let authorization = owner.parental_snapshot();
                let admission = job.clone();
                let auth = authorization.clone();
                let admission = services
                    .parental()
                    .import_allowed_for(admission, auth)
                    .await;
                let failure = match admission {
                    _ if cancel.is_canceled() || !services.is_current() => {
                        Some(FailureKind::Canceled)
                    }
                    Ok(true) => None,
                    Ok(false) => Some(FailureKind::Auth),
                    Err(error) => {
                        log::warn!("iptv data: import admission could not read the store: {error}");
                        Some(FailureKind::Storage)
                    }
                };
                if let Some(kind) = failure {
                    let _ = tx.send(Msg::Playlist(Err(ImportFailure::new(kind))));
                    return;
                }
                if job.purpose.shows_stored_first()
                    && let Some(id) = job.stored
                    && opening::send_stored(&services, &job, id, now_unix(), &tx, &cancel, &revoked)
                        .await
                {
                    return;
                }
                if cancel.is_canceled() {
                    let _ = tx.send(Msg::Playlist(Err(ImportFailure::new(
                        FailureKind::Canceled,
                    ))));
                    return;
                }
                if !services.can_import() {
                    // Imports cannot run on this host: not a passing
                    // condition to retry.
                    let _ = tx.send(Msg::Playlist(Err(ImportFailure::new(
                        FailureKind::Unavailable,
                    ))));
                    return;
                }
                let failed = tx.clone();
                let failed_if_worker_unavailable = tx.clone();
                // A new source with a typed guide waits for its terminal
                // result even after Cancel. Waiting for an unrelated import
                // on the browser lane should not delay that result.
                let cancel_while_queued =
                    (job.stored.is_none() && job.epg_url.is_some()).then(|| cancel.clone());
                if !services.submit_import(
                    visible,
                    cancel_while_queued,
                    move |worker| async move {
                        pipeline(&worker, job, &tx, &cancel, &revoked, &authorization).await;
                    },
                    move || {
                        let _ = failed_if_worker_unavailable.send(Msg::Playlist(Err(
                            ImportFailure::new(FailureKind::Unavailable),
                        )));
                    },
                ) {
                    let _ = failed.send(Msg::Playlist(Err(ImportFailure::new(FailureKind::Busy))));
                }
            }
        });
    }
    let merge_requested = Cell::new(false);
    let job = completion.clone();
    let show = {
        let owner = owner.clone();
        async move |imported: Box<Imported>, held: Option<opening::Held>| {
            let _ = library::refresh_cache(&owner).await;
            if owner.imports.generation.with(Cell::get) != generation {
                return;
            }
            if shows || combined::active(&owner) {
                let merge = purpose == Purpose::Add
                    && !combined::active(&owner)
                    && !merge_requested.get()
                    && has_multiple_enabled(&owner);
                let source = imported.source;
                show_part(&owner, imported, generation, resume, held, Some(&job)).await;
                if merge {
                    merge_requested.set(true);
                    join_after_add(&owner, source, generation, previous.take());
                }
            }
        }
    };
    owner.spawn({
        let owner = owner.clone();
        async move {
            let mut finished = false;
            let mut failed = false;
            // Whether this job set the source's next guide check, or settled
            // that the policy asks for none.
            let mut answered = false;
            // The next check this job asked for after its handoff was revoked.
            let mut unscheduled = None;
            loop {
                let mut progress = None;
                let mut open = true;
                // Sleeps until the import sends: its send wakes this task.
                let mut next = rx.recv().await.ok_or(TryRecvError::Disconnected);
                loop {
                    match next {
                        Ok(Msg::Progress(p)) => progress = Some(p),
                        Ok(Msg::Playlist(result)) => {
                            completion.playlist_done();
                            if import_source.is_none()
                                && owner.imports.generation.with(Cell::get) == generation
                                && let Ok((_, imported)) = &result
                            {
                                import_source = Some(imported.source);
                                crate::library::track_added_import(
                                    &owner,
                                    imported.source,
                                    &completion,
                                );
                                // A new playlist may have a deleted one's id:
                                // it starts from the global settings.
                                if let Err(error) = crate::settings::forget_import_switches(
                                    &owner,
                                    Some(&[imported.source]),
                                    &[],
                                ) {
                                    log::warn!(
                                        "iptv data: an old playlist's settings stay: {error}"
                                    );
                                }
                            }
                            if import_source.is_some_and(|source| {
                                !crate::library::import_is_current(&owner, source, &completion)
                            }) {
                                // A guide action can revoke handoff while authentication
                                // is still running. Keep its successful playback metadata,
                                // then cancel the superseded post-playlist guide phase.
                                completion.cancel_guide();
                            }
                            if let Some(p) = progress.take() {
                                on_update(Update::Progress(p));
                            }
                            finished = true;
                            let result = match result {
                                Ok((counts, imported)) => {
                                    show(imported, None).await;
                                    Ok(counts)
                                }
                                Err(error) => Err(error),
                            };
                            failed = result.is_err();
                            on_update(Update::Done(result));
                        }
                        Ok(Msg::Guide(imported)) => {
                            if crate::library::import_is_current(
                                &owner,
                                imported.source,
                                &completion,
                            ) {
                                show(imported, None).await;
                            }
                        }
                        Ok(Msg::FirstScreen(imported, held, shown)) => {
                            show(imported, Some(held)).await;
                            // Only now may the opening read the whole window.
                            drop(shown);
                        }
                        Ok(Msg::NextCheck { source, at })
                            if (shows || combined_includes(&owner, source))
                                && crate::library::import_is_current(
                                    &owner,
                                    source,
                                    &completion,
                                ) =>
                        {
                            // Not when a newer generation began meanwhile
                            // (an Add, a switch): should that fail and leave
                            // this playlist on screen, this is what it is owed.
                            if !schedule_check(&owner, source, at, generation) {
                                owner.owe_guide_check(source, at);
                            }
                            answered = true;
                        }
                        Ok(Msg::NextCheck { at, .. }) => unscheduled = Some(at),
                        Ok(Msg::NoCheck { source }) => {
                            if crate::library::import_is_current(&owner, source, &completion) {
                                // Nothing is owed any more: the policy answered.
                                owner.take_owed_guide_check(source);
                                answered = true;
                            }
                        }
                        Ok(Msg::WorkerUnavailable) => {}
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => {
                            open = false;
                            break;
                        }
                    }
                    // What else came meanwhile, in one go (progress coalesces).
                    next = rx.try_recv();
                }
                let reported = progress.is_some();
                if let Some(p) = progress.filter(|_| !finished) {
                    on_update(Update::Progress(p));
                }
                if !open {
                    if !finished {
                        failed = true;
                        let kind = if cancel.is_canceled() {
                            FailureKind::Canceled
                        } else {
                            log::warn!("iptv data: import ended without a result");
                            FailureKind::Network
                        };
                        on_update(Update::Done(Err(ImportFailure::new(kind))));
                    }
                    break;
                }
                if reported {
                    // Progress comes as fast as a local file reads: repaint it
                    // at most this often. No wait while nothing comes.
                    owner.delay(PROGRESS_FRAME).await;
                }
            }
            completion.finish();
            // This job canceled the source's timer when it started and did
            // not answer for it: it failed, a guide action revoked its handoff
            // (its NextCheck was dropped), or it could not read the store.
            // Newer work that replaced it answers instead.
            if let Some(source) = import_source
                && !answered
                && (shows || combined_includes(&owner, source))
                && !completion.superseded.get()
            {
                keep_guide_check(&owner, source, generation, unscheduled);
            }
            // A shown job starts a generation, which ends every timer. When it
            // fails before showing its source (a wizard's Add, a switch), the
            // playlist that stays on screen would have none.
            if failed
                && shows
                && let Some(still_shown) = owner.catalog.shown_source()
                && Some(still_shown) != import_source
            {
                keep_shown_check(&owner, still_shown, generation);
            }
        }
    });
}
