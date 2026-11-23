package com.hantekllc.iptvrs.host

import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.net.Uri

/** Starts another installed video app with an opaque Patina loopback URL. */
internal object ExternalPlayerHost {
    @Volatile private var application: Context? = null

    @JvmStatic private external fun initialize()

    fun attach(context: Context) {
        application = context.applicationContext
        initialize()
    }

    @JvmStatic fun open(address: String): Boolean {
        val context = application ?: return false
        val uri = Uri.parse(address)
        if (uri.scheme != "http" || uri.host != "127.0.0.1") return false
        val video = Intent(Intent.ACTION_VIEW).setDataAndType(uri, "video/*")
        if (video.resolveActivity(context.packageManager) == null) return false
        return try {
            val chooser = Intent.createChooser(video, null).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            context.startActivity(chooser)
            true
        } catch (_: ActivityNotFoundException) {
            false
        } catch (_: SecurityException) {
            false
        }
    }
}
