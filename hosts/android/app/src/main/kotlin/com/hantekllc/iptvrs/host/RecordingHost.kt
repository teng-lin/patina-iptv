package com.hantekllc.iptvrs.host

import android.app.*
import android.content.*
import android.os.*
import android.provider.Settings
import android.net.Uri

/** Only UTC scheduling metadata crosses JNI; streams and credentials stay in Rust. */
object RecordingHost {
    @Volatile private var desiredActive = false
    private lateinit var context: Context
    private var activity: java.lang.ref.WeakReference<Activity>? = null
    @JvmStatic fun requestAlarmAccess() { main.post { activity?.get()?.let { requestExactAlarms(it) } } }
    private val main = Handler(Looper.getMainLooper())
    @JvmStatic private external fun initialize()
    @JvmStatic external fun start(path: String): Long
    @JvmStatic external fun stop(path: String)
    @JvmStatic external fun suspendViewing(path: String)
    @JvmStatic external fun resumeViewing(path: String): Boolean
    fun attach(context: Context) {
        if (context is Activity) activity = java.lang.ref.WeakReference(context)
        this.context = context.applicationContext
        initialize()
    }
    private fun alarmIntent(context: Context) = PendingIntent.getBroadcast(
        context, 471, Intent(context, RecordingAlarm::class.java),
        PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
    /** Exact wakeups require the user's Special app access approval on Android 12+. */
    fun exactAlarmsAllowed(context: Context): Boolean = Build.VERSION.SDK_INT < 31 ||
        context.getSystemService(AlarmManager::class.java).canScheduleExactAlarms()
    fun requestExactAlarms(activity: Activity) {
        if (Build.VERSION.SDK_INT >= 31 && !exactAlarmsAllowed(activity)) {
            val packageUri = Uri.parse("package:${activity.packageName}")
            val request = Intent(Settings.ACTION_REQUEST_SCHEDULE_EXACT_ALARM, packageUri)
            try { activity.startActivity(request) } catch (_: ActivityNotFoundException) {
                activity.startActivity(Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, packageUri))
            }
        }
    }
    /** Called on Rust's publication worker. Durable OS registration completes
     * before this returns; the UI engine never waits on Binder or disk itself.
     * Codes: 1 exact, 2 inexact (permission missing), 3 registration denied. */
    @JvmStatic @Synchronized fun schedule(next: Long, active: Boolean): Int {
        desiredActive = active
        return try {
            val preferences = context.getSharedPreferences("recording-host", Context.MODE_PRIVATE)
            val wake = if (active) {
                val watchdog = System.currentTimeMillis() / 1000 + 120
                if (next > 0) minOf(next, watchdog) else watchdog
            } else next
            if (!preferences.edit().putLong("next", wake).commit()) return 3
            val capability = arm(context, wake)
            if (active) {
                if (!startService(context)) return 3
            } else main.post {
                // A later accepted record-now may overtake this queued idle.
                if (!desiredActive) RecordingService.idle(context)
            }
            capability
        } catch (_: RuntimeException) { 3 }
    }
    fun arm(context: Context, next: Long): Int = try {
        val alarms = context.getSystemService(AlarmManager::class.java)
        val pending = alarmIntent(context)
        val exact = exactAlarmsAllowed(context)
        alarms.cancel(pending)
        if (next > 0) {
            val at = maxOf(Math.multiplyExact(next, 1000L), System.currentTimeMillis() + 1000)
            if (exact) alarms.setExactAndAllowWhileIdle(AlarmManager.RTC_WAKEUP, at, pending)
            else alarms.setAndAllowWhileIdle(AlarmManager.RTC_WAKEUP, at, pending)
        }
        if (exact) 1 else 2
    } catch (_: RuntimeException) { 3 }
    fun restore(context: Context) {
        val next = context.getSharedPreferences("recording-host", Context.MODE_PRIVATE).getLong("next", 0)
        arm(context, if (next > 0) next else System.currentTimeMillis() / 1000 + 5)
    }
    fun startService(context: Context): Boolean {
        return try {
            val intent = Intent(context, RecordingService::class.java)
            if (Build.VERSION.SDK_INT >= 26) context.startForegroundService(intent) else context.startService(intent)
            true
        } catch (_: RuntimeException) {
            // Android may refuse a non-exact background launch. Retry through
            // the registered alarm; the catalogue retains the accepted job.
            arm(context, System.currentTimeMillis() / 1000 + 60)
            false
        }
    }
}

class RecordingAlarm : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action == Intent.ACTION_BOOT_COMPLETED || intent.action == Intent.ACTION_MY_PACKAGE_REPLACED ||
            intent.action == AlarmManager.ACTION_SCHEDULE_EXACT_ALARM_PERMISSION_STATE_CHANGED) {
            RecordingHost.restore(context)
        } else RecordingHost.startService(context)
    }
}

/** Native capture survives activity/engine destruction; it never owns a player view. */
class RecordingService : Service() {
    private var wake: PowerManager.WakeLock? = null
    private val worker = java.util.concurrent.Executors.newSingleThreadExecutor()
    override fun onBind(intent: Intent?) = null
    override fun onCreate() {
        super.onCreate()
        running = this
        val notifications = getSystemService(NotificationManager::class.java)
        if (Build.VERSION.SDK_INT >= 26) notifications.createNotificationChannel(
            NotificationChannel(CHANNEL, "Scheduled recordings", NotificationManager.IMPORTANCE_LOW))
        val open = PendingIntent.getActivity(this, 0, Intent(this, TvActivity::class.java),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
        val builder = if (Build.VERSION.SDK_INT >= 26) Notification.Builder(this, CHANNEL) else Notification.Builder(this)
        startForeground(472, builder.setContentTitle("Patina is recording")
            .setContentText("Your scheduled recording runs in the background")
            .setSmallIcon(android.R.drawable.ic_menu_save).setContentIntent(open).setOngoing(true).build())
        wake = getSystemService(PowerManager::class.java).newWakeLock(PowerManager.PARTIAL_WAKE_LOCK,
            "Patina:recording").also { it.acquire(6 * 60 * 60 * 1000L) }
    }
    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        worker.execute {
            try {
                System.loadLibrary("nsh")
                if (!CredentialKey.initialize(this)) { stopSelf(); return@execute }
                RecordingHost.attach(this)
                if (RecordingHost.start(filesDir.absolutePath) == 0L) stopSelf()
            } catch (_: Exception) { stopSelf() }
        }
        return START_STICKY
    }
    // Android 15's dataSync budget is six background hours per 24 hours.
    // Preserve the spool and pending jobs instead of letting the system crash us.
    override fun onTimeout(startId: Int, fgsType: Int) {
        worker.execute { RecordingHost.stop(filesDir.absolutePath) }
        // Satisfy Android's short timeout immediately. Native cancellation and
        // spool publication finish off the main thread; a process kill still
        // leaves a recoverable Recording row and transport-packet spool.
        stopSelf()
    }
    override fun onDestroy() {
        running = null
        wake?.let { if (it.isHeld) it.release() }
        worker.shutdown()
        super.onDestroy()
    }
    companion object {
        private const val CHANNEL = "recording"
        private var running: RecordingService? = null
        fun idle(context: Context) { running?.stopSelf() }
    }
}
