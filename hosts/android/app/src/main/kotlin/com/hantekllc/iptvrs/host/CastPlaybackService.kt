package com.hantekllc.iptvrs.host

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.graphics.drawable.Icon
import android.content.pm.ServiceInfo
import android.media.MediaMetadata
import android.media.VolumeProvider
import android.media.session.MediaSession
import android.media.session.PlaybackState
import android.net.ConnectivityManager
import android.net.LinkProperties
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.PowerManager
import java.net.InetAddress

/** The service can outlive the activity while an owned LAN relay serves the receiver. */
class CastPlaybackService : Service() {
    private data class Lease(val lan: CastLanNetwork, val address: InetAddress)
    private val leases = LinkedHashMap<Long, Lease>()
    private val main = Handler(Looper.getMainLooper())
    private var lastStart = 0
    private var active: Long? = null
    private var lastPlayback: Pair<Int, Int>? = null
    private var wake: PowerManager.WakeLock? = null
    private var wifi: WifiManager.WifiLock? = null
    private var monitoring = false
    private lateinit var connectivity: ConnectivityManager
    private lateinit var media: MediaSession
    private lateinit var openApp: PendingIntent

    private val networkCallback = object : ConnectivityManager.NetworkCallback() {
        override fun onLost(network: Network) {
            main.post { lost(network, emptyList()) }
        }

        override fun onLinkPropertiesChanged(network: Network, properties: LinkProperties) {
            val addresses = properties.linkAddresses.map { it.address }
            main.post { lost(network, addresses) }
        }
    }

    override fun onBind(intent: Intent?) = null

    override fun onCreate() {
        super.onCreate()
        instance = this
        connectivity = getSystemService(ConnectivityManager::class.java)
        val localized = CaptureLimits.localized(this)
        getSystemService(NotificationManager::class.java).createNotificationChannel(
            NotificationChannel(CHANNEL, localized.getString(R.string.cast_playback), NotificationManager.IMPORTANCE_LOW),
        )
        openApp = PendingIntent.getActivity(
            this, 0, appLaunchIntent(this),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        media = MediaSession(this, "Patina Cast").apply {
            setSessionActivity(openApp)
            // The receiver already controls its audio output. Android must not
            // change the phone's volume in response to this remote session.
            setPlaybackToRemote(object : VolumeProvider(VolumeProvider.VOLUME_CONTROL_FIXED, 1, 1) {})
            setCallback(object : MediaSession.Callback() {
                override fun onPlay() = command(CastHost.PLAY)
                override fun onPause() = command(CastHost.PAUSE)
                override fun onStop() = command(CastHost.STOP)

                override fun onCustomAction(action: String, extras: Bundle?) {
                    if (action == Control.STOP.name) onStop()
                }
            }, main)
            setMetadata(
                MediaMetadata.Builder()
                    .putString(MediaMetadata.METADATA_KEY_TITLE, localized.getString(R.string.cast_playback))
                    .build(),
            )
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        lastStart = startId
        val id = intent?.getLongExtra(ID, 0L) ?: 0L
        val control = Control.entries.firstOrNull { it.name == intent?.action }
        if (control != null) {
            if (id == active) CastHost.playbackEvent(id, control.event)
            if (leases.isEmpty()) stopSelf(startId)
            return START_NOT_STICKY
        }
        try {
            // Required even if the request was canceled while Android queued this start.
            val notification = notification()
            if (Build.VERSION.SDK_INT >= 29) {
                startForeground(NOTIFICATION, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE)
            } else {
                startForeground(NOTIFICATION, notification)
            }
            val request = CastHost.request(id)
            if (request == null) {
                if (leases.isEmpty()) stopSelf(startId)
                return START_NOT_STICKY
            }
            if (!monitoring) {
                val networks = NetworkRequest.Builder()
                    .addTransportType(NetworkCapabilities.TRANSPORT_WIFI)
                    .addTransportType(NetworkCapabilities.TRANSPORT_ETHERNET)
                    .build()
                // Subscribe before validating the snapshot so a subsequent
                // address change reaches this specific lease.
                connectivity.registerNetworkCallback(networks, networkCallback)
                monitoring = true
            }
            val lan = CastLanNetwork.select(CastHost.lanNetworks(this), request)
            if (lan == null) {
                CastHost.playbackEvent(id, CastHost.STOP)
                if (leases.isEmpty()) stopSelf(startId)
                return START_NOT_STICKY
            }
            if (wake == null) {
                wake = getSystemService(PowerManager::class.java)
                    .newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "patina:cast").apply { acquire() }
            }
            if (lan.wifi) holdWifi()
            leases[id] = Lease(lan, request.address)
            CastHost.playbackEvent(id, CastHost.READY)
        } catch (_: RuntimeException) {
            CastHost.playbackEvent(id, CastHost.STOP)
            if (leases.isEmpty()) stopSelf(startId)
        }
        return START_NOT_STICKY
    }

    @Suppress("DEPRECATION")
    private fun holdWifi() {
        // Android 14 maps this lock to its screen-on low-latency mode.
        // Use the older lock only where it still helps a screen-off relay.
        if (wifi != null || Build.VERSION.SDK_INT >= 34) return
        wifi = applicationContext.getSystemService(WifiManager::class.java)
            ?.createWifiLock(WifiManager.WIFI_MODE_FULL_HIGH_PERF, "patina:cast")
            ?.apply { acquire() }
    }

    private fun lost(network: Network, addresses: List<InetAddress>) {
        for ((id, lease) in leases) {
            if (lease.lan.handle == network.networkHandle && lease.address !in addresses) {
                CastHost.playbackEvent(id, CastHost.STOP)
            }
        }
    }

    private fun command(event: Int) {
        active?.let { CastHost.playbackEvent(it, event) }
    }

    internal fun playback(id: Long, state: Int, controls: Int) {
        val snapshot = state to controls
        if (!leases.containsKey(id) || (active == id && lastPlayback == snapshot)) return
        active = id
        lastPlayback = snapshot
        val play = controls and CONTROL_PLAY != 0
        val pause = controls and CONTROL_PAUSE != 0
        var actions = PlaybackState.ACTION_STOP
        if (play) actions = actions or PlaybackState.ACTION_PLAY
        if (pause) actions = actions or PlaybackState.ACTION_PAUSE
        if (play && pause) actions = actions or PlaybackState.ACTION_PLAY_PAUSE
        val (platformState, speed) = when (state) {
            PLAYING -> PlaybackState.STATE_PLAYING to 1f
            PAUSED -> PlaybackState.STATE_PAUSED to 0f
            else -> PlaybackState.STATE_BUFFERING to 0f
        }
        media.setPlaybackState(
            PlaybackState.Builder().setActions(actions)
                .setState(platformState, PlaybackState.PLAYBACK_POSITION_UNKNOWN, speed)
                // Android 13+ places the button from the playback state.
                .addCustomAction(
                    Control.STOP.name,
                    CaptureLimits.localized(this).getString(Control.STOP.label),
                    Control.STOP.icon,
                )
                .build(),
        )
        media.isActive = true
        getSystemService(NotificationManager::class.java).notify(NOTIFICATION, notification())
    }

    internal fun release(id: Long) {
        if (leases.remove(id) == null) return
        if (id == active) {
            active = null
            lastPlayback = null
            media.isActive = false
        }
        if (leases.none { it.value.lan.wifi }) {
            wifi?.release()
            wifi = null
        }
        if (leases.isEmpty()) stopSelf(lastStart)
        else getSystemService(NotificationManager::class.java).notify(NOTIFICATION, notification())
    }

    private fun notification(): Notification {
        val localized = CaptureLimits.localized(this)
        val builder = Notification.Builder(this, CHANNEL)
            .setContentTitle(localized.getString(R.string.cast_playback))
            .setSmallIcon(android.R.drawable.ic_media_play)
            .setContentIntent(openApp)
            .setCategory(Notification.CATEGORY_TRANSPORT)
            .setVisibility(Notification.VISIBILITY_PUBLIC)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
        val id = active
        val playback = lastPlayback
        if (id != null && playback != null) {
            val (state, controls) = playback
            val primary = when {
                state == PLAYING && controls and CONTROL_PAUSE != 0 -> Control.PAUSE
                state == PAUSED && controls and CONTROL_PLAY != 0 -> Control.PLAY
                else -> null
            }
            val actions = listOfNotNull(primary, Control.STOP)
            val compact = actions.indices.toList().toIntArray()
            builder.setStyle(Notification.MediaStyle().setMediaSession(media.sessionToken).setShowActionsInCompactView(*compact))
            for (action in actions) {
                val intent = Intent(this, CastPlaybackService::class.java).setAction(action.name).putExtra(ID, id)
                val pending = PendingIntent.getService(
                    this, id.toInt(), intent,
                    PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
                )
                builder.addAction(Notification.Action.Builder(
                    Icon.createWithResource(this, action.icon),
                    localized.getString(action.label),
                    pending,
                ).build())
            }
        }
        return builder.build()
    }

    override fun onTaskRemoved(rootIntent: Intent?) {
        // Removing the task ends the application-owned session.
        for (id in leases.keys) CastHost.playbackEvent(id, CastHost.STOP)
        super.onTaskRemoved(rootIntent)
    }

    override fun onDestroy() {
        instance = null
        for (id in leases.keys) CastHost.playbackEvent(id, CastHost.STOP)
        if (monitoring) connectivity.unregisterNetworkCallback(networkCallback)
        media.release()
        wifi?.release()
        wake?.release()
        super.onDestroy()
    }

    private enum class Control(val event: Int, val icon: Int, val label: Int) {
        PLAY(CastHost.PLAY, android.R.drawable.ic_media_play, R.string.cast_play),
        PAUSE(CastHost.PAUSE, android.R.drawable.ic_media_pause, R.string.cast_pause),
        STOP(CastHost.STOP, android.R.drawable.ic_menu_close_clear_cancel, R.string.cast_stop),
    }

    companion object {
        internal const val ID = "castId"
        private const val CHANNEL = "cast-playback"
        private const val NOTIFICATION = 475
        private const val PLAYING = 1
        private const val PAUSED = 2
        private const val CONTROL_PLAY = 1
        private const val CONTROL_PAUSE = 2
        internal var instance: CastPlaybackService? = null
            private set
    }
}
