package com.hantekllc.iptvrs.host

import android.content.Context
import android.content.pm.PackageManager
import android.content.res.Configuration
import android.os.Build
import android.util.DisplayMetrics
import android.util.Log
import android.view.View
import android.view.WindowManager
import com.longbridge.nsh.NshComponent
import com.longbridge.nsh.NshComponentContext

/**
 * Reports the device's form factor and connected input to the Rust app
 * (`device.rs`) as a `device` event, `{"form", "keyboard", "dpad"}`: when
 * it attaches, and again on each configuration change that alters them
 * (the activity handles `keyboard`, `keyboardHidden`, `navigation`,
 * `screenSize` and `smallestScreenSize` itself, so its views receive them).
 */
internal class DeviceComponent(
    private val context: NshComponentContext,
) : NshComponent {
    companion object {
        const val NAME = "tv.device"
    }

    private var reported: Map<String, Any>? = null
    private val observer =
        object : View(context.context) {
            override fun onAttachedToWindow() {
                super.onAttachedToWindow()
                report(resources.configuration)
            }

            override fun onConfigurationChanged(config: Configuration) {
                super.onConfigurationChanged(config)
                report(config)
            }
        }.apply {
            importantForAccessibility = View.IMPORTANT_FOR_ACCESSIBILITY_NO
            isFocusable = false
        }

    private fun report(config: Configuration) {
        val host = observer.context
        val leanback = host.packageManager.hasSystemFeature(PackageManager.FEATURE_LEANBACK)
        val tv = DeviceSignals.tv(config.uiMode, leanback)
        val display = DeviceSignals.displaySmallestDp(host, config)
        val next =
            mapOf(
                "form" to DeviceSignals.form(tv, display),
                "keyboard" to DeviceSignals.keyboard(config.keyboard, config.hardKeyboardHidden),
                "dpad" to DeviceSignals.dpad(config.navigation, config.navigationHidden),
            )
        Log.i("nsh", "iptv device: $next (display ${display}dp, window ${config.smallestScreenWidthDp}dp)")
        if (next != reported) {
            reported = next
            context.emit("device", next)
        }
    }

    override fun view(): View = observer

    override fun setProp(
        name: String,
        value: Any?,
    ) = Unit
}

/**
 * The device signals from Android's reports, as `iptv_core::capabilities`
 * names them. The form factor follows the physical display, never the
 * app's window: a tablet's split-screen window is narrower than a phone's
 * screen, and stays a tablet.
 */
internal object DeviceSignals {
    /** A display narrower than this, at its smallest, is a phone's (`FormFactor::PHONE_MAX_DP`). */
    const val PHONE_MAX_DP = 600

    /** A TV: by its UI mode, or by the leanback feature. */
    fun tv(
        uiMode: Int,
        leanback: Boolean,
    ): Boolean = leanback || (uiMode and Configuration.UI_MODE_TYPE_MASK) == Configuration.UI_MODE_TYPE_TELEVISION

    /** `FormFactor::android`: a TV, otherwise a phone or a tablet by the display's smallest width (dp; 0 if unknown). */
    fun form(
        tv: Boolean,
        displaySmallestDp: Int,
    ): String =
        when {
            tv -> "tv"
            displaySmallestDp in 1 until PHONE_MAX_DP -> "phone"
            else -> "tablet"
        }

    /** A hardware keyboard with keys for text is attached and not hidden (a folded-away keyboard is). */
    fun keyboard(
        keyboard: Int,
        hidden: Int,
    ): Boolean = keyboard == Configuration.KEYBOARD_QWERTY && hidden != Configuration.HARDKEYBOARDHIDDEN_YES

    /** A D-pad (a remote's, a game pad's) is attached and not hidden. */
    fun dpad(
        navigation: Int,
        hidden: Int,
    ): Boolean = navigation == Configuration.NAVIGATION_DPAD && hidden != Configuration.NAVIGATIONHIDDEN_YES

    /**
     * The smallest width of the physical display in dp: the largest window
     * the system would give the app, whatever its window is now (split
     * screen, a freeform window), at `config`'s density. 0 when it cannot
     * be read.
     */
    fun displaySmallestDp(
        context: Context,
        config: Configuration,
    ): Int {
        val windows = context.getSystemService(WindowManager::class.java) ?: return 0
        // The incoming configuration's density: the resources' can still
        // hold the previous one during `onConfigurationChanged`.
        val density = config.densityDpi / DisplayMetrics.DENSITY_DEFAULT.toFloat()
        if (density <= 0f) return 0
        val (width, height) =
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                val bounds = windows.maximumWindowMetrics.bounds
                bounds.width() to bounds.height()
            } else {
                val metrics = DisplayMetrics()
                @Suppress("DEPRECATION")
                windows.defaultDisplay.getRealMetrics(metrics)
                metrics.widthPixels to metrics.heightPixels
            }
        return (minOf(width, height) / density).toInt()
    }
}
