package com.hantekllc.iptvrs.host

import android.app.Activity
import android.content.Intent
import android.content.res.Configuration
import android.os.Build
import android.os.Bundle
import android.os.Process
import android.os.SystemClock
import android.util.Log
import android.view.KeyEvent
import android.view.View
import android.view.WindowInsets
import android.view.WindowInsetsController
import com.longbridge.nsh.DevActivity
import com.longbridge.nsh.NshView

/**
 * The Android host: mirrors NSH's AppActivity for a Rust app, and registers
 * the app's native components before the Rust root mounts. Its players stop
 * while it is stopped (Home, another app, the screen off) and reopen when
 * it starts again ([PlayerComponent.background]); nothing plays in the
 * background. It runs full screen (immersive): no status or navigation bar
 * until a swipe from the edge shows them for a moment.
 *
 * Picture in picture ([PipHost]) only pauses the activity, so the main
 * player plays on in the window; closing the window stops the activity,
 * which stops it as above.
 */
class TvActivity : Activity() {
    private lateinit var nsh: NshView
    /** The mounted players (multiview mounts more than one). */
    private val players = LinkedHashSet<PlayerComponent>()
    /** Between onStart and onStop. */
    private var started = false
    private lateinit var pip: PipHost
    private lateinit var backups: BackupHost

    override fun onCreate(state: Bundle?) {
        super.onCreate(state)
        mark("activity created")
        DevActivity.edgeToEdge(this)
        nsh = NshView(this)
        mark("library loaded")
        initializeDataDirectory(filesDir.absolutePath)
        CredentialKey.initialize(this)
        RecordingHost.attach(this)
        nsh.useStandaloneLimits()
        pip = PipHost(this)
        backups = BackupHost(this)
        nsh.registerComponent(BackupComponent.NAME) { context ->
            BackupComponent(this, context, backups)
        }
        val gone = { p: PlayerComponent ->
            players.remove(p)
            pip.destroyed(p)
        }
        nsh.registerComponent(PlayerComponent.NAME) { context ->
            PlayerComponent(this, context, gone, pip).also {
                players.add(it)
                if (!started) it.background()
            }
        }
        setContentView(nsh)
        immersive()
        nsh.handleNotificationIntent(intent)
        if (state == null) nsh.handleUrlSchemeIntent(intent)
        nsh.runRustApp("nsh")
        mark("rust app started")
    }

    /** Called after loading the Rust library, before mounting any app state. */
    private external fun initializeDataDirectory(path: String)

    /** Waits, at most a second, for the small files the Rust app queued. */
    private external fun flushPendingWrites()

    /** A cold-start phase mark, as the Rust side logs them (`startup.rs`). */
    private fun mark(phase: String) {
        val ms = SystemClock.uptimeMillis() - Process.getStartUptimeMillis()
        Log.i("nsh", "iptv startup: $phase +$ms ms")
    }

    override fun onStart() {
        super.onStart()
        started = true
        val allowed = RecordingHost.resumeViewing(filesDir.absolutePath)
        for (p in players.toList()) p.foreground(allowed)
    }

    override fun onStop() {
        started = false
        for (p in players.toList()) p.background()
        RecordingHost.suspendViewing(filesDir.absolutePath)
        // The process may end in the background without a teardown: the
        // last channel or a setting changed a moment ago must be on disk.
        flushPendingWrites()
        super.onStop()
    }

    override fun onUserLeaveHint() {
        super.onUserLeaveHint()
        pip.userLeaving()
    }

    override fun onPictureInPictureModeChanged(inPip: Boolean, config: Configuration) {
        super.onPictureInPictureModeChanged(inPip, config)
        pip.modeChanged(inPip)
    }

    override fun onConfigurationChanged(config: Configuration) {
        super.onConfigurationChanged(config)
        DevActivity.systemBars(this)
        immersive()
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        // Dialogs and the system's own windows can bring the bars back.
        if (hasFocus) immersive()
    }

    /** Hides the status and navigation bars; a swipe shows them briefly. */
    @Suppress("DEPRECATION")
    private fun immersive() {
        if (Build.VERSION.SDK_INT >= 30) {
            val c = window.insetsController ?: return
            c.systemBarsBehavior = WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
            c.hide(WindowInsets.Type.systemBars())
        } else {
            window.decorView.systemUiVisibility = (window.decorView.systemUiVisibility
                or View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY
                or View.SYSTEM_UI_FLAG_FULLSCREEN
                or View.SYSTEM_UI_FLAG_HIDE_NAVIGATION
                or View.SYSTEM_UI_FLAG_LAYOUT_STABLE
                or View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN
                or View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION)
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        nsh.handleNotificationIntent(intent)
        nsh.handleUrlSchemeIntent(intent)
    }

    @Deprecated("App-owned Storage Access Framework bridge")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        if (!backups.activityResult(requestCode, resultCode, data)) {
            super.onActivityResult(requestCode, resultCode, data)
        }
    }

    /**
     * A remote's Search key: NSH has no key for `KEYCODE_SEARCH` (and the
     * system would start its own search), so it goes on as F12, which no
     * remote sends; the app binds F12 to its search on Android (`input.rs`).
     */
    override fun dispatchKeyEvent(event: KeyEvent): Boolean {
        val mapped = RemoteKeys.translate(event.keyCode)
        if (mapped == event.keyCode) return super.dispatchKeyEvent(event)
        val search = KeyEvent(
            event.downTime, event.eventTime, event.action, mapped, event.repeatCount,
            event.metaState, event.deviceId, event.scanCode, event.flags, event.source,
        )
        return super.dispatchKeyEvent(search)
    }

    @Deprecated("Back is also handled by NshView's OnBackInvokedCallback on API 33+")
    override fun onBackPressed() {
        if (!nsh.handleBack()) super.onBackPressed()
    }

    override fun onDestroy() {
        backups.destroy()
        pip.destroy()
        nsh.destroy()
        super.onDestroy()
    }

}
