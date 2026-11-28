//! Platform dialogs and document transport for runtime-owned backups.
use crate::host::backup_documents as documents;
use crate::kit::i18n::{tr, trf};
use iptv_runtime::backup::error::{Context, Error, Rejection};
use iptv_runtime::backup::report;

use std::fs::File;

mod review;
pub(crate) use review::{RestoreReview, back, is_open, key, layer};

/// What a backup holds, before its file is chosen.
const EXPORT_NOTE: &str = crate::kit::i18n::key(
    "The unencrypted backup includes playlist logins and passwords, settings, \
     channel arrangements, history and guide setup. Downloaded guide listings are excluded. \
     Keep the file private.",
);
/// A restore is already scheduled: what happens to changes until then.
const PENDING_NOTE: &str = crate::kit::i18n::key(
    "The backup replaces current data when you next open Patina. \
     Changes made before then will be replaced.",
);
/// Android: the app has to be closed and opened for the restore to apply.
const RESTART_NOTE: &str = crate::kit::i18n::key(
    "Close Patina, then open it again to apply the backup. Keep the \
     app open to cancel the scheduled restore from Settings first.",
);

pub(crate) fn export_ui() {
    crate::wiring::tasks::spawn_in_app(async move {
        let Some(busy) = crate::wiring::runtime_owner::runtime().begin_backup() else {
            return;
        };
        let answer = nsh_base::alert_dialog()
            .title(tr("Back up Patina"))
            .description(tr(EXPORT_NOTE))
            .cancel("cancel", tr("Cancel"))
            .action("export", tr("Choose file…"))
            .show()
            .await;
        if !answer.is_ok_and(|a| a.action.as_deref() == Some("export")) {
            return;
        }
        if !crate::guide::favorites::flush().await {
            crate::kit::toast::notify(tr("Recent channel changes could not be saved; try again"));
            return;
        }
        let settings = crate::wiring::settings::get().to_json_string();
        let root = crate::wiring::data::data_dir();
        crate::kit::toast::notify(tr("Writing backup…"));
        let result = documents::save_export(&busy, root, settings, tr("Patina backup")).await;
        crate::kit::toast::notify(tr(match result {
            Ok(true) => "Backup saved",
            Ok(false) => "Backup canceled",
            Err(error) => report("writing a backup", error),
        }));
    });
}

/// The document pickers a device has: one that creates a file (export)
/// and one that opens one (import). Each backup action is offered only
/// where its picker is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pickers {
    pub create: bool,
    pub open: bool,
}

impl Pickers {
    pub const NONE: Pickers = Pickers {
        create: false,
        open: false,
    };
    pub const BOTH: Pickers = Pickers {
        create: true,
        open: true,
    };

    /// What `platform` has before (or without) asking the device.
    pub const fn of(platform: iptv_core::capabilities::Platform) -> Pickers {
        use iptv_core::capabilities::Platform;
        match platform {
            // NSH's save and open dialogs.
            Platform::Linux | Platform::Macos | Platform::Windows => Pickers::BOTH,
            // The iPad's document transport is unfinished: no backup rows
            // until it works.
            Platform::Ipad => Pickers::NONE,
            // Android's host says which pickers resolve (`tv.backup`'s
            // `pickers` event); none until it answers.
            Platform::Android | Platform::Unknown => Pickers::NONE,
        }
    }
}

#[cfg(feature = "test-support")]
static PICKERS_FOR_TEST: std::sync::Mutex<Option<Pickers>> = std::sync::Mutex::new(None);

/// The pickers apps started from now on have, whatever their platform
/// (headless tests: a device without one, or the iPad's).
#[cfg(feature = "test-support")]
pub fn set_pickers_for_test(pickers: Pickers) {
    *PICKERS_FOR_TEST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(pickers);
}

fn initial_pickers() -> Pickers {
    #[cfg(feature = "test-support")]
    if let Some(pickers) = *PICKERS_FOR_TEST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
    {
        return pickers;
    }
    Pickers::of(crate::host::PLATFORM)
}

thread_local! {
    /// This device's pickers, made with the service layer. Android's host
    /// reports them when the layer mounts, when Settings opens
    /// ([`probe_pickers`]) and when the app resumes.
    static PICKERS: std::cell::Cell<Option<nsh_core::prelude::Signal<Pickers>>> =
        const { std::cell::Cell::new(None) };
}

/// This device's pickers (reactive once the service layer is made).
pub(crate) fn pickers() -> Pickers {
    PICKERS
        .with(std::cell::Cell::get)
        .map_or_else(initial_pickers, |pickers| pickers.get())
}

/// Asks the host again which pickers resolve (Settings opening): one may
/// have been installed, enabled or removed since.
pub(crate) fn probe_pickers() {
    documents::probe();
}

/// Whether the backup command `key` (`general.backup` or
/// `general.restore`) can run here. Exporting needs a picker that creates
/// a file. Restoring needs one that opens a file, except to review, keep
/// or cancel a restore already scheduled, which needs none.
pub(crate) fn offered(key: &str) -> bool {
    if !crate::host::HAS_WORKERS {
        return false;
    }
    let pickers = pickers();
    match key {
        "general.backup" => pickers.create,
        _ => pickers.open || crate::wiring::runtime_owner::runtime().restore_is_pending(),
    }
}

/// Whether this host offers the restore entry point (the welcome screen's
/// and Settings').
pub(crate) fn available() -> bool {
    offered("general.restore")
}

pub(crate) fn request_restore(ui: crate::Ui) {
    if available() {
        crate::wiring::navigation::request(
            ui.screens,
            iptv_core::parental::Target::settings(),
            move || restore_ui(ui),
        );
    }
}

fn restore_ui(ui: crate::Ui) {
    crate::wiring::tasks::spawn_in_app(async move {
        let Some(busy) = crate::wiring::runtime_owner::runtime().begin_backup() else {
            return;
        };
        let root = crate::wiring::data::data_dir();
        if crate::wiring::runtime_owner::runtime().restore_is_pending() {
            let dialog = nsh_base::alert_dialog()
                .title(tr("Restore scheduled"))
                .description(tr(PENDING_NOTE))
                .cancel("keep", tr("Keep scheduled"))
                .action("cancel", tr("Cancel restore"));
            // Without a picker that opens files the scheduled restore can be
            // kept or canceled, not replaced.
            let dialog = if pickers().open {
                dialog.action("replace", tr("Choose another backup…"))
            } else {
                dialog
            };
            let answer = dialog.show().await;
            match answer.ok().and_then(|a| a.action) {
                Some(action) if action.as_str() == "cancel" => {
                    let result = busy.worker(|archive| archive.cancel()).await;
                    crate::wiring::settings_wiring::refresh(ui.app);
                    crate::kit::toast::notify(tr(match result {
                        Ok(()) => "Scheduled restore canceled",
                        Err(error) => report("canceling the scheduled restore", error),
                    }));
                    return;
                }
                Some(action) if action.as_str() == "replace" => {}
                _ => {
                    offer_restart().await;
                    return;
                }
            }
        }
        let mut file = match documents::select_import(root, tr("Patina backup")).await {
            Ok(Some(file)) => file,
            Ok(None) => return,
            Err(error) => {
                crate::kit::toast::notify(tr(report("choosing a backup", error)));
                return;
            }
        };
        if !review::confirm(ui.screens, file.name.take()).await {
            return;
        }
        crate::kit::toast::notify(tr("Checking backup…"));
        let result = busy
            .worker(move |archive| {
                let mut input =
                    File::open(&file.path).context("Could not read the chosen backup")?;
                archive.stage(&mut input)
            })
            .await;
        crate::wiring::settings_wiring::refresh(ui.app);
        // Release operation ownership before presenting a dismissible failure.
        drop(busy);
        match result {
            Ok(()) => {
                crate::kit::toast::notify(tr(
                    "Restore scheduled. Close and reopen Patina to apply it",
                ));
                offer_restart().await;
            }
            Err(error) => {
                if error.rejection().is_some() {
                    report("scheduling a restore", error);
                    let _ = nsh_base::alert_dialog()
                        .title(tr("Backup could not be restored"))
                        .description(rejection_description(error))
                        .cancel("close", tr("Close"))
                        .show()
                        .await;
                } else {
                    crate::kit::toast::notify(tr(report("scheduling a restore", error)));
                }
            }
        }
    });
}

/// Independent facts are paragraphs so recovery guidance remains readable in
/// native dialogs at enlarged text sizes. No archive-provided text is shown.
fn rejection_description(error: Error) -> String {
    rejection_paragraphs(
        error.rejection(),
        tr("Your current data and any previously scheduled restore have been kept."),
    )
}

pub(crate) fn recovery_rejection_description(rejection: Rejection) -> String {
    rejection_paragraphs(
        Some(rejection),
        tr("Choose another backup, or start fresh with an empty library."),
    )
}

fn rejection_paragraphs(rejection: Option<Rejection>, outcome: &str) -> String {
    let (detail, newer) = match rejection {
        Some(Rejection::Format {
            found: Some(found),
            supported,
        }) => (
            trf(
                "Backup format version {found} is not supported; this Patina supports version {supported}.",
                &[("found", found.into()), ("supported", supported.into())],
            ),
            found > supported,
        ),
        Some(Rejection::Format { found: None, .. }) => {
            (tr("This backup format is not supported.").to_owned(), false)
        }
        Some(Rejection::Settings {
            found: Some(found),
            supported,
        }) => (
            trf(
                "This Patina cannot restore these backup settings (version {found}; supported version {supported}).",
                &[("found", found.into()), ("supported", supported.into())],
            ),
            found > supported,
        ),
        Some(Rejection::Settings { found: None, .. }) => (
            tr("This backup contains settings in an unsupported format.").to_owned(),
            false,
        ),
        Some(Rejection::Database { found, supported }) => (
            trf(
                "Backup database schema {found} is not supported; this Patina supports schema {supported}.",
                &[("found", found.into()), ("supported", supported.into())],
            ),
            found > supported,
        ),
        _ => {
            return [
                tr(
                    "This backup is damaged or incomplete. Choose another backup, \
                     or create a new backup on the original device.",
                ),
                outcome,
            ]
            .join("\n\n");
        }
    };
    let next = if newer {
        tr("Use a newer Patina version that supports this backup, or choose another backup.")
    } else {
        tr("Choose another backup, or use a Patina version that supports this backup.")
    };
    [detail.as_str(), next, outcome].join("\n\n")
}

pub(crate) fn service_layer(ui: crate::Ui) -> impl nsh_core::prelude::IntoElement {
    use nsh_core::prelude::*;
    let pickers = crate::host::ANDROID.then(|| signal(initial_pickers()));
    if let Some(pickers) = pickers {
        PICKERS.with(|slot| slot.set(Some(pickers)));
    }
    documents::layer(move |create, open| {
        let Some(pickers) = pickers else { return };
        let found = Pickers { create, open };
        if pickers.get_untracked() != found {
            pickers.set(found);
            let restore = available();
            ui.screens.wizard.update(|open| {
                if let Some(open) = open {
                    open.wizard.set_restore(restore);
                }
            });
            crate::wiring::navigation::set_restore_available(
                ui.chrome,
                crate::host::HAS_WORKERS && found.open,
            );
        }
    })
}

async fn offer_restart() {
    if crate::host::ANDROID {
        let answer = nsh_base::alert_dialog()
            .title(tr("Restore scheduled"))
            .description(tr(RESTART_NOTE))
            .cancel("keep", tr("Keep app open"))
            .action("close", tr("Close Patina"))
            .show()
            .await;
        if answer.is_ok_and(|a| a.action.as_deref() == Some("close")) {
            documents::close(crate::wiring::data::data_dir()).await;
        }
    }
}
