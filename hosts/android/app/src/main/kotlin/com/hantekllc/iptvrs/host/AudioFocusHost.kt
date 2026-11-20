package com.hantekllc.iptvrs.host

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.util.Log
import androidx.core.content.ContextCompat

/**
 * The activity's one audio focus, for all its players ([PlayerComponent]):
 * multiview mounts several, and each asking for the focus would take it
 * from the others. Held while any player means to play aloud (a source,
 * set to play, not muted, the activity started: picture in picture
 * included); given up once none does, unless a call's pause is waiting for
 * the focus to come back.
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

    /** The players that mean to play aloud now. */
    private val audible = LinkedHashSet<PlayerComponent>()

    /** The mounted players: each hears the changes (the app reads the main one's). */
    private val players = LinkedHashSet<PlayerComponent>()

    private var held = false

    /** A transient loss is waiting for the focus to come back. */
    private var waiting = false

    private val listener =
        AudioManager.OnAudioFocusChangeListener { change ->
            when (change) {
                AudioManager.AUDIOFOCUS_LOSS -> {
                    waiting = false
                    report(PlayerEvent.Audio(AudioChange.FOCUS_LOSS))
                    abandon()
                }
                AudioManager.AUDIOFOCUS_LOSS_TRANSIENT -> {
                    waiting = true
                    report(PlayerEvent.Audio(AudioChange.FOCUS_LOSS_TRANSIENT))
                }
                AudioManager.AUDIOFOCUS_GAIN -> {
                    if (waiting) {
                        waiting = false
                        report(PlayerEvent.Audio(AudioChange.FOCUS_GAIN))
                    }
                }
                // Ducking is the system's (no will-pause-when-ducked).
                else -> {}
            }
            settle()
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
    }

    /** `player` means to play aloud now, or no longer does. */
    fun audible(
        player: PlayerComponent,
        aloud: Boolean,
    ) {
        val changed = if (aloud) audible.add(player) else audible.remove(player)
        if (changed) settle()
    }

    fun destroyed(player: PlayerComponent) {
        players.remove(player)
        audible(player, false)
    }

    /** Takes or gives up the focus as the players need it. */
    private fun settle() {
        val wanted = audible.isNotEmpty()
        if (wanted && !held) {
            when (manager.requestAudioFocus(request)) {
                AudioManager.AUDIOFOCUS_REQUEST_GRANTED -> held = true
                // A call is on: as a call's pause, until the system grants it.
                AudioManager.AUDIOFOCUS_REQUEST_DELAYED -> {
                    held = true
                    waiting = true
                    report(PlayerEvent.Audio(AudioChange.FOCUS_LOSS_TRANSIENT))
                }
                // Refused outright: playback goes on as before, unmanaged.
                else -> held = false
            }
            Log.i(TAG, "iptv audio: focus ${if (held) "requested" else "refused"}")
        } else if (!wanted && held && !waiting) {
            abandon()
        }
        listen(held)
    }

    private fun abandon() {
        if (!held) return
        manager.abandonAudioFocusRequest(request)
        held = false
        Log.i(TAG, "iptv audio: focus given up")
        listen(false)
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

    /** The activity is going. */
    fun destroy() {
        waiting = false
        abandon()
        listen(false)
    }

    private companion object {
        const val TAG = "nsh"
    }
}
