package com.hantekllc.iptvrs.host

import android.app.Activity
import android.app.PendingIntent
import android.app.PictureInPictureParams
import android.app.RemoteAction
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.graphics.Rect
import android.graphics.drawable.Icon
import android.os.Build
import android.util.Log
import android.util.Rational
import android.view.View
import kotlin.math.roundToInt

/**
 * Picture in picture for [TvActivity]: the app's main player (surface 0,
 * the one given a `pip` prop) in the system's small window over other
 * apps. The activity is only paused there, not stopped, so its players
 * play on ([TvActivity.onStop] stops them: closing the window stops the
 * activity, and so playback).
 *
 * The app's side (`crates/iptv-app/src/pip.rs`), through that player:
 * - prop `pip` `{auto, enter}`: `auto`, whether leaving the activity (Home,
 *   recents) opens the window now (Android 12+: the system's auto-enter,
 *   except on a TV; before, and on a TV: [userLeaving]); a new `enter`
 *   count opens it now ([enter]);
 * - event `pip` `{supported, tv, active}` once the prop first arrives and
 *   whenever the window opens or closes;
 * - event `pipAction` `{action: play|pause|next|previous}`: the window's
 *   buttons. Play/pause shows what the `paused` prop says.
 *
 * The window takes the stream's aspect ratio (within the system's limits)
 * and the video's place on screen as the start of its animation.
 */
class PipHost(
    private val activity: Activity,
) {
    private val supported =
        activity.packageManager
            .hasSystemFeature(PackageManager.FEATURE_PICTURE_IN_PICTURE)
    private val tv = activity.packageManager.hasSystemFeature(PackageManager.FEATURE_LEANBACK)

    /** The window is open. */
    private var active = false

    /** The player the window shows. */
    private var main: PlayerComponent? = null
    private var auto = false

    /** The last `enter` count seen. */
    private var asked = 0L
    private var aspect = Rational(16, 9)
    private var paused = false

    /** What the system was last given. */
    private var given: Given? = null

    private data class Given(
        val auto: Boolean,
        val aspect: Rational,
        val paused: Boolean,
        val hint: Rect?,
    )

    /** The video moved or changed size: the window's start follows it. */
    private val moved = View.OnLayoutChangeListener { _, _, _, _, _, _, _, _, _ -> update() }
    private var listening = false

    /** `player`'s `pip` prop: null for every player but the main one. */
    internal fun setProp(
        player: PlayerComponent,
        pip: PlayerPip?,
    ) {
        if (pip == null) {
            if (main === player) main = null
            return
        }
        val enter = pip.enter
        if (main !== player) {
            main = player
            // A count from before this player is no request.
            asked = enter
            player.picture.post { report() }
            player.view().addOnLayoutChangeListener(moved)
            player.picture.addOnLayoutChangeListener(moved)
        }
        auto = pip.auto
        val wanted = enter > asked
        asked = maxOf(asked, enter)
        update()
        if (wanted) enter()
    }

    /** `player`'s `paused` prop (the window's play/pause button). */
    fun paused(
        player: PlayerComponent,
        paused: Boolean,
    ) {
        if (player !== main) return
        this.paused = paused
        update()
    }

    /** `player`'s video has aspect `ratio` (width / height, pixels' shape included). */
    fun videoAspect(
        player: PlayerComponent,
        ratio: Float,
    ) {
        if (player !== main || ratio <= 0f) return
        // The system's limits: between 1:2.39 and 2.39:1.
        val r = ratio.coerceIn(1f / MAX_ASPECT, MAX_ASPECT)
        aspect = Rational((r * 1000).roundToInt(), 1000)
        update()
    }

    fun destroyed(player: PlayerComponent) {
        if (player !== main) return
        player.view().removeOnLayoutChangeListener(moved)
        player.picture.removeOnLayoutChangeListener(moved)
        main = null
    }

    /** The app asked for the window ([setProp]'s `enter`). */
    private fun enter() {
        if (!supported || active || main == null) return
        try {
            if (!activity.enterPictureInPictureMode(params())) Log.i(TAG, "iptv pip: not entered")
        } catch (e: IllegalStateException) {
            Log.w(TAG, "iptv pip: cannot enter (${e.javaClass.simpleName})")
        }
    }

    /**
     * [Activity.onUserLeaveHint]: the viewer is leaving (Home). Where the
     * system does not enter by itself ([update]), enters when `auto` says.
     */
    fun userLeaving() {
        if (auto && !systemAutoEnter()) enter()
    }

    /** Android 12+ enters by itself on leaving; a TV launcher's Home is left to [userLeaving]. */
    private fun systemAutoEnter() = Build.VERSION.SDK_INT >= 31 && !tv

    /** [Activity.onPictureInPictureModeChanged]. */
    fun modeChanged(inPip: Boolean) {
        if (inPip == active) return
        active = inPip
        Log.i(TAG, "iptv pip: ${if (inPip) "entered" else "left"}")
        listen(inPip)
        report()
    }

    private fun report() {
        // Also for an activity that starts in the window (recreated there).
        active = activity.isInPictureInPictureMode
        listen(active)
        main?.emit(PlayerEvent.Pip(supported, tv, active))
    }

    /** Gives the system what the window would show, when it changed. */
    private fun update() {
        if (!supported || main == null) return
        val now = Given(auto, aspect, paused, hint())
        if (now == given) return
        given = now
        try {
            activity.setPictureInPictureParams(params())
        } catch (e: IllegalArgumentException) {
            Log.w(TAG, "iptv pip: parameters refused (${e.javaClass.simpleName})")
        } catch (e: IllegalStateException) {
            Log.w(TAG, "iptv pip: parameters refused (${e.javaClass.simpleName})")
        }
    }

    private fun params(): PictureInPictureParams {
        val b =
            PictureInPictureParams
                .Builder()
                .setAspectRatio(aspect)
                .setActions(actions())
        hint()?.let { b.setSourceRectHint(it) }
        if (Build.VERSION.SDK_INT >= 31) {
            b.setAutoEnterEnabled(auto && systemAutoEnter())
            // Video: no cross-fade of the content while the window resizes.
            b.setSeamlessResizeEnabled(false)
        }
        return b.build()
    }

    /** Where the picture is in the window, if it shows. */
    private fun hint(): Rect? {
        val r = Rect()
        return r.takeIf { main?.picture?.getGlobalVisibleRect(it) == true && !it.isEmpty }
    }

    private fun actions(): List<RemoteAction> =
        listOf(
            action(PipButton.PREVIOUS, R.drawable.pip_previous, "Previous channel", 1),
            if (paused) {
                action(PipButton.PLAY, R.drawable.pip_play, "Play", 2)
            } else {
                action(PipButton.PAUSE, R.drawable.pip_pause, "Pause", 2)
            },
            action(PipButton.NEXT, R.drawable.pip_next, "Next channel", 3),
        )

    private fun action(
        button: PipButton,
        icon: Int,
        title: String,
        code: Int,
    ): RemoteAction {
        val intent = Intent(ACTION).setPackage(activity.packageName).putExtra(EXTRA, button.wire)
        val flags = PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        val pending = PendingIntent.getBroadcast(activity, code, intent, flags)
        return RemoteAction(Icon.createWithResource(activity, icon), title, title, pending)
    }

    /** The window's buttons, while it is open. */
    private val receiver =
        object : BroadcastReceiver() {
            override fun onReceive(
                context: Context,
                intent: Intent,
            ) {
                val action = intent.getStringExtra(EXTRA) ?: return
                main?.emit(PlayerEvent.PipAction(action))
            }
        }

    private fun listen(on: Boolean) {
        if (on == listening) return
        listening = on
        if (!on) {
            activity.unregisterReceiver(receiver)
        } else {
            androidx.core.content.ContextCompat.registerReceiver(
                activity,
                receiver,
                IntentFilter(ACTION),
                androidx.core.content.ContextCompat.RECEIVER_NOT_EXPORTED,
            )
        }
    }

    /** The activity is going: no more buttons. */
    fun destroy() = listen(false)

    private companion object {
        const val TAG = "nsh"
        const val ACTION = "com.hantekllc.iptvrs.PIP_ACTION"
        const val EXTRA = "action"
        const val MAX_ASPECT = 2.39f
    }
}
