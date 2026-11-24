package com.hantekllc.iptvrs.host

import android.app.AlarmManager
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.PowerManager

/** Whether a capture runs, and the interface language, cross JNI;
 * credentials stay in Rust. */
object RecordingHost {
    @Volatile private var desiredActive = false
    private lateinit var context: Context

    @JvmStatic fun setLanguage(tag: String) {
        main.post {
            if (!::context.isInitialized) return@post
            val preferences = context.getSharedPreferences("recording-host", Context.MODE_PRIVATE)
            if (preferences.getString("language", "") != tag) {
                preferences.edit().putString("language", tag).apply()
                RecordingService.refresh()
            }
        }
    }

    private val main = Handler(Looper.getMainLooper())

    @JvmStatic private external fun initialize()

    @JvmStatic external fun start(path: String): Long

    @JvmStatic external fun stop(path: String)

    /** Stops running captures early, each kept as a partial recording with
     * the reason: [STOP_TV_OFF] or [STOP_BACKGROUND_LIMIT]. Returns how many
     * were running. */
    @JvmStatic external fun stopCaptures(
        path: String,
        why: Int,
    ): Int

    const val STOP_TV_OFF = 1
    const val STOP_BACKGROUND_LIMIT = 2

    @JvmStatic external fun suspendViewing(path: String)

    @JvmStatic external fun resumeViewing(path: String): Boolean

    fun attach(context: Context) {
        this.context = context.applicationContext
        retireSchedules(this.context)
        initialize()
    }

    /** Scheduled recording is retired. Once per install, before Rust can
     * publish again: cancel the wake an earlier version armed for a scheduled
     * job and forget its stored time. A Record now capture this version
     * starts arms its own watchdog through [schedule], which also ends the
     * retirement. Returns whether this call retired them; a failure is
     * retried on the next launch or broadcast until [schedule] has run. */
    @JvmStatic @Synchronized
    fun retireSchedules(context: Context): Boolean {
        val preferences = context.getSharedPreferences("recording-host", Context.MODE_PRIVATE)
        if (preferences.getBoolean(RETIRED, false)) return false
        return try {
            context.getSystemService(AlarmManager::class.java).cancel(alarmIntent(context))
            preferences
                .edit()
                .remove("next")
                .putBoolean(RETIRED, true)
                .commit()
        } catch (_: RuntimeException) {
            false
        }
    }

    fun schedulesRetired(context: Context): Boolean =
        context.getSharedPreferences("recording-host", Context.MODE_PRIVATE).getBoolean(RETIRED, false)

    private const val RETIRED = "schedules-retired"

    private fun alarmIntent(context: Context) =
        PendingIntent.getBroadcast(
            context,
            471,
            Intent(context, RecordingAlarm::class.java),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )

    /** Called on Rust's publication worker whenever whether a capture is
     * running or due changes; the UI engine never waits on Binder or disk
     * itself. While one is, the foreground service runs and a 120-second
     * watchdog alarm restarts it if Android killed it (#275); otherwise the
     * alarm is disarmed and the service stops. */
    @JvmStatic @Synchronized
    fun schedule(active: Boolean) {
        desiredActive = active
        try {
            val preferences = context.getSharedPreferences("recording-host", Context.MODE_PRIVATE)
            val wake = if (active) System.currentTimeMillis() / 1000 + 120 else 0
            // This version now owns the alarm: arming replaces any wake an
            // earlier version left, and a retirement retried later must not
            // cancel this one.
            preferences
                .edit()
                .putLong("next", wake)
                .putBoolean(RETIRED, true)
                .commit()
            arm(context, wake)
            if (active) {
                startService(context)
            } else {
                main.post {
                    // A later accepted record-now may overtake this queued idle.
                    if (!desiredActive) RecordingService.idle(context)
                }
            }
        } catch (_: RuntimeException) {
        }
    }

    /** Arms the watchdog alarm at `next` (UTC seconds), or disarms it for 0.
     * Exact where Android allows it without a permission (before Android
     * 12), otherwise inexact: Patina no longer asks for exact alarms. */
    fun arm(
        context: Context,
        next: Long,
    ) {
        try {
            val alarms = context.getSystemService(AlarmManager::class.java)
            val pending = alarmIntent(context)
            val exact = Build.VERSION.SDK_INT < 31 || alarms.canScheduleExactAlarms()
            alarms.cancel(pending)
            if (next > 0) {
                val at = maxOf(Math.multiplyExact(next, 1000L), System.currentTimeMillis() + 1000)
                if (exact) {
                    alarms.setExactAndAllowWhileIdle(AlarmManager.RTC_WAKEUP, at, pending)
                } else {
                    alarms.setAndAllowWhileIdle(AlarmManager.RTC_WAKEUP, at, pending)
                }
            }
        } catch (_: RuntimeException) {
        }
    }

    /** The watchdog fired while this process still records: Rust publishes
     * only when its schedule changes, so the next watchdog is armed here,
     * or a kill later in a long capture would have none (#275). A new
     * process starts idle and arms its own once its capture resumes. */
    @JvmStatic @Synchronized
    fun renewWatchdog(context: Context) {
        if (!desiredActive) return
        try {
            val wake = System.currentTimeMillis() / 1000 + 120
            context
                .getSharedPreferences("recording-host", Context.MODE_PRIVATE)
                .edit()
                .putLong("next", wake)
                .apply()
            arm(context, wake)
        } catch (_: RuntimeException) {
            // The service start below still runs; Rust's next publication rearms.
        }
    }

    fun startService(context: Context): Boolean =
        try {
            val intent = Intent(context, RecordingService::class.java)
            if (Build.VERSION.SDK_INT >= 26) context.startForegroundService(intent) else context.startService(intent)
            true
        } catch (_: RuntimeException) {
            // Android may refuse a non-exact background launch. Retry through
            // the registered alarm; the catalog retains the accepted job.
            arm(context, System.currentTimeMillis() / 1000 + 60)
            false
        }
}

class RecordingAlarm : BroadcastReceiver() {
    override fun onReceive(
        context: Context,
        intent: Intent,
    ) {
        // Until the wake an earlier version armed for a scheduled job is
        // retired, a broadcast retires it instead of acting on it; a failed
        // retirement is retried by the next launch or broadcast.
        if (!RecordingHost.schedulesRetired(context)) {
            RecordingHost.retireSchedules(context)
            return
        }
        // The Record now watchdog: rearm the next one while this process
        // still records, then restart the capture's service.
        RecordingHost.renewWatchdog(context)
        RecordingHost.startService(context)
    }
}

/** Native capture survives activity/engine destruction; it never owns a player view. */
class RecordingService : Service() {
    private val wakeHandler = Handler(Looper.getMainLooper())

    /** Low Power Standby cuts the network when the TV is turned off: the
     * capture stops then, keeping what was written, rather than failing
     * later (#275). Elsewhere the screen turning off changes nothing. */
    private val screenOff =
        object : BroadcastReceiver() {
            override fun onReceive(
                context: Context,
                intent: Intent,
            ) {
                if (!CaptureLimits.tv(context) || !CaptureLimits.standbyStops(context)) return
                val path = filesDir.absolutePath
                worker.execute {
                    val stopped = RecordingHost.stopCaptures(path, RecordingHost.STOP_TV_OFF)
                    android.util.Log.i("nsh", "iptv recordings: TV off under Low Power Standby; stopped $stopped")
                    if (stopped > 0) {
                        CaptureLimits.notifyStopped(localized(), R.string.recording_stopped_tv_off)
                    }
                }
            }
        }
    private var wakeLease: RecordingWakeLease? = null
    private val worker =
        java.util.concurrent.Executors
            .newSingleThreadExecutor()

    override fun onBind(intent: Intent?) = null

    /** This service in the app's interface language. */
    private fun localized(): Context {
        val tag = getSharedPreferences("recording-host", Context.MODE_PRIVATE).getString("language", "")
        val configuration = android.content.res.Configuration(resources.configuration)
        if (!tag.isNullOrEmpty()) configuration.setLocale(java.util.Locale.forLanguageTag(tag))
        return createConfigurationContext(configuration)
    }

    fun refreshNotification() {
        val localized = localized()
        val notifications = getSystemService(NotificationManager::class.java)
        if (Build.VERSION.SDK_INT >= 26) {
            notifications.createNotificationChannel(
                NotificationChannel(CHANNEL, localized.getString(R.string.recording_channel), NotificationManager.IMPORTANCE_LOW),
            )
        }
        val open =
            PendingIntent.getActivity(
                this,
                0,
                Intent(this, TvActivity::class.java),
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
        val builder = if (Build.VERSION.SDK_INT >= 26) Notification.Builder(this, CHANNEL) else Notification.Builder(this)
        startForeground(
            472,
            builder
                .setContentTitle(localized.getString(R.string.recording_title))
                .setContentText(localized.getString(R.string.recording_background))
                .setSmallIcon(android.R.drawable.ic_menu_save)
                .setContentIntent(open)
                .setOngoing(true)
                .build(),
        )
    }

    override fun onCreate() {
        super.onCreate()
        running = this
        refreshNotification()
        try {
            if (Build.VERSION.SDK_INT >= 33) {
                registerReceiver(screenOff, IntentFilter(Intent.ACTION_SCREEN_OFF), Context.RECEIVER_NOT_EXPORTED)
            } else {
                registerReceiver(screenOff, IntentFilter(Intent.ACTION_SCREEN_OFF))
            }
        } catch (_: RuntimeException) {
            // Without it a standby cut ends the capture as a network failure.
        }
        val wake =
            getSystemService(PowerManager::class.java)
                .newWakeLock(
                    PowerManager.PARTIAL_WAKE_LOCK,
                    "Patina:recording",
                ).also { it.setReferenceCounted(false) }
        wakeLease =
            RecordingWakeLease(
                acquire = { timeout -> wake.acquire(timeout) },
                schedule = { renewal, delay -> wakeHandler.postDelayed(renewal, delay) },
                cancel = { renewal -> wakeHandler.removeCallbacks(renewal) },
                release = { if (wake.isHeld) wake.release() },
            ).also { it.start() }
    }

    override fun onStartCommand(
        intent: Intent?,
        flags: Int,
        startId: Int,
    ): Int {
        worker.execute {
            try {
                System.loadLibrary("nsh")
                if (!CredentialKey.initialize(this)) {
                    stopSelf()
                    return@execute
                }
                RecordingHost.attach(this)
                if (RecordingHost.start(filesDir.absolutePath) == 0L) stopSelf()
            } catch (_: Exception) {
                stopSelf()
            }
        }
        return START_STICKY
    }

    // Android 15's dataSync budget is six background hours per 24 hours.
    // End the running captures as partial recordings that say why, tell the
    // viewer, and stop instead of letting the system crash us (#275).
    override fun onTimeout(
        startId: Int,
        fgsType: Int,
    ) {
        val path = filesDir.absolutePath
        worker.execute {
            val stopped = RecordingHost.stopCaptures(path, RecordingHost.STOP_BACKGROUND_LIMIT)
            android.util.Log.i("nsh", "iptv recordings: background time limit; stopped $stopped")
            if (stopped > 0) {
                CaptureLimits.notifyStopped(localized(), R.string.recording_stopped_background_limit)
            }
            RecordingHost.stop(path)
        }
        // Satisfy Android's short timeout immediately. Native cancellation and
        // spool publication finish off the main thread; a process kill still
        // leaves a recoverable Recording row and transport-packet spool.
        stopSelf()
    }

    override fun onDestroy() {
        try {
            unregisterReceiver(screenOff)
        } catch (_: RuntimeException) {
        }
        running = null
        wakeLease?.stop()
        wakeLease = null
        worker.shutdown()
        super.onDestroy()
    }

    companion object {
        private const val CHANNEL = "recording"
        private var running: RecordingService? = null

        fun refresh() {
            running?.refreshNotification()
        }

        fun idle(context: Context) {
            running?.stopSelf()
        }
    }
}
