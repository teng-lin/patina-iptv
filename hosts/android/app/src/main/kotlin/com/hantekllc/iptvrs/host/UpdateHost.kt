package com.hantekllc.iptvrs.host

import android.app.Activity
import android.content.Context
import android.content.Intent
import java.io.File
import java.lang.ref.WeakReference

/** The installer activity exists only in the directly distributed APK. */
internal object UpdateHost {
    private var owner = WeakReference<Activity>(null)

    @JvmStatic private external fun initialize()

    fun attach(activity: Activity) {
        if (BuildConfig.BUILD_TYPE == "directRelease") {
            owner = WeakReference(activity)
            initialize()
            val pending = activity.getSharedPreferences("pending-app-update", Context.MODE_PRIVATE)
            val path = pending.getString("path", null)?.let(::File)
            if (path != null && pending.getLong("code", Long.MAX_VALUE) <= BuildConfig.VERSION_CODE) {
                discard(activity, path)
            }
            // Also retire interrupted downloads left by a killed process.
            Thread {
                val cutoff = System.currentTimeMillis() - 2 * 24 * 60 * 60 * 1000L
                activity.cacheDir.listFiles()?.filter { owns(activity, it) && it.lastModified() < cutoff }
                    ?.forEach { discard(activity, it) }
            }.start()
        }
    }

    fun active() = owner.get() != null
    fun owns(context: Context, path: File): Boolean =
        path.name.startsWith("patina-update-") && path.name.endsWith(".apk") &&
            path.parentFile == context.cacheDir && path.canonicalFile == path

    @Synchronized fun discard(context: Context, file: File) {
        if (owns(context, file)) file.delete()
        val pending = context.getSharedPreferences("pending-app-update", Context.MODE_PRIVATE)
        if (pending.getString("path", null) == file.path) pending.edit().clear().apply()
    }

    @JvmStatic fun cacheDirectory(): String? = owner.get()?.cacheDir?.absolutePath

    @JvmStatic fun open(path: String, code: Long): Boolean {
        val activity = owner.get() ?: return false
        if (activity.isFinishing || activity.isDestroyed || !activity.hasWindowFocus()
            || RecordingHost.updateMustWait()) return false
        return try {
            if (!owns(activity, File(path))) return false
            val pending = activity.getSharedPreferences("pending-app-update", Context.MODE_PRIVATE)
            if (!pending.edit().putLong("code", code).putString("path", path).commit()) return false
            activity.startActivity(Intent().setClassName(activity,
                "com.hantekllc.iptvrs.host.DirectUpdateActivity")
                .putExtra("path", path).putExtra("code", code))
            true
        } catch (_: RuntimeException) {
            false
        }
    }
}
