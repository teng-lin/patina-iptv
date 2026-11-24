package com.hantekllc.iptvrs.host

import android.app.Activity
import android.app.AlarmManager
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.ActivityNotFoundException
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.PowerManager
import android.provider.Settings

/** Scheduling metadata and interface language cross JNI; credentials stay in Rust. */
object RecordingHost {
    @Volatile private var desiredActive = false
    private lateinit var context: Context
    private var activity: java.lang.ref.WeakReference<Activity>? = null

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

    @JvmStatic fun requestAlarmAccess() {
        main.post { activity?.get()?.let { requestExactAlarms(it) } }
    }

    private val main = Handler(Looper.getMainLooper())

    @JvmStatic private external fun initialize()

    @JvmStatic external fun start(path: String): Long

    @JvmStatic external fun stop(path: String)

    @JvmStatic external fun suspendViewing(path: String)

    @JvmStatic external fun resumeViewing(path: String): Boolean

    fun attach(context: Context) {
        if (context is Activity) activity = java.lang.ref.WeakReference(context)
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

    private const val RETIRED = "schedules-retired"

    private fun alarmIntent(context: Context) =
        PendingIntent.getBroadcast(
            context,
            471,
            Intent(context, RecordingAlarm::class.java),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )

    /** Exact wakeups require the user's Special app access approval on Android 12+. */
    fun exactAlarmsAllowed(context: Context): Boolean =
        Build.VERSION.SDK_INT < 31 ||
            context.getSystemService(AlarmManager::class.java).canScheduleExactAlarms()

    fun requestExactAlarms(activity: Activity) {
        if (Build.VERSION.SDK_INT >= 31 && !exactAlarmsAllowed(activity)) {
            val packageUri = Uri.parse("package:${activity.packageName}")
            val request = Intent(Settings.ACTION_REQUEST_SCHEDULE_EXACT_ALARM, packageUri)
            try {
                activity.startActivity(request)
            } catch (_: ActivityNotFoundException) {
                activity.startActivity(Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, packageUri))
            }
        }
    }

    /** Called on Rust's publication worker. Durable OS registration completes
     * before this returns; the UI engine never waits on Binder or disk itself.
     * Codes: 1 exact, 2 inexact (permission missing), 3 registration denied. */
    @JvmStatic @Synchronized
    fun schedule(
        next: Long,
        active: Boolean,
    ): Int {
        desiredActive = active
        return try {
            val preferences = context.getSharedPreferences("recording-host", Context.MODE_PRIVATE)
            val wake =
                if (active) {
                    val watchdog = System.currentTimeMillis() / 1000 + 120
                    if (next > 0) minOf(next, watchdog) else watchdog
                } else {
                    next
                }
            // This version now owns the alarm: arming replaces any wake an
            // earlier version left, and a retirement retried later must not
            // cancel this one.
            if (!preferences
                    .edit()
                    .putLong("next", wake)
                    .putBoolean(RETIRED, true)
                    .commit()
            ) {
                return 3
            }
            val capability = arm(context, wake)
            if (active) {
                if (!startService(context)) return 3
            } else {
                main.post {
                    // A later accepted record-now may overtake this queued idle.
                    if (!desiredActive) RecordingService.idle(context)
                }
            }
            capability
        } catch (_: RuntimeException) {
            3
        }
    }

    fun arm(
        context: Context,
        next: Long,
    ): Int =
        try {
            val alarms = context.getSystemService(AlarmManager::class.java)
            val pending = alarmIntent(context)
            val exact = exactAlarmsAllowed(context)
            alarms.cancel(pending)
            if (next > 0) {
                val at = maxOf(Math.multiplyExact(next, 1000L), System.currentTimeMillis() + 1000)
                if (exact) {
                    alarms.setExactAndAllowWhileIdle(AlarmManager.RTC_WAKEUP, at, pending)
                } else {
                    alarms.setAndAllowWhileIdle(AlarmManager.RTC_WAKEUP, at, pending)
                }
            }
            if (exact) 1 else 2
        } catch (_: RuntimeException) {
            3
        }

    /** Rearms the stored wake after a reboot or update. Nothing is scheduled
     * any more: a stored time is the watchdog of a Record now capture, and
     * without one there is nothing to start. */
    fun restore(context: Context) {
        val next = context.getSharedPreferences("recording-host", Context.MODE_PRIVATE).getLong("next", 0)
        if (next > 0) arm(context, next)
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
        // The first broadcast after updating from a version that scheduled
        // recordings retires its wake instead of acting on it.
        if (RecordingHost.retireSchedules(context)) return
        if (intent.action == Intent.ACTION_BOOT_COMPLETED ||
            intent.action == Intent.ACTION_MY_PACKAGE_REPLACED ||
            intent.action == AlarmManager.ACTION_SCHEDULE_EXACT_ALARM_PERMISSION_STATE_CHANGED
        ) {
            RecordingHost.restore(context)
        } else {
            RecordingHost.startService(context)
        }
    }
}

/** Native capture survives activity/engine destruction; it never owns a player view. */
class RecordingService : Service() {
    private val wakeHandler = Handler(Looper.getMainLooper())
    private var wakeLease: RecordingWakeLease? = null
    private val worker =
        java.util.concurrent.Executors
            .newSingleThreadExecutor()

    override fun onBind(intent: Intent?) = null

    fun refreshNotification() {
        val tag = getSharedPreferences("recording-host", Context.MODE_PRIVATE).getString("language", "")
        val configuration = android.content.res.Configuration(resources.configuration)
        if (!tag.isNullOrEmpty()) configuration.setLocale(java.util.Locale.forLanguageTag(tag))
        val localized = createConfigurationContext(configuration)
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
    // Preserve the spool and pending jobs instead of letting the system crash us.
    override fun onTimeout(
        startId: Int,
        fgsType: Int,
    ) {
        worker.execute { RecordingHost.stop(filesDir.absolutePath) }
        // Satisfy Android's short timeout immediately. Native cancellation and
        // spool publication finish off the main thread; a process kill still
        // leaves a recoverable Recording row and transport-packet spool.
        stopSelf()
    }

    override fun onDestroy() {
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
