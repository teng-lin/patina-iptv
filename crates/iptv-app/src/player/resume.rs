//! The last channel before the guide: at a cold start the stored source's
//! last channel starts from its one stored row as soon as the store is
//! open ([`early`], from `data::start`), in parallel with reading the
//! catalog and the guide window, as the reference app puts video first.
//! Until the guide arrives the full-screen player plays that stream with
//! no catalog channel behind it (`Ui::playing` is `None`); when the guide
//! is shown, [`adopt`] finds the channel in it by key and makes it the
//! playing channel without opening the stream again.
//!
//! Only streams that resolve without the network start early (M3U, and
//! Xtream with the stored account's details); a Stalker channel, a
//! blocked one, or one the store no longer has starts as before, once the
//! guide is shown (`player::source_ready`).

use iptv_core::playback::PlaySource;
use iptv_guide::playback::{Resolution, resolve};
use iptv_library::StoredSource;
use iptv_providers::id::ContentId;

use crate::{Playback, Ui, ViewMode};

/// The stream surface 0 plays before the guide arrived.
#[derive(Clone, Debug, PartialEq)]
pub struct Early {
    /// The channel's content key (finds it in the guide).
    pub key: String,
    pub source: PlaySource,
}

/// Startup, with the store open: starts `stored`'s last channel full
/// screen when it may resume and resolves from the store alone. False
/// when it does not start (the guide's arrival resumes it, if anything).
pub async fn early(ui: Ui, stored: &StoredSource) -> bool {
    let generation = crate::data::generation();
    if !crate::settings::flag(iptv_settings::keys::RESUME_LAST_CHANNEL)
        || ui.wizard.with_untracked(Option::is_some)
        || ui.playing.get_untracked().is_some()
    {
        return false;
    }
    let Some(context) = super::stored_context(&stored.source.config) else {
        return false;
    };
    let Some(key) = super::load_last_channel(Some(stored.id)) else {
        return false;
    };
    let id = stored.id;
    let content = ContentId::from_stored(key.clone());
    let row = match crate::runtime::commands::channel(id, content).await {
        Ok(Some(row)) if !row.blocked => row,
        Ok(_) => return false,
        Err(e) => {
            log::warn!("iptv player: the last channel's row: {e}");
            return false;
        }
    };
    if crate::data::generation() != generation
        || ui.wizard.with_untracked(Option::is_some)
        || ui.playing.get_untracked().is_some()
    {
        return false;
    }
    let source = match resolve(Some(&context), &row.channel, None, super::clock(None)) {
        Resolution::Play(source) => source,
        Resolution::StalkerLink(_) | Resolution::Unavailable(_) => return false,
    };
    log::info!("iptv player: resuming the last channel before the guide");
    crate::startup::mark("last channel resolved");
    let main = ui.player.main();
    let mut session = 0;
    let now = ui.now_ms();
    main.health.update(|h| session = h.start(true, now));
    ui.player.resume.set(false);
    ui.playback.set(Playback::Live);
    ui.player.early.set(Some(Early {
        key,
        source: source.clone(),
    }));
    super::load(ui, 0, session, source);
    ui.mode.set(ViewMode::Fullscreen);
    true
}

/// Whether surface 0 plays an early stream (no catalog channel yet).
pub fn playing(ui: Ui) -> bool {
    ui.player.early.with(Option::is_some)
}

/// The early stream to open again (a retry before the guide arrived).
pub fn source(ui: Ui) -> Option<PlaySource> {
    ui.player
        .early
        .with_untracked(|e| e.as_ref().map(|e| e.source.clone()))
}

/// The guide is on screen: the early stream's channel becomes the playing
/// channel, found by key, with its info card. True when there was an early
/// stream (also when its channel is gone and it stopped).
pub fn adopt(ui: Ui) -> bool {
    let Some(early) = ui.player.early.get_untracked() else {
        return false;
    };
    let main = ui.player.main();
    let Some(channel) = crate::schedule().find_channel(&early.key) else {
        log::info!("iptv player: the early channel is not in the guide; stopping");
        ui.player.early.set(None);
        super::stop(ui);
        ui.mode.set(crate::settings::browse_mode());
        return true;
    };
    let open = main.source.with_untracked(Option::is_some);
    nsh_core::prelude::batch(|| {
        main.key.set(Some(early.key.clone()));
        ui.player.history.update(|h| h.played(&early.key));
        ui.playing.set(Some(channel));
        ui.player.early.set(None);
    });
    crate::favorites::played(&early.key);
    super::show_channel(ui, channel);
    if !open {
        // It closed meanwhile (stopped, or failed for good): start it as
        // the guide has it.
        ui.play(channel, Playback::Live);
    }
    if ui.mode.get_untracked() == ViewMode::Fullscreen {
        ui.update_panels(|p, now| p.show_info(now));
    }
    true
}
