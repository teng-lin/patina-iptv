//! Native on-demand transport; seeking keeps the same player and stream open.
use crate::Ui;

pub fn active(ui: Ui) -> bool {
    ui.player.recorded.with(Option::is_some) || ui.player.vod.with(Option::is_some)
}
pub fn seekable(ui: Ui) -> bool {
    let stream = ui.player.main();
    let published = stream.native_published();
    let seekable = stream.timeline.with(|timeline| timeline.seekable);
    active(ui) && published && seekable
}

pub fn seek_to(ui: Ui, ms: u64) -> bool {
    if !active(ui) || !ui.player.main().accepts_native(None) {
        return false;
    }
    let stream = ui.player.main();
    let mut command = None;
    stream.timeline.update(|t| command = t.seek_to(ms));
    if let Some(command) = command {
        crate::interruption::viewer_acted(ui);
        if stream.finished.get_untracked() {
            stream.finished.set(false);
            ui.player.vod.update(|playing| {
                if let Some((playing, _)) = playing {
                    playing.ended = false;
                }
            });
        }
        stream.seek.set(Some(iptv_player_mpv::SeekProps {
            token: stream.token.get_untracked(),
            id: command.id,
            ms: command.ms,
        }));
        flush(ui, false);
        true
    } else {
        false
    }
}
pub fn step(ui: Ui, seconds: i32) -> bool {
    let position = ui.player.main().timeline.get_untracked().position_ms;
    seek_to(
        ui,
        position.saturating_add_signed(i64::from(seconds) * 1000),
    )
}
pub fn observe(
    ui: Ui,
    surface: usize,
    token: u64,
    seek_id: u64,
    ms: u64,
    duration_ms: Option<u64>,
    seekable: bool,
) {
    let stream = ui.player.stream(surface);
    if surface != 0 || !stream.accepts_native(Some(token)) || !active(ui) {
        return;
    }
    let mut accepted = false;
    stream
        .timeline
        .update(|t| accepted = t.observe(seek_id, ms, duration_ms, seekable));
    if !accepted {
        return;
    }
    if let Some(resume) = stream.resume_ms.get_untracked() {
        // A current native timeline is authoritative even for an unseekable
        // stream: it started from the beginning and cannot apply the old resume.
        stream.resume_ms.set(None);
        if seekable && resume > 0 {
            seek_to(ui, resume);
            return;
        }
    }
    // A source which has not yet accepted its restored position must not erase it.
    if stream.resume_ms.get_untracked().is_none() {
        flush(ui, false);
    }
}
pub fn flush(ui: Ui, ended: bool) {
    let stream = ui.player.main();
    if !stream.accepts_native(None) {
        return;
    }
    let ended = ended || stream.finished.get_untracked();
    if stream.resume_ms.get_untracked().is_some() {
        if !ended {
            return;
        }
        // A short unseekable stream may end before its first timeline tick.
        stream.resume_ms.set(None);
    }
    let progress = stream.timeline.get_untracked().progress(ended);
    crate::vod_view::observe(ui, progress.position_ms, progress.duration_ms, ended);
    if let Some((job, source)) = ui.player.recorded.get_untracked() {
        // This method queues the write immediately; disposal cannot lose the last point.
        crate::runtime().save_recording_progress(job.id, &source, progress);
    }
}

pub fn title(ui: Ui) -> Option<String> {
    ui.player
        .recorded
        .with(|r| r.as_ref().map(|(job, _)| job.request.title.clone()))
        .or_else(|| crate::vod_view::title(ui))
}
