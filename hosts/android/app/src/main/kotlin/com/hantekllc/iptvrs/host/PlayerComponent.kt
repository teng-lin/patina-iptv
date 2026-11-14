package com.hantekllc.iptvrs.host

import android.content.Context
import android.graphics.Color
import android.net.Uri
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.view.Gravity
import android.view.SurfaceView
import android.view.View
import android.widget.FrameLayout
import androidx.annotation.OptIn
import androidx.media3.common.C
import androidx.media3.common.Format
import androidx.media3.common.MediaItem
import androidx.media3.common.MimeTypes
import androidx.media3.common.PlaybackException
import androidx.media3.common.Player
import androidx.media3.common.Tracks
import androidx.media3.common.VideoSize
import androidx.media3.common.text.CueGroup
import androidx.media3.common.util.UnstableApi
import androidx.media3.common.util.Util
import androidx.media3.datasource.DefaultHttpDataSource
import androidx.media3.datasource.HttpDataSource
import androidx.media3.exoplayer.DecoderReuseEvaluation
import androidx.media3.exoplayer.DefaultRenderersFactory
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.exoplayer.analytics.AnalyticsListener
import androidx.media3.exoplayer.audio.AudioSink
import androidx.media3.exoplayer.source.DefaultMediaSourceFactory
import androidx.media3.ui.SubtitleView
import com.longbridge.nsh.NshComponent
import com.longbridge.nsh.NshComponentContext

/**
 * `tv.player`: one persistent Media3 player on a SurfaceView, aspect-fit in a
 * black frame. The Rust app lays it out like any view and draws its UI above
 * it; changing layout never recreates the decoder.
 *
 * Props (their names and types are [PlayerProps]'s, the one place this
 * host matches them; `PlayerContract.kt` holds the contract):
 * - `source`: `{url, headers: {name: value}, token, live, reconnect}` or
 *   null (stops). A new url or token reopens the stream (the token forces a
 *   retry of the same url). The bounded transport confines identity headers
 *   to the exact original scheme/host/port, including redirects and segments.
 *   `live` (a live channel, not catch-up): a progressive live stream plays
 *   on across a connection the server closes ([start]). `reconnect`: the
 *   app reopens the stream that played (a retry), so the last frame stays
 *   instead of the black shutter; so does a new token for the same url.
 *   A source whose headers name no `User-Agent` is sent this player's own.
 * - `paused` (bool),
 * - `pictureSize` ("normal", "full", "zoom", "cinema", "reduced", "4:3",
 *   "16:9", "2.35:1"; sizes the surface by layout, never by transform).
 * - `videoTrack` ("auto" or a video track id), `audioTrack` ("auto" or an
 *   audio track id), `textTrack` ("off" or a text track id), for the open
 *   stream ([PlayerTracks]; a new source starts again from auto/auto/off).
 * - `muted` (bool; the volume is the system's: `volume` is ignored here),
 *   `audioDelay` (ms, positive: the sound later; [DelayedAudioSink]).
 * - `pip` (the main player only): picture in picture, [PipHost], which
 *   also emits its `pip` and `pipAction` events through this component.
 *
 * Events (each a [PlayerEvent]): `state` {state:
 * idle|buffering|ready|ended}, `error` {code, http?}
 * (code is Media3's error code name, or `ERROR_CODE_UNSUPPORTED_SCHEME` for
 * a load outside the relay, [RelayGuard]; no URLs or messages), `size` {width,
 * height}, `position` {ms} (from where the stream opened; once a second
 * while playing), `tracks` (whenever they change; [PlayerTracks.event])
 * and `format` {fps?, channels?} for what plays. Subtitles of the chosen
 * text track show in a [SubtitleView] over the picture. Each component has
 * its own player and state (several may play at once: multiview mounts one
 * per tile, two on a TV box by default, and mutes all but the active one;
 * a component's [destroy] releases its decoders). Recovers by itself from
 * BEHIND_LIVE_WINDOW (back to the default position), retries once with
 * the other kind of stream (a container it could not parse as HLS, a
 * playlist it could not parse as progressive), and reopens a live stream
 * that played once per 30 s after a network error or refusal
 * ([quickReopen]); everything else goes to the app, which owns the retry
 * policy. Loads retry network failures for about 15 s first
 * ([LiveLoadPolicy]), and live streams play at least 10 s behind their
 * live edge ([LIVE_MIN_OFFSET_MS]), so short outages play out of the
 * buffer. The picture is black from a new source until its first frame
 * (and after an error, except a live stream's that showed one); a
 * reconnect of the same stream keeps the last frame.
 *
 * In the background nothing plays or loads: the activity's stop calls
 * [background] (the player stops, closing its connections; a provider
 * counts each one against the account), its start [foreground], which
 * reopens the stream that was playing: live at the live edge, catch-up
 * where it was. The app is not told (no `state` while away; its stall
 * checks pause on `lifecycle()` instead); a source it sets meanwhile opens
 * on the start. No audio plays in the background.
 *
 * Live HLS needs nothing here: Media3 reloads a media playlist from the
 * address it was given (a provider's redirect to a tokenized address is
 * followed again on every reload, so an expired token never shows), not
 * from where the redirect led.
 */
@OptIn(UnstableApi::class)
class PlayerComponent(
    context: Context,
    private val nsh: NshComponentContext,
    /** Called once the component is destroyed (the activity's list of players). */
    private val onDestroyed: (PlayerComponent) -> Unit = {},
    /** The activity's picture in picture (props `pip`, `paused`; the video's aspect). */
    private val pip: PipHost? = null,
) : NshComponent {
    companion object {
        const val NAME = "tv.player"
        /** The `User-Agent` of a stream whose source names none. */
        private const val DEFAULT_UA = "iptv-rs/0.1 (Media3)"
        /**
         * A live item that played shorter than this before the server closed
         * it gets no further spare: its end goes to the app, which backs off.
         */
        private const val MIN_BRIDGED_MS = 5_000L
        /**
         * A live stream plays at least this far behind its live edge (HLS:
         * Media3's own offset, three segments, when that is more), so its
         * buffer rides out a short outage instead of stalling at once.
         */
        const val LIVE_MIN_OFFSET_MS = 10_000L
        /**
         * A live stream that played reopens itself after an error at most
         * once in this long ([quickReopen]); a second error in it goes to
         * the app, which backs off.
         */
        private const val QUICK_REOPEN_EVERY_MS = 30_000L
    }

    /** What [foreground] reopens. */
    private enum class Reopen { NONE, SAME, NEW }

    /** Headers for the current stream; read on loader threads. */

    private val surface = SurfaceView(context)
    /**
     * Black over the picture from a new source (or an error) until the
     * stream's first frame: the surface would keep the last channel's frame.
     */
    private val shutter = View(context).apply {
        setBackgroundColor(Color.BLACK)
        visibility = View.GONE
    }
    /** The chosen text track's cues, over the picture. */
    private val subtitles = SubtitleView(context).apply {
        setUserDefaultStyle()
        setUserDefaultTextSize()
    }
    private val root = PictureFrame(context).apply {
        setBackgroundColor(Color.BLACK)
        addView(surface, FrameLayout.LayoutParams(-1, -1, Gravity.CENTER))
        addView(subtitles, FrameLayout.LayoutParams(-1, -1, Gravity.CENTER))
        addView(shutter, FrameLayout.LayoutParams(-1, -1, Gravity.CENTER))
    }

    private var url: String? = null
    private var token: Long = 0
    /** The source is a live channel ([start]'s spare items). */
    private var live = false
    /** When the playing item started (a spare that closes too soon gets none). */
    private var itemStartedAt = 0L
    /** The current source showed a frame (its errors keep it: [listener]). */
    private var rendered = false
    /** When [quickReopen] last reopened the stream. */
    private var quickReopenAt = 0L
    private var sourceHeaders: Map<String, String> = emptyMap()
    private val transport = PlaybackTransport()
    private var publicOnly = true
    private var headers: Map<String, String> = emptyMap()
    /** Set after the one format fallback (HLS or progressive) for the current stream. */
    private var triedOtherFormat = false
    /** The type [start] gave the current stream (null: from its address). */
    private var mime: String? = null
    /** The activity is stopped: the player is too ([background]). */
    private var away = false
    /** What [foreground] reopens: the stream that played, or a new one set while away. */
    private var reopen = Reopen.NONE
    /** The viewer's track choices for the open stream. */
    private var choice = PlayerTracks.Choice()
    /** This player's audio sink (made when the player is built). */
    private var sink: DelayedAudioSink? = null
    private var audioDelayUs = 0L
    private val player: ExoPlayer

    private fun build(context: Context): ExoPlayer {
        // No factory User-Agent: it would override the per-stream one. Every
        // load goes through this player's relay, never straight out ([RelayGuard]).
        val http = RelayOnlyDataSource.Factory(
            DefaultHttpDataSource.Factory()
                .setUserAgent(null)
                .setAllowCrossProtocolRedirects(false)
                .setConnectTimeoutMs(8_000)
                .setReadTimeoutMs(15_000),
        ) { transport.prefix }
        // The audio sink carries the audio delay (one per player).
        val renderers = object : DefaultRenderersFactory(context) {
            override fun buildAudioSink(
                context: Context,
                enableFloatOutput: Boolean,
                enableAudioTrackPlaybackParams: Boolean,
            ): AudioSink? {
                val base = super.buildAudioSink(context, enableFloatOutput, enableAudioTrackPlaybackParams)
                    ?: return null
                return DelayedAudioSink(base).also {
                    it.delayUs = audioDelayUs
                    sink = it
                }
            }
        }
        return ExoPlayer.Builder(context, renderers)
            .setMediaSourceFactory(
                DefaultMediaSourceFactory(context)
                    .setDataSourceFactory(http)
                    .setLoadErrorHandlingPolicy(LiveLoadPolicy()),
            )
            .build()
            .also { p ->
                p.setVideoSurfaceView(surface)
                // Live streams that end are reconnected by the app.
                p.repeatMode = Player.REPEAT_MODE_OFF
                p.playWhenReady = true
                // Subtitles only when the viewer picks a text track.
                p.trackSelectionParameters = p.trackSelectionParameters.buildUpon()
                    .setTrackTypeDisabled(C.TRACK_TYPE_TEXT, true)
                    .build()
                p.addListener(listener)
                p.addAnalyticsListener(formats)
            }
    }

    /** The viewer's choices on the stream's current tracks. */
    private fun applyChoice() {
        val next = PlayerTracks.parameters(player.trackSelectionParameters, player.currentTracks, choice)
        if (next != player.trackSelectionParameters) player.trackSelectionParameters = next
    }

    private fun emitFormat(video: Format?, audio: Format?) {
        val format = PlayerTracks.format(video, audio)
        if (!format.isEmpty) emit(format)
    }

    /** Frame rate and audio channels as the decoders get them. */
    private val formats = object : AnalyticsListener {
        override fun onVideoInputFormatChanged(
            eventTime: AnalyticsListener.EventTime,
            format: Format,
            decoderReuseEvaluation: DecoderReuseEvaluation?,
        ) = emitFormat(format, null)

        override fun onAudioInputFormatChanged(
            eventTime: AnalyticsListener.EventTime,
            format: Format,
            decoderReuseEvaluation: DecoderReuseEvaluation?,
        ) = emitFormat(null, format)
    }

    /**
     * The screen stays on while this player means to play: set to play and
     * buffering or playing, so a live stream's rebuffers and the app's
     * reconnects keep it on, not only the frames. Paused, stopped, failed
     * or ended, the system's screen timeout applies again. (A view's
     * `keepScreenOn` holds the window's flag while the view is shown: the
     * guide's preview, a multiview tile, picture in picture.)
     */
    private fun keepScreen() {
        val state = player.playbackState
        surface.keepScreenOn = player.playWhenReady &&
            (state == Player.STATE_BUFFERING || state == Player.STATE_READY)
    }

    private val listener = object : Player.Listener {
        override fun onPlayWhenReadyChanged(playWhenReady: Boolean, reason: Int) = keepScreen()

        override fun onPlaybackStateChanged(state: Int) {
            keepScreen()
            // The stop of [background]: the app sees the stream as it was.
            if (away) return
            emit(PlayerEvent.State(PlayerState.of(state)))
        }

        override fun onRenderedFirstFrame() {
            transport.takeError()
            shutter.visibility = View.GONE
            rendered = true
            // Again for each stream: the app forgets the last one's.
            val size = player.videoSize
            if (size.width > 0 && size.height > 0) {
                emit(PlayerEvent.Size(size.width, size.height))
            }
            emitFormat(player.videoFormat, player.audioFormat)
        }

        override fun onMediaItemTransition(mediaItem: MediaItem?, reason: Int) {
            if (reason != Player.MEDIA_ITEM_TRANSITION_REASON_AUTO || mediaItem == null) return
            // The server closed a live stream and the spare plays on: drop
            // what played, and queue the next spare.
            val now = SystemClock.elapsedRealtime()
            val played = now - itemStartedAt
            itemStartedAt = now
            val index = player.currentMediaItemIndex
            if (index > 0) player.removeMediaItems(0, index)
            if (played >= MIN_BRIDGED_MS) player.addMediaItem(mediaItem)
        }

        override fun onTracksChanged(tracks: Tracks) {
            // Stopped in the background: the tracks come back with the stream.
            if (away) return
            applyChoice()
            emit(PlayerTracks.event(tracks, choice))
        }

        override fun onCues(cueGroup: CueGroup) {
            subtitles.setCues(cueGroup.cues)
        }

        override fun onVideoSizeChanged(size: VideoSize) {
            if (size.width > 0 && size.height > 0) {
                root.videoAspect = size.width * size.pixelWidthHeightRatio / size.height
                pip?.videoAspect(this@PlayerComponent, root.videoAspect)
                emit(PlayerEvent.Size(size.width, size.height))
            }
        }

        override fun onPlayerError(error: PlaybackException) {
            when {
                error.errorCode == PlaybackException.ERROR_CODE_BEHIND_LIVE_WINDOW -> {
                    player.seekToDefaultPosition()
                    player.prepare()
                    return
                }
                // The address's extension can lie (`.ts` serving HLS, `.m3u8`
                // serving MP4): one retry per source with the other kind.
                !triedOtherFormat && isContainerError(error.errorCode) -> {
                    triedOtherFormat = true
                    url?.let { start(it, MimeTypes.APPLICATION_M3U8) }
                    return
                }
                !triedOtherFormat &&
                    error.errorCode == PlaybackException.ERROR_CODE_PARSING_MANIFEST_MALFORMED -> {
                    triedOtherFormat = true
                    // Progressive: the extractors sniff the container.
                    url?.let { start(it, MimeTypes.VIDEO_UNKNOWN) }
                    return
                }
            }
            val transportError = transport.takeError()
            // A load outside the relay: as the desktops' players report an
            // address they may not open (no retry, no reopen).
            val refused = RelayGuard.refusedIn(error)
            val status = if (transportError == null && !refused) httpStatus(error) else null
            if (!refused && quickReopen(error, status)) return
            // A live stream keeps its last frame: the app reconnects it,
            // with "Reconnecting…" or its card over the frame.
            if (!live || !rendered) shutter.visibility = View.VISIBLE
            val code = if (refused) "ERROR_CODE_UNSUPPORTED_SCHEME" else transportError ?: error.errorCodeName
            emit(PlayerEvent.Error(code, status))
        }
    }

    /**
     * A live stream that played and fails on the network or with a refusal
     * (an expired token: [LiveLoadPolicy]) reopens through its original
     * address at once, keeping its last frame, without telling the app; at
     * most once per [QUICK_REOPEN_EVERY_MS]. Whether it did.
     */
    private fun quickReopen(error: PlaybackException, status: Int?): Boolean {
        val io = error.errorCode in 2000..2999
        val now = SystemClock.elapsedRealtime()
        if (!live || !rendered || !(io || LiveLoadPolicy.refused(status))) return false
        if (now - itemStartedAt < MIN_BRIDGED_MS || now - quickReopenAt < QUICK_REOPEN_EVERY_MS) {
            return false
        }
        quickReopenAt = now
        val target = url ?: return false
        start(target, mime)
        return true
    }

    /** Reports where the stream is once a second while it plays (catch-up seeking). */
    private val ticker = Handler(Looper.getMainLooper())
    private val tick = object : Runnable {
        override fun run() {
            if (player.isPlaying) {
                emit(PlayerEvent.Position(player.currentPosition.coerceAtLeast(0)))
            }
            ticker.postDelayed(this, 1_000)
        }
    }

    // After `listener` is initialised: build() registers it.
    init {
        player = build(context)
        ticker.postDelayed(tick, 1_000)
    }

    override fun view(): View = root

    /** The picture itself (the window's start for picture in picture). */
    internal val picture: View get() = surface

    /** Emits `event` to the app (this player's, and [PipHost]'s through it). */
    internal fun emit(event: PlayerEvent) = nsh.emit(event.name, event.data())

    /** What each prop of the contract does on this player. */
    private val props = object : PlayerProps {
        override fun source(source: PlayerSource?) = setSource(source)

        override fun paused(paused: Boolean) {
            player.playWhenReady = !paused
            this@PlayerComponent.pip?.paused(this@PlayerComponent, paused)
        }

        override fun pictureSize(mode: String) {
            root.mode = mode
        }

        override fun videoTrack(id: String) = choose(choice.copy(video = id))

        override fun audioTrack(id: String) = choose(choice.copy(audio = id))

        override fun textTrack(id: String) = choose(choice.copy(text = id))

        /** The volume is the system's here: the app's is the desktops'. */
        override fun volume(level: Int) {}

        override fun muted(muted: Boolean) {
            player.volume = if (muted) 0f else 1f
        }

        override fun audioDelay(ms: Long) {
            audioDelayUs = ms.coerceIn(-10_000L, 10_000L) * 1000
            sink?.delayUs = audioDelayUs
        }

        override fun pip(pip: PlayerPip?) {
            this@PlayerComponent.pip?.setProp(this@PlayerComponent, pip)
        }
    }

    /** A name outside the contract sets nothing ([PlayerProps.setProp]). */
    override fun setProp(name: String, value: Any?) {
        props.setProp(name, value)
    }

    private fun choose(next: PlayerTracks.Choice) {
        if (next == choice) return
        choice = next
        applyChoice()
        emit(PlayerTracks.event(player.currentTracks, choice))
    }

    private fun setSource(source: PlayerSource?) {
        val next = source?.url
        val nextToken = source?.token ?: 0L
        live = source?.live == true
        // The same stream again (a retry): keep its last frame.
        val keepFrame = next != null && (next == url || source?.reconnect == true)
        val nextPublicOnly = source?.publicOnly != false
        val changed = next != url || nextToken != token || nextPublicOnly != publicOnly
        if (changed) transport.close()
        sourceHeaders = source?.headers ?: emptyMap()
        url = next
        publicOnly = nextPublicOnly
        token = nextToken
        applyHeaders()
        if (!changed) return
        triedOtherFormat = false
        if (!keepFrame) rendered = false
        // A new stream: default tracks, no old cues.
        choice = PlayerTracks.Choice()
        applyChoice()
        subtitles.setCues(emptyList())
        if (!keepFrame) shutter.visibility = View.VISIBLE
        reopen = Reopen.NONE
        if (next == null) {
            player.stop()
            player.clearMediaItems()
        } else if (away) {
            // Opens on the activity's start ([foreground]).
            reopen = Reopen.NEW
        } else {
            start(next, null)
        }
    }

    /**
     * The activity stopped: stops the player, which closes the stream's
     * connections (a pause would keep a live stream loading), and keeps what
     * to reopen. The playlist and position stay ([foreground]).
     */
    fun background() {
        if (away) return
        away = true
        ticker.removeCallbacks(tick)
        val active = player.playbackState == Player.STATE_BUFFERING ||
            player.playbackState == Player.STATE_READY
        // A failed or finished stream stays as it is: the app owns its retry.
        if (reopen == Reopen.NONE && active) reopen = Reopen.SAME
        player.stop()
    }

    /**
     * The activity started again: reopens what [background] stopped (live
     * at the live edge, catch-up at its position) or the source the app set
     * meanwhile.
     */
    fun foreground() {
        if (!away) return
        away = false
        ticker.postDelayed(tick, 1_000)
        val what = reopen
        reopen = Reopen.NONE
        val target = url ?: return
        when {
            what == Reopen.NEW -> start(target, null)
            what == Reopen.SAME && live -> start(target, mime)
            what == Reopen.SAME -> player.prepare()
        }
    }

    private fun applyHeaders() {
        val all = LinkedHashMap(sourceHeaders)
        if (all.keys.none { it.equals("User-Agent", ignoreCase = true) }) {
            all["User-Agent"] = DEFAULT_UA
        }
        headers = all
        try {
            transport.update(all)
        } catch (_: java.io.IOException) {
            transport.close()
            player.stop()
            emit(PlayerEvent.Error("ERROR_CODE_IO_UNSPECIFIED"))
        }
    }

    /**
     * Plays `target`. A live progressive stream (MPEG-TS over HTTP) ends when
     * the server closes the connection, which providers do every minute or
     * so: it plays as two items of the same address, and once the first is
     * fully read the player opens the second while the buffered rest plays,
     * then switches without a gap ([listener]'s `onMediaItemTransition`
     * queues the next spare).
     */
    private fun start(target: String, mime: String?) {
        val local = try {
            transport.open(target, headers, publicOnly)
        } catch (_: java.io.IOException) {
            player.stop()
            emit(PlayerEvent.Error("ERROR_CODE_IO_UNSPECIFIED"))
            return
        }
        val builder = MediaItem.Builder().setUri(local)
        if (mime != null) builder.setMimeType(mime)
        if (live) {
            // Behind live, never at the edge ([LIVE_MIN_OFFSET_MS]); the
            // first frame still comes from the first segment loaded.
            builder.setLiveConfiguration(
                MediaItem.LiveConfiguration.Builder().setMinOffsetMs(LIVE_MIN_OFFSET_MS).build(),
            )
        }
        val item = builder.build()
        this.mime = mime
        val progressive =
            Util.inferContentTypeForUriAndMimeType(Uri.parse(target), mime) == C.CONTENT_TYPE_OTHER
        if (live && progressive) {
            player.setMediaItems(listOf(item, item))
        } else {
            player.setMediaItem(item)
        }
        itemStartedAt = SystemClock.elapsedRealtime()
        player.prepare()
    }

    override fun destroy() {
        ticker.removeCallbacks(tick)
        player.removeListener(listener)
        player.removeAnalyticsListener(formats)
        surface.keepScreenOn = false
        player.release()
        transport.close()
        onDestroyed(this)
    }
}

private fun isContainerError(code: Int) =
    code == PlaybackException.ERROR_CODE_PARSING_CONTAINER_UNSUPPORTED ||
        code == PlaybackException.ERROR_CODE_PARSING_CONTAINER_MALFORMED

/** The HTTP status behind a playback error, if a server answered one. */
private fun httpStatus(error: PlaybackException): Int? {
    var cause: Throwable? = error.cause
    while (cause != null) {
        if (cause is HttpDataSource.InvalidResponseCodeException) return cause.responseCode
        cause = cause.cause
    }
    return null
}
