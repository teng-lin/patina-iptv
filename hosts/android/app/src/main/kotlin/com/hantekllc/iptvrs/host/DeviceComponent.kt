package com.hantekllc.iptvrs.host

import android.accessibilityservice.AccessibilityServiceInfo
import android.annotation.SuppressLint
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.content.res.Configuration
import android.database.ContentObserver
import android.media.MediaCodecInfo
import android.media.MediaCodecList
import android.media.MediaFormat
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.PowerManager
import android.provider.Settings
import android.util.DisplayMetrics
import android.util.Log
import android.view.View
import android.view.WindowManager
import android.view.accessibility.AccessibilityManager
import com.longbridge.nsh.NshComponent
import com.longbridge.nsh.NshComponentContext

/**
 * Reports the device's form factor, connected input and video decoder
 * capacity to the Rust app (`device.rs`) as a `device` event, `{"form",
 * "keyboard", "dpad", "decoders"?}`: when it attaches, and again on each
 * configuration change that alters them
 * (the activity handles `keyboard`, `keyboardHidden`, `navigation`,
 * `screenSize` and `smallestScreenSize` itself, so its views receive them).
 *
 * Also the accessibility settings that panels hiding by themselves follow,
 * as an `accessibility` event, `{"controls_ms", "text_ms",
 * "switch_access"}`: the timeouts `AccessibilityManager` recommends (the
 * "time to take action" setting) and whether Switch Access runs. They are
 * reported when it attaches, whenever accessibility is switched on or off
 * or its services or timeouts change, and when the window regains the
 * focus (back from the system's settings).
 *
 * It reports what can stop a capture ([CaptureLimits]) as a `limits` event
 * on attach and when the default network or Low Power Standby changes.
 */
internal class DeviceComponent(
    private val context: NshComponentContext,
) : NshComponent {
    companion object {
        const val NAME = "tv.device"
    }

    private var reported: Map<String, Any>? = null
    private var reportedAccessibility: Map<String, Any>? = null
    private var limits: Map<String, Any>? = null
    private val main = Handler(Looper.getMainLooper())
    private val accessibility = context.context.getSystemService(AccessibilityManager::class.java)
    private val accessibilityChanged = AccessibilityManager.AccessibilityStateChangeListener { reportAccessibilitySoon() }
    private val accessibilitySettings =
        object : ContentObserver(main) {
            override fun onChange(selfChange: Boolean) = reportAccessibilitySoon()
        }
    private val network =
        object : ConnectivityManager.NetworkCallback() {
            override fun onCapabilitiesChanged(
                network: Network,
                capabilities: NetworkCapabilities,
            ) {
                main.post { reportLimits() }
            }

            override fun onLost(network: Network) {
                main.post { reportLimits() }
            }
        }
    private val standby =
        object : BroadcastReceiver() {
            override fun onReceive(
                context: Context,
                intent: Intent,
            ) = reportLimits()
        }

    /** Read once: the codecs do not change while the app runs. */
    private val decoders: Int? by lazy { DeviceSignals.probeDecoders() }
    private val observer =
        object : View(context.context) {
            override fun onAttachedToWindow() {
                super.onAttachedToWindow()
                report(resources.configuration)
                accessibility?.addAccessibilityStateChangeListener(accessibilityChanged)
                for (name in AccessibilitySignals.SETTINGS) {
                    context.contentResolver.registerContentObserver(
                        Settings.Secure.getUriFor(name),
                        false,
                        accessibilitySettings,
                    )
                }
                reportAccessibility()
                watchLimits(true)
                reportLimits()
            }

            override fun onDetachedFromWindow() {
                accessibility?.removeAccessibilityStateChangeListener(accessibilityChanged)
                context.contentResolver.unregisterContentObserver(accessibilitySettings)
                watchLimits(false)
                // Drop accessibility and network reports queued before detaching.
                main.removeCallbacksAndMessages(null)
                super.onDetachedFromWindow()
            }

            override fun onWindowFocusChanged(hasWindowFocus: Boolean) {
                super.onWindowFocusChanged(hasWindowFocus)
                if (hasWindowFocus) reportAccessibility()
            }

            override fun onConfigurationChanged(config: Configuration) {
                super.onConfigurationChanged(config)
                report(config)
                reportLimits()
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
            buildMap {
                put("form", DeviceSignals.form(tv, display))
                put("keyboard", DeviceSignals.keyboard(config.keyboard, config.hardKeyboardHidden))
                put("dpad", DeviceSignals.dpad(config.navigation, config.navigationHidden))
                decoders?.let { put("decoders", it) }
            }
        Log.i("nsh", "iptv device: $next (display ${display}dp, window ${config.smallestScreenWidthDp}dp)")
        if (next != reported) {
            reported = next
            context.emit("device", next)
        }
    }

    /**
     * A settings change reaches this observer before `AccessibilityManager`
     * has its new values (seen on the emulator: a timeout or a service
     * switched on read as before), so read now and again once they settled.
     */
    private fun reportAccessibilitySoon() {
        if (!observer.isAttachedToWindow) return
        reportAccessibility()
        main.removeCallbacks(settledReport)
        main.postDelayed(settledReport, AccessibilitySignals.SETTLE_MS)
    }

    private val settledReport = Runnable { reportAccessibility() }

    private fun reportAccessibility() {
        val manager = accessibility ?: return
        val recommended = { flags: Int ->
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) manager.getRecommendedTimeoutMillis(0, flags) else 0
        }
        val services =
            manager
                .getEnabledAccessibilityServiceList(AccessibilityServiceInfo.FEEDBACK_ALL_MASK)
                .mapNotNull { it.id }
        val next =
            mapOf(
                "controls_ms" to recommended(AccessibilitySignals.CONTROLS),
                "text_ms" to recommended(AccessibilitySignals.TEXT),
                "switch_access" to AccessibilitySignals.switchAccess(services),
            )
        if (next != reportedAccessibility) {
            Log.i("nsh", "iptv accessibility: $next")
            reportedAccessibility = next
            context.emit("accessibility", next)
        }
    }

    private fun reportLimits() {
        if (!observer.isAttachedToWindow) return
        val next = CaptureLimits.report(observer.context)
        if (next != limits) {
            limits = next
            Log.i("nsh", "iptv capture limits: $next")
            context.emit("limits", next)
        }
    }

    private fun watchLimits(on: Boolean) {
        val host = observer.context
        try {
            val connectivity = host.getSystemService(ConnectivityManager::class.java)
            if (on) {
                connectivity?.registerDefaultNetworkCallback(network)
                if (Build.VERSION.SDK_INT >= 33) {
                    host.registerReceiver(
                        standby,
                        IntentFilter(PowerManager.ACTION_LOW_POWER_STANDBY_ENABLED_CHANGED),
                        Context.RECEIVER_NOT_EXPORTED,
                    )
                }
            } else {
                connectivity?.unregisterNetworkCallback(network)
                if (Build.VERSION.SDK_INT >= 33) host.unregisterReceiver(standby)
            }
        } catch (_: RuntimeException) {
            // Not registered (or already removed): the report on each
            // attach and configuration change still applies.
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

    /** What the decoder probe reads of one codec. */
    data class Decoder(
        val encoder: Boolean,
        val hardware: Boolean,
        val avc: Boolean,
        val maxInstances: Int,
        /** Only for protected content (a `.secure` codec): not picked for a clear stream. */
        val secureOnly: Boolean = false,
        /** Another name for a codec listed too (Android 10+): Media3 skips it. */
        val alias: Boolean = false,
    )

    /**
     * The H.264 streams the device decodes in hardware at once: the
     * instance limit of the hardware H.264 decoder the player picks for a
     * clear stream (Media3 takes the first in the codec list's order), or
     * null without one (Media3 then decodes in software, which no instance
     * limit bounds) or when it reports no limit. The app only lowers its
     * two-tile budget by it (`Host::decoder_slots`).
     */
    fun decoders(codecs: List<Decoder>): Int? =
        codecs
            .firstOrNull { !it.encoder && !it.alias && !it.secureOnly && it.hardware && it.avc }
            ?.maxInstances
            ?.takeIf { it > 0 }

    /**
     * [decoders] on this device's codec list; null if any of it cannot be
     * read. Skipping an unreadable codec could lose the one decoder that
     * allows two instances, so the budget is then left as it is.
     */
    fun probeDecoders(): Int? =
        runCatching {
            val codecs =
                MediaCodecList(MediaCodecList.REGULAR_CODECS).codecInfos.map { info ->
                    // The type as the codec spells it: its capabilities are
                    // looked up by that exact string.
                    val avc = info.supportedTypes.firstOrNull { it.equals(MediaFormat.MIMETYPE_VIDEO_AVC, ignoreCase = true) }
                    val capabilities = if (avc != null && !info.isEncoder) info.getCapabilitiesForType(avc) else null
                    Decoder(
                        encoder = info.isEncoder,
                        hardware = hardware(info),
                        avc = avc != null,
                        maxInstances = capabilities?.maxSupportedInstances ?: 0,
                        secureOnly =
                            capabilities?.isFeatureRequired(MediaCodecInfo.CodecCapabilities.FEATURE_SecurePlayback) == true,
                        alias = Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q && info.isAlias,
                    )
                }
            decoders(codecs)
        }.onFailure { Log.w("nsh", "iptv device: decoder probe failed", it) }
            .getOrNull()

    /** A hardware codec: as Android 10 reports it, before that by the software codecs' names. */
    private fun hardware(info: MediaCodecInfo): Boolean =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            info.isHardwareAccelerated
        } else {
            val name = info.name.lowercase()
            !name.startsWith("omx.google.") && !name.startsWith("c2.android.")
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

/**
 * What Android's accessibility settings ask of panels that hide by
 * themselves (`device.rs`'s `Recommended` and `assistive`). The content
 * flags are API 29's constants, inlined at compile time and passed only to
 * the API 29 call that reads them.
 */
@SuppressLint("InlinedApi")
internal object AccessibilitySignals {
    /** Panels with controls: the control strip, the card's actions, the play bar. */
    const val CONTROLS = AccessibilityManager.FLAG_CONTENT_CONTROLS or AccessibilityManager.FLAG_CONTENT_TEXT

    /** How long after a change its new values are read again (ms). */
    const val SETTLE_MS = 1_000L

    /** Panels to read: the banner after a channel change, the info card. */
    const val TEXT = AccessibilityManager.FLAG_CONTENT_TEXT or AccessibilityManager.FLAG_CONTENT_ICONS

    /**
     * The secure settings whose change can change a report: the enabled
     * services, and the interactive and non-interactive timeouts behind
     * `getRecommendedTimeoutMillis` (named as `Settings.Secure` keeps them).
     */
    val SETTINGS =
        listOf(
            Settings.Secure.ENABLED_ACCESSIBILITY_SERVICES,
            "accessibility_interactive_ui_timeout_ms",
            "accessibility_non_interactive_ui_timeout_ms",
        )

    /**
     * The class names of the switch services: Google's Switch Access (in
     * the Accessibility Suite, `com.google.android.marvin.talkback`, or
     * its own older package) and Samsung's Universal Switch.
     */
    private val SWITCH_SERVICES = setOf("SwitchAccessService", "UniversalSwitchService")

    /**
     * Whether a switch service runs, by the ids (`package/class`) of the
     * enabled accessibility services. A heuristic: no service property
     * sets a switch service apart, so it matches the services' class names
     * ([SWITCH_SERVICES]) exactly, whatever their package, and nothing
     * else that only contains those words.
     */
    fun switchAccess(serviceIds: List<String>): Boolean =
        serviceIds.any { id ->
            id.substringAfter('/').substringAfterLast('.') in SWITCH_SERVICES
        }
}
