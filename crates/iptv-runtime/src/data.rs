//! Real data: imports a source and its EPG off the UI thread, keeps it in
//! the SQLite store, and swaps the guide to the imported schedule.
//!
//! At startup enabled sources are browsed together when several are saved;
//! otherwise the stored source shown last (else the newest) is refreshed.
//! With none, the add-playlist wizard opens on its empty state (`wizard_view`).
//! The wizard adds sources
//! through [`start_import`]. A new source is stored only once its import
//! succeeded, so a failed or canceled attempt leaves nothing behind. The
//! playlists screen switches, refreshes, renames and deletes stored
//! sources through this module. The library prepares imports and persists changes,
//! and this module applies the corresponding guide and browser transitions.
//!
//! A stored source's saved channels and guide are on screen before its
//! refresh reaches the network, and stay when the refresh fails. Its guide
//! is downloaded again only when the refresh policy says so (`stored`).
//! Only the source on screen has guide timers: each shown import starts a
//! new generation, and timers of an older one stop.

mod admission;
pub mod assign;
pub mod combined;
mod progress;
mod refresh_scheduler;
mod schedule_view;
pub use crate::guides;
pub mod library;
pub mod opening;
mod organization;
pub mod pages;
pub mod stored;
pub use crate::wizard_map;

pub use library::{Activity, Playlist, playlists, source_details};
use std::time::Duration;
pub use wizard_map::source_from_draft;

/// The shortest time between two progress reports shown while an import
/// runs. Reports wake the UI themselves (an awaited channel, no polling);
/// a fast local import would otherwise repaint for each.
const PROGRESS_FRAME: Duration = Duration::from_millis(100);
/// Guide checks wait at most this long at a time (a sleeping device can
/// delay timers; an early check finds nothing due and waits again)...
const MAX_WAIT: Duration = Duration::from_secs(6 * 3600);
/// ...and at least this long.
const MIN_WAIT: Duration = Duration::from_secs(60);
/// Let multiple edits to the guide's feeds settle before checking them.
const GUIDE_EDIT_SETTLE: Duration = Duration::from_secs(2);

pub use crate::imports::{Job, Purpose, Update};

#[cfg(test)]
use crate::Runtime;
#[cfg(test)]
use crate::imports::kept_file_name;
pub(super) use admission::*;
pub use admission::{
    delete, edit_guides, next_playlist_after_deletion, order_changed, refresh, refresh_all, rename,
    reorder, retry_startup, set_policy, set_user_agent, start, switch_to, try_details,
    update_policy,
};
#[cfg(test)]
use iptv_core::add_playlist::FailureKind;
#[cfg(test)]
use iptv_providers::model::{Source, SourceConfig};
pub use progress::start_import;
pub(super) use progress::*;
pub(super) use refresh_scheduler::*;
pub use refresh_scheduler::{
    clear_guide, clear_source_guide, refresh_guide_status, update_guide_now, update_source_guide,
};
pub(super) use schedule_view::*;
pub use schedule_view::{
    combined_includes, generation, has_multiple_enabled, organization_changed, switch_all,
    toggle_playlist,
};
pub(crate) use schedule_view::{repair_source_shown, show_repair};
#[cfg(test)]
use std::cell::{Cell, RefCell};
#[cfg(test)]
use std::rc::Rc;
#[cfg(all(test, feature = "native"))]
use tokio::sync::mpsc::unbounded_channel;

#[cfg(all(test, feature = "native"))]
mod playlist_lifecycle_tests {
    use super::*;
    use crate::imports::GuideRead;
    use iptv_providers::{SecretUrl, model::M3uSourceConfig};

    #[test]
    fn wizard_import_rejects_unstarted_storage_or_an_unbound_task_host() {
        for start_services in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let owner = Rc::new(Runtime::new(crate::Ports::new(
                directory.path().into(),
                |_| Ok(crate::model::DatabaseKey::from_bytes([0x72; 32])),
            )));
            if start_services {
                owner.start_services();
            }
            let admitted = Rc::new(Cell::new(None));
            let request = admitted.clone();
            let reports = Rc::new(RefCell::new(Vec::new()));
            let sink = reports.clone();
            let (id, epoch) = owner.start_wizard_import(
                Job {
                    source: synthetic("Synthetic unbound import"),
                    epg_url: None,
                    stored: None,
                    purpose: Purpose::Add,
                    edit: false,
                },
                |id, epoch| admitted.set(Some((id, epoch))),
                move |id, update| {
                    assert_eq!(request.get().map(|(admitted, _)| admitted), Some(id));
                    sink.borrow_mut().push(update);
                },
            );
            assert_eq!(admitted.get(), Some((id, epoch)));
            assert!(matches!(
                reports.borrow().as_slice(),
                [Update::Done(Err(failure))] if failure.kind == FailureKind::Unavailable
            ));
            assert!(owner.imports.wizard_import.borrow().is_none());
            if start_services {
                assert!(owner.drain_services());
            }
        }
    }

    fn run(test: impl AsyncFnOnce(Rc<Runtime>)) {
        run_with(|_| {}, test);
    }

    fn run_with(configure: impl FnOnce(&mut crate::Ports), test: impl AsyncFnOnce(Rc<Runtime>)) {
        let directory = tempfile::tempdir().unwrap();
        let mut ports = crate::Ports::new(directory.path().into(), |_| {
            Ok(crate::model::DatabaseKey::from_bytes([0x73; 32]))
        });
        configure(&mut ports);
        let owner = Rc::new(Runtime::new(ports));
        owner.bind_tasks(
            |task| drop(tokio::task::spawn_local(task)),
            |duration| Box::pin(tokio::time::sleep(duration)),
        );
        owner.start_services();
        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        tokio::task::LocalSet::new().block_on(&executor, async {
            tokio::time::timeout(Duration::from_secs(15), test(owner.clone()))
                .await
                .expect("playlist lifecycle scenario timed out");
        });
        assert!(owner.drain_services());
    }

    #[test]
    fn guide_worker_refusal_reports_failure_and_leaves_a_check_owed() {
        use crate::execution::{Execution, Job, Lane, LocalTask, Task, Tasks};
        use crate::native::execution::NativeExecution;

        struct RefusingGuideExecution;
        impl Execution for RefusingGuideExecution {
            fn lane(&self, name: &'static str) -> std::io::Result<Box<dyn Lane>> {
                NativeExecution.lane(name)
            }
            fn worker(&self, name: String, job: Job) -> std::io::Result<()> {
                NativeExecution.worker(name, job)
            }
            fn tasks(&self) -> Box<dyn Tasks> {
                struct Refused;
                impl Tasks for Refused {
                    fn ready(&self) -> bool {
                        false
                    }
                    fn spawn(&self, _: Task) {}
                    fn spawn_blocking(&self, _: Job) {}
                    fn block_on(&self, _: LocalTask<'_>) {}
                    fn delay(&self, _: Duration) -> Task {
                        Box::pin(std::future::ready(()))
                    }
                }
                Box::new(Refused)
            }
        }

        run_with(
            |ports| ports.execution = std::sync::Arc::new(RefusingGuideExecution),
            async |owner| {
                let source = owner
                    .services()
                    .call(|store| {
                        let source = store.add_source(&synthetic("Synthetic guide retry"))?;
                        let feed = crate::guides::user_source(
                            crate::guides::FIRST_USER_FEED,
                            SecretUrl::new("http://127.0.0.1:9/synthetic.xml"),
                            None,
                        );
                        store.set_guide_sources(source, &[feed])?;
                        Ok(source)
                    })
                    .await
                    .unwrap();
                owner.catalog.set_shown_source(Some(source));
                let generation = owner.imports.generation.get();
                let (done, result) = futures_channel::oneshot::channel();
                assert!(start_update(&owner, source, None, true, move |accepted| {
                    let _ = done.send(accepted);
                }));
                assert!(!result.await.unwrap(), "the manual update never ran");
                assert!(owner.has_guide_check(source));

                owner.end_guide_check(source);
                read_guide(&owner, source, generation, GuideRead::Check, None, true).await;
                assert!(owner.has_guide_check(source));
            },
        );
    }

    fn synthetic(name: &str) -> Source {
        Source {
            name: name.to_owned(),
            config: SourceConfig::M3u(M3uSourceConfig {
                location: SecretUrl::new("https://example.test/synthetic.m3u"),
                user_agent: None,
                referrer: None,
            }),
        }
    }

    #[test]
    fn deleting_the_shown_source_previews_the_enabled_manual_order_fallback() {
        run(async |owner| {
            let (shown, fallback, hidden) = owner
                .services()
                .call(|store| {
                    let shown = store.add_source(&synthetic("Synthetic shown"))?;
                    let hidden = store.add_source(&synthetic("Synthetic hidden"))?;
                    let fallback = store.add_source(&synthetic("Synthetic fallback"))?;
                    let earlier = store.add_source(&synthetic("Synthetic earlier"))?;
                    store.set_active_source(shown)?;
                    store.set_source_order(&[shown, earlier, fallback, hidden])?;
                    store.set_source_policy(
                        hidden,
                        crate::model::SourcePolicy {
                            enabled: false,
                            ..Default::default()
                        },
                    )?;
                    Ok((shown, fallback, hidden))
                })
                .await
                .unwrap();
            crate::library::install(&owner, crate::library::snapshot(&owner).await.unwrap());
            assert_eq!(
                next_playlist_after_deletion(&owner, shown).as_deref(),
                Some("Synthetic fallback")
            );
            assert_eq!(
                next_playlist_after_deletion(&owner, hidden).as_deref(),
                Some("Synthetic shown")
            );
            owner
                .services()
                .call(move |store| {
                    store.delete_source(shown)?;
                    Ok(())
                })
                .await
                .unwrap();
            crate::library::install(&owner, crate::library::snapshot(&owner).await.unwrap());
            assert_eq!(
                crate::library::newest(&owner).unwrap().unwrap().id,
                fallback
            );
        });
    }

    #[test]
    fn reenabled_source_stays_shown_while_the_earlier_hide_reloads() {
        run(async |owner| {
            let shown = owner
                .services()
                .call(|store| store.add_source(&synthetic("Synthetic shown")))
                .await
                .unwrap();
            owner.catalog.set_shown_source(Some(shown));
            crate::library::install(&owner, crate::library::snapshot(&owner).await.unwrap());

            // Hold the reader so the checked state from both writes reaches
            // the UI before either can make a playback change.
            let (entered, held) = futures_channel::oneshot::channel();
            let (release, released) = std::sync::mpsc::channel();
            let reader = owner.services().read(move |_| {
                let _ = entered.send(());
                released
                    .recv_timeout(Duration::from_secs(10))
                    .expect("reader was not released");
                Ok(())
            });
            held.await.unwrap();

            let (updates, mut updated) = unbounded_channel();
            let weak = Rc::downgrade(&owner);
            owner.bind(move |event| {
                if matches!(event, crate::RuntimeEvent::LibraryChanged)
                    && let Some(owner) = weak.upgrade()
                    && let Ok(policy) = library::policy(&owner, shown)
                {
                    let _ = updates.send(policy.enabled);
                }
            });

            let set_visibility = |enabled| {
                let owner = owner.clone();
                tokio::task::spawn_local(async move {
                    update_policy(&owner, shown, move |policy| policy.enabled = enabled).await
                })
            };
            let hide = set_visibility(false);
            assert_eq!(updated.recv().await, Some(false));
            let show = set_visibility(true);
            assert_eq!(updated.recv().await, Some(true));
            release.send(()).unwrap();
            reader.await.unwrap();
            hide.await.unwrap().unwrap();
            show.await.unwrap().unwrap();

            assert_eq!(owner.shown_source(), Some(shown));
            assert!(library::policy(&owner, shown).unwrap().enabled);
        });
    }
}

#[cfg(test)]
mod kept_file_tests {
    use super::kept_file_name;

    #[test]
    fn kept_playlist_names_are_portable() {
        assert_eq!(kept_file_name("my list (1).m3u"), "my_list__1_.m3u");
        assert_eq!(kept_file_name("..hidden.m3u"), "hidden.m3u");
        assert_eq!(kept_file_name("list."), "list");
        assert_eq!(kept_file_name("///"), "___");
        assert_eq!(kept_file_name(""), "playlist.m3u");
        for reserved in ["con.m3u", "NUL", "com1.m3u8", "Lpt9.txt"] {
            let kept = kept_file_name(reserved);
            assert_eq!(kept, format!("playlist-{reserved}"));
            assert!(
                iptv_import::backup_manifest::portable_component(&kept),
                "{kept}"
            );
        }
    }
}
