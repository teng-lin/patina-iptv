//! Player panels and clock, playback compatibility, catch-up and seeking.
//!
//! Default provenance markers as in `general.rs`.

use super::{choices::*, key};
use crate::schema::*;
use crate::scope::Scope;

const fn player(
    sub: &'static str,
    key: &'static str,
    ty: ValueType,
    summary: &'static str,
) -> Setting {
    Setting::new(Section::Player, sub, key, ty, summary)
}

const fn playback(
    sub: &'static str,
    key: &'static str,
    ty: ValueType,
    summary: &'static str,
) -> Setting {
    Setting::new(Section::Playback, sub, key, ty, summary)
}

const fn catchup(
    sub: &'static str,
    key: &'static str,
    ty: ValueType,
    summary: &'static str,
) -> Setting {
    Setting::new(Section::Catchup, sub, key, ty, summary)
}

const IF_CLOCK: &[Dependency] = &[visible_if(&[when("player.clock", Test::IsTrue)])];
const IF_AFR: &[Dependency] = &[enabled_if(&[when("playback.afr", Test::IsTrue)])];

pub static PLAYER: &[Setting] = &[
    // default: ours (iptv-core panels::Timeouts)
    player(
        "accessibility",
        "player.info_panel_timeout",
        secs(1, 30, 1, 4),
        key("The info panel hides itself after this long without input."),
    ),
    // default: ours (iptv-core panels::Timeouts)
    player(
        "accessibility",
        "player.controls_panel_timeout",
        secs(1, 60, 1, 7),
        key("The control panel hides itself after this long without input."),
    ),
    // default: ours
    player(
        "panels",
        "player.info_on_switch",
        boolean(true),
        key("Flash the info panel whenever the channel changes in full screen."),
    ),
    // default: ours
    player(
        "panels",
        "player.switch_info_at_bottom",
        boolean(true),
        key("Place that channel-change info panel along the bottom edge."),
    )
    .depends(&[visible_if(&[when("player.info_on_switch", Test::IsTrue)])]),
    // default: ours
    player(
        "panels",
        "player.description_on_switch_only",
        boolean(false),
        key("Include the program description only in the channel-change panel."),
    ),
    // default: ours
    player(
        "info",
        "player.show_media_info",
        boolean(true),
        key("List stream properties (resolution, frame rate, audio format) in the info panel."),
    ),
    // default: ours
    player(
        "info",
        "player.numeric_resolution",
        boolean(false),
        key("Write resolution as pixel dimensions rather than a quality tag."),
    )
    .depends(&[visible_if(&[when("player.show_media_info", Test::IsTrue)])]),
    // default: ours
    player(
        "clock",
        "player.clock",
        boolean(true),
        key("Keep a clock on screen."),
    ),
    // default: ours
    player(
        "clock",
        "player.clock_date",
        boolean(false),
        key("Add the date to the clock."),
    )
    .depends(&[enabled_if(&[when("player.clock", Test::IsTrue)])]),
    // default: ours
    player(
        "clock",
        "player.clock_position",
        choice(&CORNER, "top_right"),
        key("Screen corner the clock sits in."),
    )
    .depends(IF_CLOCK),
    // default: ours
    player(
        "clock",
        "player.clock_size",
        choice(&CLOCK_SIZE, "medium"),
        key("Clock text scale."),
    )
    .depends(IF_CLOCK),
    // default: ours
    player(
        "buttons",
        "player.history_button",
        boolean(true),
        key("Offer a history shortcut in the control panel."),
    ),
    // default: ours
    player(
        "buttons",
        "player.guide_button",
        boolean(true),
        key("Offer a guide shortcut in the control panel."),
    ),
    // default: ours (iptv-core pip::OnLeave::Auto: on phones and tablets,
    // off on a TV)
    player(
        "pip",
        "player.pip_on_leave",
        choice(&PIP_ON_LEAVE, "auto"),
        key("Keep a channel playing full screen in a small window when you leave the app."),
    ),
    // default: ours (#179: off, the heard tile changes only by Listen to
    // this, iptv-core multiview)
    player(
        "multiview",
        "player.sound_follows_tile",
        boolean(false),
        key("In multiview, hear the tile you select instead of choosing Listen to this."),
    ),
];

pub static PLAYBACK: &[Setting] = &[
    // default: ours (#285; the play bar's channel buttons stay either way)
    playback(
        "switching",
        "playback.swipe_to_zap",
        boolean(true),
        key("Swipe up or down on full-screen live video to change channel."),
    ),
    // default: observed (07) — Hardware
    playback(
        "decoding",
        "playback.video_decoder",
        choice(&DECODER, "hardware"),
        key("Decode video in hardware, or in software when a device's decoder misbehaves."),
    )
    .scope(Scope::Group),
    // default: observed (07) — Hardware
    playback(
        "decoding",
        "playback.audio_decoder",
        choice(&DECODER, "hardware"),
        key("Decode audio with the platform's decoders or the bundled software ones."),
    )
    .scope(Scope::Group),
    // default: ours
    playback(
        "decoding",
        "playback.tunneling",
        boolean(false),
        key("Hand video straight to the display pipeline where the device supports it."),
    )
    .advanced(),
    // default: ours
    playback(
        "decoding",
        "playback.amlogic_workaround",
        boolean(false),
        key("Work around stutter and freezes seen with some Amlogic decoders."),
    )
    .advanced()
    .restart(),
    // default: ours
    playback(
        "decoding",
        "playback.black_frame_on_switch",
        boolean(true),
        key("Blank the picture while a new channel tunes instead of holding the old frame."),
    ),
    // default: ours
    playback(
        "audio",
        "playback.passthrough",
        boolean(false),
        key("Send compressed surround audio to the receiver undecoded."),
    ),
    // default: ours
    playback(
        "audio",
        "playback.prefer_surround",
        boolean(false),
        key("Pick a multichannel audio track first when a stream has one."),
    ),
    // default: ours
    playback(
        "audio",
        "playback.audio_offset",
        dur(TimeUnit::Millis, -2_000, 2_000, 10, 0),
        key("Delay (positive) or advance (negative) audio against video."),
    ),
    // default: observed (07) — Small
    playback(
        "buffering",
        "playback.buffer",
        choice(&SIZE, "small"),
        key("Buffer size: larger rides out network hiccups but makes switching slower."),
    ),
    // default: observed (07) — off
    playback(
        "frame_rate",
        "playback.afr",
        boolean(false),
        key("Change the display mode to match the content's frame rate; device support varies."),
    ),
    // default: ours
    playback(
        "frame_rate",
        "playback.afr_refresh_rate",
        boolean(true),
        key("Frame-rate matching may change the refresh rate."),
    )
    .depends(IF_AFR),
    // default: ours
    playback(
        "frame_rate",
        "playback.afr_resolution",
        boolean(false),
        key("Frame-rate matching may also change the resolution."),
    )
    .depends(IF_AFR),
    // default: ours
    playback(
        "frame_rate",
        "playback.afr_50_60_only",
        boolean(false),
        key("Only switch refresh rate for 50 and 60 fps content."),
    )
    .depends(&[
        enabled_if(&[when("playback.afr", Test::IsTrue)]),
        enabled_if(&[when("playback.afr_refresh_rate", Test::IsTrue)]),
    ]),
    // default: observed (07) — not used
    playback(
        "external",
        "playback.external_player",
        choice(&EXTERNAL_PLAYER, "never"),
        key("Hand streams to another installed player: never, for movies and series only, or always."),
    )
    .scope(Scope::Group),
];

pub static CATCHUP: &[Setting] = &[
    // default: ours
    catchup(
        "source",
        "catchup.scheme",
        choice(&CATCHUP_SCHEME, "auto"),
        key("How replay URLs are built; auto follows what the playlist declares."),
    )
    .scope(Scope::Playlist),
    // default: ours
    catchup(
        "source",
        "catchup.archive_days",
        days(0, 30, 1, 0),
        key("Replay window to assume; 0 uses what the provider reports."),
    )
    .scope(Scope::Playlist),
    // default: ours
    catchup(
        "source",
        "catchup.start_offset",
        offset_hms(-3_600, 3_600, 1, 0),
        key("Correction added to a replayed program's start time."),
    )
    .scope(Scope::Playlist),
    // default: ours
    catchup(
        "seeking",
        "catchup.rewind_live_key",
        choice(&REWIND_LIVE_KEY, "rewind"),
        key("Which key starts rewinding a live channel that has replay."),
    ),
    // default: ours
    catchup(
        "seeking",
        "catchup.seek_keys",
        choice(&SEEK_KEYS, "left_right"),
        key("Which keys seek while replaying: Left/Right, Down/Up, or the transport keys."),
    ),
    // default: ours
    catchup(
        "advanced",
        "catchup.forward_step",
        secs(5, 600, 5, 30),
        key("Jump made by one fast-forward press."),
    ),
    // default: ours
    catchup(
        "advanced",
        "catchup.rewind_step",
        secs(5, 600, 5, 10),
        key("Jump made by one rewind press."),
    ),
    // default: ours
    catchup(
        "steps",
        "catchup.timebar_step",
        mins(1, 60, 1, 5),
        key("Jump made by one step along the time bar, both directions."),
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The tables are built at compile time; this runs the helpers to
    /// check that each files its settings under its own section and adds
    /// nothing the row does not ask for.
    #[test]
    fn helpers_build_plain_settings_in_their_section() {
        let ty = boolean(true);
        let built = [
            (player("clock", "player.x", ty, "S."), Section::Player),
            (playback("", "playback.x", ty, "S."), Section::Playback),
            (catchup("steps", "catchup.x", ty, "S."), Section::Catchup),
        ];
        for (s, section) in built {
            assert_eq!(s.section, section);
            assert_eq!((s.scope, s.flags), (Scope::Global, Flags::default()));
            assert!(s.depends.is_empty());
            assert_eq!(s.ty, ty);
        }
        let subsections = built.map(|(s, _)| s.subsection);
        assert_eq!(subsections, [Some("clock"), None, Some("steps")]);
    }
}
