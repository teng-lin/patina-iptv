//! Native on-demand transport; seeking keeps the same player and stream open.
use crate::Ui;

pub fn active(ui: Ui) -> bool {
    ui.player.recorded.with(Option::is_some)
        || (!ui.player.main().live.get() && ui.playing.get().is_none())
}
pub fn seekable(ui: Ui) -> bool {
    active(ui) && ui.player.main().timeline.with(|t| t.seekable)
}
pub fn seek_to(ui: Ui, ms: u64) -> bool {
    if !active(ui) {
        return false;
    }
    let stream = ui.player.main();
    let mut command = None;
    stream.timeline.update(|t| command = t.seek_to(ms));
    if let Some(command) = command {
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
    if surface != 0 || token != stream.token.get_untracked() || !active(ui) {
        return;
    }
    let mut accepted = false;
    stream
        .timeline
        .update(|t| accepted = t.observe(seek_id, ms, duration_ms, seekable));
    if !accepted {
        return;
    }
    if seekable && let Some(resume) = stream.resume_ms.get_untracked() {
        stream.resume_ms.set(None);
        if resume > 0 {
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
    if stream.resume_ms.get_untracked().is_some() {
        return;
    }
    let progress = stream.timeline.get_untracked().progress(ended);
    if let Some((job, source)) = ui.player.recorded.get_untracked() {
        // This method queues the write immediately; disposal cannot lose the last point.
        crate::runtime().save_recording_progress(job.id, &source, progress);
    }
}
