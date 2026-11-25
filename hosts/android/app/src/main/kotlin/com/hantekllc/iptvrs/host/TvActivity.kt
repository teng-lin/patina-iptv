package com.hantekllc.iptvrs.host

import android.app.Activity
import android.content.Intent
import android.content.pm.ActivityInfo
import android.content.pm.PackageManager
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
import android.widget.EditText
import androidx.core.view.WindowCompat
import com.longbridge.nsh.NshView
import dev.nshtv.RemoteKeys

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
 *
 * One audio focus for all players ([AudioFocusHost]): a call, another
 * app's media or unplugged headphones reach the app as the players'
 * `audio` event, and the app's policy pauses or resumes.
 *
 * A phone runs in landscape for now (#168), on either side as the user's
 * rotation lock allows: the guide and the player are landscape layouts,
 * and portrait waits for its own design. Tablets, an unfolded phone, TVs
 * and windows of 600 dp and wider rotate freely ([landscapeOnPhones]).
 */
class TvActivity : Activity() {
    private lateinit var nsh: NshView

    /** The mounted players (multiview mounts more than one). */
    private val players = LinkedHashSet<PlayerComponent>()

    /** Between onStart and onStop. */
    private var started = false
    private lateinit var pip: PipHost
    private lateinit var audio: AudioFocusHost
    private lateinit var backups: BackupHost

    override fun onCreate(state: Bundle?) {
        super.onCreate(state)
        mark("activity created")
        edgeToEdge()
        landscapeOnPhones(resources.configuration)
        nsh = NshView(this)
        mark("library loaded")
        initializeDataDirectory(filesDir.absolutePath)
        CredentialKey.initialize(this)
        RecordingHost.attach(this)
        ExternalPlayerHost.attach(this)
        // Use large ceilings for Patina's own Rust root. The 4095 MiB
        // heap ceiling fits 32-bit Android, and zero keeps the default stack.
        nsh.setLimits(4095, 0, Int.MAX_VALUE, Int.MAX_VALUE, Int.MAX_VALUE)
        pip = PipHost(this)
        audio = AudioFocusHost(this)
        backups = BackupHost(this)
        nsh.registerComponent(BackupComponent.NAME) { context ->
            BackupComponent(this, context, backups)
        }
        nsh.registerComponent(ImeVisibilityComponent.NAME, ::ImeVisibilityComponent)
        nsh.registerComponent(DeviceComponent.NAME) { context ->
            DeviceComponent(context, ::updatePhoneOrientation)
        }
        val gone = { p: PlayerComponent ->
            players.remove(p)
            pip.destroyed(p)
        }
        nsh.registerComponent(PlayerComponent.NAME) { context ->
            PlayerComponent(this, context, gone, pip, audio).also {
                players.add(it)
                audio.mounted(it)
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

    override fun onResume() {
        super.onResume()
        // The backup rows follow the document pickers that resolve now.
        backups.resumed()
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

    override fun onPictureInPictureModeChanged(
        inPip: Boolean,
        config: Configuration,
    ) {
        super.onPictureInPictureModeChanged(inPip, config)
        pip.modeChanged(inPip)
        landscapeOnPhones(config)
    }

    override fun onMultiWindowModeChanged(
        inMultiWindow: Boolean,
        config: Configuration,
    ) {
        super.onMultiWindowModeChanged(inMultiWindow, config)
        landscapeOnPhones(config)
    }

    override fun onConfigurationChanged(config: Configuration) {
        super.onConfigurationChanged(config)
        landscapeOnPhones(config)
        updateSystemBarAppearance(config)
        immersive()
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        // Dialogs and the system's own windows can bring the bars back.
        if (hasFocus) immersive()
    }

    /**
     * A phone's screen (smallest width under 600 dp, not a TV) asks for
     * landscape, either side as the rotation lock allows; anything wider,
     * such as a phone unfolded to a tablet's screen, gives the orientation
     * back. In multi-window and picture in picture the window is not the
     * screen (a tablet's split window can be narrower than 600 dp): it asks
     * for nothing there, and asks again when the window fills the screen
     * (a configuration change).
     */
    private fun landscapeOnPhones(config: Configuration) {
        val tv = DeviceSignals.tv(config.uiMode, packageManager.hasSystemFeature(PackageManager.FEATURE_LEANBACK))
        updatePhoneOrientation(!tv && DeviceSignals.phone(DeviceSignals.displaySmallestDp(this, config)))
    }

    private fun updatePhoneOrientation(phone: Boolean) {
        val windowed = isInMultiWindowMode || isInPictureInPictureMode
        val wanted =
            if (phone && !windowed) {
                ActivityInfo.SCREEN_ORIENTATION_USER_LANDSCAPE
            } else {
                ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED
            }
        if (requestedOrientation != wanted) requestedOrientation = wanted
    }

    /** Draw under the system bars so their transient appearance leaves the layout in place. */
    @Suppress("DEPRECATION")
    private fun edgeToEdge() {
        WindowCompat.setDecorFitsSystemWindows(window, false)
        window.statusBarColor = 0
        window.navigationBarColor = 0
        updateSystemBarAppearance(resources.configuration)
    }

    private fun updateSystemBarAppearance(config: Configuration) {
        val night = config.uiMode and Configuration.UI_MODE_NIGHT_MASK
        val light = night != Configuration.UI_MODE_NIGHT_YES
        val controller = WindowCompat.getInsetsController(window, window.decorView)
        controller.isAppearanceLightStatusBars = light
        controller.isAppearanceLightNavigationBars = light
    }

    /** Hides the status and navigation bars; a swipe shows them briefly. */
    @Suppress("DEPRECATION")
    private fun immersive() {
        if (Build.VERSION.SDK_INT >= 30) {
            val c = window.insetsController ?: return
            c.systemBarsBehavior = WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
            c.hide(WindowInsets.Type.systemBars())
        } else {
            window.decorView.systemUiVisibility = (
                window.decorView.systemUiVisibility
                    or View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY
                    or View.SYSTEM_UI_FLAG_FULLSCREEN
                    or View.SYSTEM_UI_FLAG_HIDE_NAVIGATION
                    or View.SYSTEM_UI_FLAG_LAYOUT_STABLE
                    or View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN
                    or View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION
            )
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        nsh.handleNotificationIntent(intent)
        nsh.handleUrlSchemeIntent(intent)
    }

    @Deprecated("App-owned Storage Access Framework bridge")
    override fun onActivityResult(
        requestCode: Int,
        resultCode: Int,
        data: Intent?,
    ) {
        if (!backups.activityResult(requestCode, resultCode, data)) {
            super.onActivityResult(requestCode, resultCode, data)
        }
    }

    override fun dispatchKeyEvent(event: KeyEvent): Boolean {
        val editing = currentFocus is EditText
        val translated = if (editing) event else RemoteKeys.translate(event)
        return super.dispatchKeyEvent(translated) || (!editing && RemoteKeys.appOwned(event.keyCode))
    }

    @Deprecated("Back is also handled by NshView's OnBackInvokedCallback on API 33+")
    override fun onBackPressed() {
        if (!nsh.handleBack()) super.onBackPressed()
    }

    override fun onDestroy() {
        backups.destroy()
        pip.destroy()
        audio.destroy()
        nsh.destroy()
        super.onDestroy()
    }
}
