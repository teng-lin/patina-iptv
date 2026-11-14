package com.hantekllc.iptvrs.host

import androidx.media3.common.Player

/*
 * The `tv.player` wire contract on this host: the props the app sets and the
 * events it is sent, typed. The contract is declared once, in Rust
 * (`crates/iptv-player-mpv/src/props.rs` and `events.rs`), which generates
 * every name its side uses from that declaration; this file is the one place
 * the names are written here. The fixtures in
 * `crates/iptv-player-mpv/fixtures` hold both sides to the same names: the
 * Rust tests read and write them exactly, and `PlayerContractTest` hands
 * every fixture prop to [PlayerProps.setProp] (an unknown one fails it) and
 * compares every fixture event with a [PlayerEvent].
 */

/** The `source` prop: the stream to play. */
internal data class PlayerSource(
    /**
     * Null when the prop is an object without an address: nothing plays,
     * as for no source, but the object's other fields are still the
     * player's (its token, headers, `live` and `publicOnly`).
     */
    val url: String?,
    /** Only public addresses may be loaded (the default when unset). */
    val publicOnly: Boolean,
    val headers: Map<String, String>,
    /** A new token reopens the same url. */
    val token: Long,
    /** A live channel, not catch-up. */
    val live: Boolean,
    /** The app reopens the stream that played: its last frame stays. */
    val reconnect: Boolean,
) {
    companion object {
        /** The prop's value; null for no source (JSON null, or no object). */
        fun from(value: Any?): PlayerSource? {
            val map = value as? Map<*, *> ?: return null
            val headers =
                (map["headers"] as? Map<*, *>)
                    ?.entries
                    ?.mapNotNull { (k, v) -> (k as? String)?.let { key -> (v as? String)?.let { key to it } } }
                    ?.toMap()
                    ?: emptyMap()
            return PlayerSource(
                url = map["url"] as? String,
                publicOnly = map["publicOnly"] != false,
                headers = headers,
                token = (map["token"] as? Number)?.toLong() ?: 0L,
                live = map["live"] == true,
                reconnect = map["reconnect"] == true,
            )
        }
    }
}

/** The `pip` prop: picture in picture, for the main player. */
internal data class PlayerPip(
    /** Enter when the viewer leaves the app. */
    val auto: Boolean,
    /** A count of the app's requests to enter: a higher one is a new request. */
    val enter: Long,
    /** The desktops' picture-in-picture window shows the player (not used here). */
    val window: Boolean,
) {
    companion object {
        /** The prop's value; null for a player that is not the main one. */
        fun from(value: Any?): PlayerPip? {
            val map = value as? Map<*, *> ?: return null
            return PlayerPip(
                auto = map["auto"] == true,
                enter = (map["enter"] as? Number)?.toLong() ?: 0L,
                window = map["window"] == true,
            )
        }
    }
}

/**
 * The props of `tv.player`, one typed setter each. [setProp] is the only
 * place a prop's name is matched: a player implements the setters, and a
 * prop the contract gains needs both a setter and a name here before the
 * shared fixture passes.
 */
internal interface PlayerProps {
    fun source(source: PlayerSource?)

    fun paused(paused: Boolean)

    /** "normal", "full", "zoom", "cinema", "reduced", "4:3", "16:9" or "2.35:1". */
    fun pictureSize(mode: String)

    /** "auto" or a video track id. */
    fun videoTrack(id: String)

    /** "auto" or an audio track id. */
    fun audioTrack(id: String)

    /** "off" or a text track id. */
    fun textTrack(id: String)

    /** 0–100: the desktops' volume. */
    fun volume(level: Int)

    fun muted(muted: Boolean)

    /** In ms; positive: the sound later. */
    fun audioDelay(ms: Long)

    fun pip(pip: PlayerPip?)

    /**
     * Prop [name] changed to [value] (JSON as Java objects, or null when
     * removed): decodes it and calls its setter. False for a name the
     * contract does not have; nothing is set then.
     */
    fun setProp(
        name: String,
        value: Any?,
    ): Boolean {
        when (name) {
            "source" -> source(PlayerSource.from(value))
            "paused" -> paused(value == true)
            "pictureSize" -> pictureSize((value as? String) ?: "normal")
            "videoTrack" -> videoTrack((value as? String) ?: PlayerTracks.AUTO)
            "audioTrack" -> audioTrack((value as? String) ?: PlayerTracks.AUTO)
            "textTrack" -> textTrack((value as? String) ?: PlayerTracks.OFF)
            "volume" -> volume((value as? Number)?.toInt() ?: 100)
            "muted" -> muted(value == true)
            "audioDelay" -> audioDelay((value as? Number)?.toLong() ?: 0L)
            "pip" -> pip(PlayerPip.from(value))
            else -> return false
        }
        return true
    }
}

/** The `state` event's states. */
internal enum class PlayerState(
    val wire: String,
) {
    IDLE("idle"),
    BUFFERING("buffering"),
    READY("ready"),
    ENDED("ended"),

    /**
     * No state of the contract: what this host has always said of a Media3
     * state outside the four above (Media3 has none today). The app shows
     * the name on its status line and does nothing else with it
     * (`PlayerEventError::UnknownState` in the Rust contract).
     */
    UNKNOWN("unknown"),
    ;

    companion object {
        /** Media3's playback state ([Player.getPlaybackState]). */
        fun of(media3: Int): PlayerState =
            when (media3) {
                Player.STATE_IDLE -> IDLE
                Player.STATE_BUFFERING -> BUFFERING
                Player.STATE_READY -> READY
                Player.STATE_ENDED -> ENDED
                else -> UNKNOWN
            }
    }
}

/** One button of the picture-in-picture window (the `pipAction` event). */
internal enum class PipButton(
    val wire: String,
) {
    PLAY("play"),
    PAUSE("pause"),
    NEXT("next"),
    PREVIOUS("previous"),
    ;

    companion object {
        fun of(wire: String?): PipButton? = entries.firstOrNull { it.wire == wire }
    }
}

/**
 * One event of `tv.player`: its [name] on the wire and its [data] (JSON as
 * Java objects), as `PlayerEvent::decode` in the Rust contract reads them.
 */
internal sealed class PlayerEvent(
    val name: String,
) {
    abstract fun data(): Map<String, Any?>

    data class State(
        val state: PlayerState,
    ) : PlayerEvent("state") {
        override fun data(): Map<String, Any?> = mapOf("state" to state.wire)
    }

    /** The video's size in pixels. */
    data class Size(
        val width: Int,
        val height: Int,
    ) : PlayerEvent("size") {
        override fun data(): Map<String, Any?> = mapOf("width" to width, "height" to height)
    }

    /** Media3's error code name, and the HTTP status behind it if a server answered one. */
    data class Error(
        val code: String,
        val http: Int? = null,
    ) : PlayerEvent("error") {
        override fun data(): Map<String, Any?> {
            val data = mutableMapOf<String, Any?>("code" to code)
            http?.let { data["http"] = it }
            return data
        }
    }

    /** Where the stream is, from where it opened. */
    data class Position(
        val ms: Long,
    ) : PlayerEvent("position") {
        override fun data(): Map<String, Any?> = mapOf("ms" to ms)
    }

    /** The open stream's tracks and what plays; a null number is unknown. */
    data class Tracks(
        val video: List<Video>,
        val audio: List<Audio>,
        val text: List<Text>,
        /** "auto" or a video track id. */
        val selectedVideo: String,
        val selectedAudio: String?,
        /** "off" or a text track id. */
        val selectedText: String,
    ) : PlayerEvent("tracks") {
        data class Video(
            val id: String,
            val label: String,
            val width: Int?,
            val height: Int?,
            val fps: Float?,
            val bitrate: Int?,
        )

        data class Audio(
            val id: String,
            val label: String,
            val language: String,
            val channels: Int?,
            val codec: String,
        )

        data class Text(
            val id: String,
            val label: String,
            val language: String,
        )

        override fun data(): Map<String, Any?> =
            mapOf(
                "video" to
                    video.map {
                        mapOf(
                            "id" to it.id,
                            "label" to it.label,
                            "width" to it.width,
                            "height" to it.height,
                            "fps" to it.fps,
                            "bitrate" to it.bitrate,
                        )
                    },
                "audio" to
                    audio.map {
                        mapOf(
                            "id" to it.id,
                            "label" to it.label,
                            "language" to it.language,
                            "channels" to it.channels,
                            "codec" to it.codec,
                        )
                    },
                "text" to text.map { mapOf("id" to it.id, "label" to it.label, "language" to it.language) },
                "selected" to mapOf("video" to selectedVideo, "audio" to selectedAudio, "text" to selectedText),
            )
    }

    /** What plays, as far as known: a null field is left out. */
    data class Format(
        val fps: Float?,
        val channels: Int?,
    ) : PlayerEvent("format") {
        /** Nothing is known: not worth an event. */
        val isEmpty: Boolean get() = fps == null && channels == null

        override fun data(): Map<String, Any?> {
            val data = mutableMapOf<String, Any?>()
            fps?.let { data["fps"] = it }
            channels?.let { data["channels"] = it }
            return data
        }
    }

    /** Picture in picture as the system has it. */
    data class Pip(
        val supported: Boolean,
        val tv: Boolean,
        val active: Boolean,
    ) : PlayerEvent("pip") {
        override fun data(): Map<String, Any?> = mapOf("supported" to supported, "tv" to tv, "active" to active)
    }

    data class PipAction(
        val action: String,
    ) : PlayerEvent("pipAction") {
        constructor(button: PipButton) : this(button.wire)

        override fun data(): Map<String, Any?> = mapOf("action" to action)
    }
}
