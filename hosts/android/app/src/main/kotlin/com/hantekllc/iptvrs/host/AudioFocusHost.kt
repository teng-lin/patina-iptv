package com.hantekllc.iptvrs.host

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.os.Handler
import android.os.Looper
import android.util.Log
import androidx.core.content.ContextCompat

/**
 * The activity's one audio focus, for all its players ([PlayerComponent]):
 * multiview mounts several, and each asking for the focus would take it
 * from the others. When it is held and when it is given up is
 * [AudioFocusState]'s: while a player means to play aloud (picture in
 * picture included), never taken back from another app until a new play,
 * kept through a call while a stream is open.
 *
 * What happens to the focus and the sound's route goes to the app as the
 * players' `audio` event `{change}` ([PlayerEvent.Audio]); the app's policy
 * (`iptv_core::interruption`) pauses or resumes through the `paused` prop.
 * This host never pauses, resumes, stops or reopens a stream itself:
 * - `focusLoss`: another app took the focus for good (the focus is given
 *   up: it never comes back by itself);
 * - `focusLossTransient`: a call or a prompt took it for a while;
 * - `focusGain`: it came back;
 * - `noisy`: the sound's route went away (headphones unplugged, a
 *   Bluetooth headset gone), [AudioManager.ACTION_AUDIO_BECOMING_NOISY].
 * Ducking (a notification's sound) is the system's: the players keep
 * playing, quieter.
 */
class AudioFocusHost(
    private val context: Context,
) {
    private val manager = context.getSystemService(AudioManager::class.java)

    private val state = AudioFocusState()

    /** The mounted players: each hears the changes (the app applies them once). */
    private val players = LinkedHashSet<PlayerComponent>()

    private val main = Handler(Looper.getMainLooper())

    /** The app's decision after the focus came back had its time. */
    private val decided = Runnable { apply(state.decided()) }

    private val listener =
        AudioManager.OnAudioFocusChangeListener { change ->
            when (change) {
                AudioManager.AUDIOFOCUS_LOSS -> {
                    report(PlayerEvent.Audio(AudioChange.FOCUS_LOSS))
                    apply(state.lost())
                }
                AudioManager.AUDIOFOCUS_LOSS_TRANSIENT -> {
                    state.lostForAWhile()
                    silence()
                    report(PlayerEvent.Audio(AudioChange.FOCUS_LOSS_TRANSIENT))
                }
                AudioManager.AUDIOFOCUS_GAIN -> {
                    if (state.gained()) {
                        silence()
                        report(PlayerEvent.Audio(AudioChange.FOCUS_GAIN))
                        main.removeCallbacks(decided)
                        main.postDelayed(decided, DECISION_MS)
                    }
                }
                // Ducking is the system's (no will-pause-when-ducked).
                else -> {}
            }
        }

    private val request =
        AudioFocusRequest
            .Builder(AudioManager.AUDIOFOCUS_GAIN)
            .setAudioAttributes(
                AudioAttributes
                    .Builder()
                    .setUsage(AudioAttributes.USAGE_MEDIA)
                    .setContentType(AudioAttributes.CONTENT_TYPE_MOVIE)
                    .build(),
            ).setAcceptsDelayedFocusGain(true)
            .setOnAudioFocusChangeListener(listener)
            .build()

    private val noisy =
        object : BroadcastReceiver() {
            override fun onReceive(
                context: Context,
                intent: Intent,
            ) {
                if (intent.action == AudioManager.ACTION_AUDIO_BECOMING_NOISY) {
                    Log.i(TAG, "iptv audio: route gone")
                    report(PlayerEvent.Audio(AudioChange.NOISY))
                }
            }
        }
    private var listening = false

    fun mounted(player: PlayerComponent) {
        players.add(player)
        player.silence(state.silenced)
    }

    /**
     * `player` plays aloud now (or not), and has an open stream (a source,
     * buffering or ready, the activity started) or not.
     */
    fun player(
        player: PlayerComponent,
        aloud: Boolean,
        open: Boolean,
    ) = apply(state.player(player, aloud, open))

    fun destroyed(player: PlayerComponent) {
        players.remove(player)
        apply(state.removed(player))
    }

    /** Does what [AudioFocusState] says with the system. */
    private fun apply(action: AudioFocusState.Do) {
        when (action) {
            AudioFocusState.Do.REQUEST -> {
                val result = manager.requestAudioFocus(request)
                val delayed = result == AudioManager.AUDIOFOCUS_REQUEST_DELAYED
                state.requested(result == AudioManager.AUDIOFOCUS_REQUEST_GRANTED, delayed)
                Log.i(TAG, "iptv audio: focus ${if (state.held) "requested" else "refused"}")
                // A call is on: as a call's pause, until the system grants it.
                if (delayed) report(PlayerEvent.Audio(AudioChange.FOCUS_LOSS_TRANSIENT))
            }
            AudioFocusState.Do.ABANDON -> {
                manager.abandonAudioFocusRequest(request)
                Log.i(TAG, "iptv audio: focus given up")
            }
            AudioFocusState.Do.NOTHING -> {}
        }
        listen(state.held)
        silence()
    }

    private fun listen(on: Boolean) {
        if (on == listening) return
        listening = on
        if (on) {
            ContextCompat.registerReceiver(
                context,
                noisy,
                IntentFilter(AudioManager.ACTION_AUDIO_BECOMING_NOISY),
                ContextCompat.RECEIVER_NOT_EXPORTED,
            )
        } else {
            context.unregisterReceiver(noisy)
        }
    }

    private fun report(event: PlayerEvent.Audio) {
        Log.i(TAG, "iptv audio: ${event.change.wire}")
        for (p in players.toList()) p.emit(event)
    }

    /**
     * Every player is silent while a call holds the focus
     * ([AudioFocusState.silenced]): a tile chosen meanwhile plays no sound
     * over it, and the app's pauses stay as they are.
     */
    private fun silence() {
        val on = state.silenced
        for (p in players.toList()) p.silence(on)
    }

    /** The activity is going. */
    fun destroy() {
        main.removeCallbacks(decided)
        apply(state.destroyed())
        listen(false)
    }

    private companion object {
        const val TAG = "nsh"

        /** How long the app has to resume after the focus came back. */
        const val DECISION_MS = 2_000L
    }
}
