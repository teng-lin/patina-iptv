package com.hantekllc.iptvrs.host

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.pm.PackageManager
import android.net.ConnectivityManager
import android.os.Build
import android.os.PowerManager

/**
 * What can stop a Record now capture on this device, or make it costly
 * (#275), as the Rust app reads it (`capture_limits.rs`): reported by
 * [DeviceComponent] as a `limits` event, `{"metered", "standbyStops",
 * "backgroundHours"}`, and acted on by [RecordingService].
 */
internal object CaptureLimits {
    /** Android 15's data-sync budget: six background hours a day. */
    const val BACKGROUND_LIMIT_SDK = 35

    /**
     * Whether Low Power Standby stops Patina when the TV is turned off: it
     * is on (Android 13 and later) and Patina is not exempt from it. Before
     * Android 14 an exemption cannot be read, so on counts as stopping.
     */
    fun standbyStops(
        sdk: Int,
        enabled: Boolean,
        exempt: Boolean,
    ): Boolean = sdk >= 33 && enabled && (sdk < 34 || !exempt)

    fun standbyStops(context: Context): Boolean {
        if (Build.VERSION.SDK_INT < 33) return false
        val power = context.getSystemService(PowerManager::class.java) ?: return false
        return try {
            standbyStops(
                Build.VERSION.SDK_INT,
                power.isLowPowerStandbyEnabled,
                Build.VERSION.SDK_INT >= 34 && power.isExemptFromLowPowerStandby,
            )
        } catch (_: RuntimeException) {
            false
        }
    }

    /** Whether Android limits a background capture to six hours a day. */
    fun backgroundHours(sdk: Int): Boolean = sdk >= BACKGROUND_LIMIT_SDK

    /** Whether the active network is metered; null when it cannot be read. */
    fun metered(context: Context): Boolean? =
        try {
            context.getSystemService(ConnectivityManager::class.java)?.isActiveNetworkMetered
        } catch (_: RuntimeException) {
            null
        }

    /** The report for `capture_limits.rs`; a metered network it cannot read is left out. */
    fun report(context: Context): Map<String, Any> {
        val report =
            mutableMapOf<String, Any>(
                "standbyStops" to standbyStops(context),
                "backgroundHours" to backgroundHours(Build.VERSION.SDK_INT),
            )
        metered(context)?.let { report["metered"] = it }
        return report
    }

    /** A TV, as [DeviceSignals.tv] tells it. */
    fun tv(context: Context): Boolean =
        DeviceSignals.tv(
            context.resources.configuration.uiMode,
            context.packageManager.hasSystemFeature(PackageManager.FEATURE_LEANBACK),
        )

    private const val CHANNEL = "recording-stopped"
    private const val NOTIFICATION = 473

    /**
     * Tells the viewer that a background capture stopped early, and why
     * (`message`, a string resource). The recording keeps what was
     * written; opening the notification opens Patina.
     */
    fun notifyStopped(
        context: Context,
        message: Int,
    ) {
        try {
            val notifications = context.getSystemService(NotificationManager::class.java) ?: return
            if (Build.VERSION.SDK_INT >= 26) {
                notifications.createNotificationChannel(
                    NotificationChannel(
                        CHANNEL,
                        context.getString(R.string.recording_stopped_channel),
                        NotificationManager.IMPORTANCE_DEFAULT,
                    ),
                )
            }
            val open =
                PendingIntent.getActivity(
                    context,
                    1,
                    appLaunchIntent(context),
                    PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
                )
            val builder =
                if (Build.VERSION.SDK_INT >= 26) Notification.Builder(context, CHANNEL) else Notification.Builder(context)
            notifications.notify(
                NOTIFICATION,
                builder
                    .setContentTitle(context.getString(R.string.recording_stopped_title))
                    .setContentText(context.getString(message))
                    .setStyle(Notification.BigTextStyle().bigText(context.getString(message)))
                    .setSmallIcon(android.R.drawable.ic_menu_save)
                    .setContentIntent(open)
                    .setAutoCancel(true)
                    .build(),
            )
        } catch (_: RuntimeException) {
            // Notifications denied: the library still shows why it stopped.
        }
    }
}
