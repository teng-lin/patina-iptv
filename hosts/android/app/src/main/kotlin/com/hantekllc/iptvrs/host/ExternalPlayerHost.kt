package com.hantekllc.iptvrs.host

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.net.Uri
import android.os.Build

/** Starts another installed video app with an opaque Patina loopback URL. */
internal object ExternalPlayerHost {
    @Volatile private var application: Context? = null

    /** What the handoff's intent looks like before it has an address; the
     * manifest's `<queries>` entry makes the apps that take it visible. */
    const val SCHEME = "http"
    const val MIME_TYPE = "video/*"

    @JvmStatic private external fun initialize()

    fun attach(context: Context) {
        application = context.applicationContext
        initialize()
    }

    /** Whether another app would take a handoff now. Rust asks before it
     * offers the group option and before each handoff, so a player removed
     * since the viewer chose it is noticed. */
    @JvmStatic fun available(): Boolean {
        val context = application ?: return false
        return try {
            players(context, video(Uri.parse("$SCHEME://127.0.0.1/"))).isNotEmpty()
        } catch (_: RuntimeException) {
            false
        }
    }

    @JvmStatic fun open(address: String): Boolean {
        val context = application ?: return false
        val uri = Uri.parse(address)
        if (uri.scheme != SCHEME || uri.host != "127.0.0.1") return false
        return try {
            val video = video(uri)
            val players = players(context, video)
            val default = context.packageManager
                .resolveActivity(video, PackageManager.MATCH_DEFAULT_ONLY)
                ?.activityInfo
                ?.let { ComponentName(it.packageName, it.name) }
            val launch = when (val target = target(players, default)) {
                // Android TV images resolve the chooser and the resolver to
                // framework stubs that only say no app can do this, so one
                // player, or the viewer's default, is started directly.
                null -> {
                    if (players.isEmpty()) return false
                    Intent.createChooser(video, null)
                        .putExtra(Intent.EXTRA_EXCLUDE_COMPONENTS, stubs(context, video).toTypedArray())
                }
                else -> video.setComponent(target)
            }
            // Started while Patina is still in the foreground, as Android
            // requires; a failed start leaves nothing to release.
            ExternalPlaybackService.hold(context)
            try {
                context.startActivity(launch.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
            } catch (error: RuntimeException) {
                ExternalPlaybackService.release()
                throw error
            }
            true
        } catch (_: RuntimeException) {
            // No activity found, refused (SecurityException) or a stale intent.
            false
        }
    }

    /** Called by Rust once for each handoff [open] started, when its relay
     * has shut down. */
    @JvmStatic fun released() {
        ExternalPlaybackService.release()
    }

    private fun video(uri: Uri) = Intent(Intent.ACTION_VIEW).setDataAndType(uri, MIME_TYPE)

    private fun handlers(context: Context, video: Intent): List<ComponentName> =
        context.packageManager
            .queryIntentActivities(video, PackageManager.MATCH_DEFAULT_ONLY)
            .mapNotNull { it.activityInfo?.let { info -> ComponentName(info.packageName, info.name) } }

    private fun players(context: Context, video: Intent): List<ComponentName> =
        handlers(context, video).filter { player(it.packageName, context.packageName) }

    private fun stubs(context: Context, video: Intent): List<ComponentName> =
        handlers(context, video).filter { stub(it.packageName) }

    /** The framework package stubs (AOSP's `FrameworkPackageStubs`, on
     * Android TV and some other images) answer every view intent with a
     * message that no app can do it; they are not a player. Only these exact
     * system packages are left out, never another app with a similar name. */
    private val stubPackages = setOf("com.android.tv.frameworkpackagestubs", "com.android.frameworkpackagestubs")

    fun stub(handler: String): Boolean = handler in stubPackages

    /** Only another app that plays video counts: a handoff never comes back
     * to Patina, and a framework stub plays nothing. */
    fun player(handler: String, own: String): Boolean = handler != own && !stub(handler)

    /** The player to start directly: the viewer's default when it is one of
     * [players], else the only one; null leaves the choice to the chooser. */
    fun <T> target(players: List<T>, default: T?): T? =
        when {
            default != null && default in players -> default
            players.size == 1 -> players[0]
            else -> null
        }
}

/**
 * Keeps Patina's process running while another app plays through its
 * loopback relay. Without it Android caches the process once its activity
 * stops, and Android 14 and later freeze cached processes: the relay stops
 * answering and the other player stalls within seconds. It follows the
 * recording service's pattern, with the media playback type, which has no
 * daily time limit.
 */
class ExternalPlaybackService : Service() {
    private lateinit var notification: Notification
    private var lastStart = 0

    override fun onBind(intent: Intent?) = null

    override fun onCreate() {
        super.onCreate()
        val notifications = getSystemService(NotificationManager::class.java)
        if (Build.VERSION.SDK_INT >= 26) {
            notifications.createNotificationChannel(
                NotificationChannel(CHANNEL, getString(R.string.external_channel), NotificationManager.IMPORTANCE_LOW),
            )
        }
        // Returning to Patina ends the handoff and this service with it.
        val open =
            PendingIntent.getActivity(
                this,
                0,
                Intent(this, TvActivity::class.java),
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
        val builder = if (Build.VERSION.SDK_INT >= 26) Notification.Builder(this, CHANNEL) else Notification.Builder(this)
        notification =
            builder
                .setContentTitle(getString(R.string.external_title))
                .setContentText(getString(R.string.external_background))
                .setSmallIcon(android.R.drawable.ic_media_play)
                .setContentIntent(open)
                .setOngoing(true)
                .build()
    }

    // Each start asks for the foreground again, as Android requires.
    override fun onStartCommand(
        intent: Intent?,
        flags: Int,
        startId: Int,
    ): Int {
        if (Build.VERSION.SDK_INT >= 29) {
            startForeground(NOTIFICATION, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK)
        } else {
            startForeground(NOTIFICATION, notification)
        }
        // Every handoff may have ended before this start arrived.
        if (!started(this, startId)) stopSelf(startId)
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        destroyed(this)
        super.onDestroy()
    }

    companion object {
        private const val CHANNEL = "external-player"
        private const val NOTIFICATION = 473

        // Guarded by the companion: JNI calls come from Rust's threads.
        private var holds = 0
        private var running: ExternalPlaybackService? = null

        /** Counts a handoff and starts the service for it, before the other
         * app's activity takes the foreground from Patina. Every handoff
         * starts it, so one whose start failed does not leave later ones
         * without the service. */
        @Synchronized fun hold(context: Context) {
            holds += 1
            try {
                val application = context.applicationContext
                val intent = Intent(application, ExternalPlaybackService::class.java)
                if (Build.VERSION.SDK_INT >= 26) {
                    application.startForegroundService(intent)
                } else {
                    application.startService(intent)
                }
            } catch (_: RuntimeException) {
                // The handoff still runs; Android may freeze the relay.
            }
        }

        /** Ends a handoff's hold and stops the service after the last one. */
        @Synchronized fun release() {
            holds = maxOf(0, holds - 1)
            // A start sent after the one stopped here keeps the service.
            if (holds == 0) running?.let { it.stopSelf(it.lastStart) }
        }

        @Synchronized private fun started(
            service: ExternalPlaybackService,
            startId: Int,
        ): Boolean {
            running = service
            service.lastStart = startId
            return holds > 0
        }

        @Synchronized private fun destroyed(service: ExternalPlaybackService) {
            if (running === service) running = null
        }
    }
}
