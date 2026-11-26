package com.hantekllc.iptvrs.host

import android.app.Activity
import android.content.Intent
import android.content.pm.ApplicationInfo
import android.content.pm.PackageInfo
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.provider.Settings
import android.widget.Toast
import com.longbridge.nsh.FilesProvider
import java.io.File
import java.util.concurrent.Executors

/** Receives a private, length- and digest-verified download from the active viewer. */
class DirectUpdateActivity : Activity() {
    private val worker = Executors.newSingleThreadExecutor()
    private var file: File? = null
    private var waiting = 0

    override fun onCreate(state: Bundle?) {
        super.onCreate(state)
        val path = intent.getStringExtra("path")?.let(::File)
        if (path == null || !UpdateHost.pending(this, path, intent.getLongExtra("code", -1))) {
            finish()
            return
        }
        file = path
        waiting = state?.getInt("waiting") ?: 0
        if (waiting == 0) proceed()
    }

    override fun onSaveInstanceState(state: Bundle) {
        super.onSaveInstanceState(state)
        state.putInt("waiting", waiting)
    }

    @Suppress("DEPRECATION")
    private fun proceed() {
        if (RecordingHost.updateMustWait()) return stop(R.string.update_recording_active)
        val apk = file ?: return finish()
        val code = intent.getLongExtra("code", -1)
        worker.execute {
            val valid = runCatching { matchesInstalled(apk, code) }.getOrDefault(false)
            runOnUiThread {
                if (isDestroyed || isFinishing) return@runOnUiThread
                if (!valid) return@runOnUiThread stop(R.string.update_invalid_apk)
                if (RecordingHost.updateMustWait()) return@runOnUiThread stop(R.string.update_recording_active)
                if (!packageManager.canRequestPackageInstalls()) {
                    waiting = 1
                    try {
                        startActivityForResult(
                            Intent(
                                Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES,
                                Uri.fromParts("package", packageName, null),
                            ),
                            waiting,
                        )
                    } catch (_: RuntimeException) {
                        stop(R.string.update_install_unavailable)
                    }
                    return@runOnUiThread
                }
                try {
                    val uri = FilesProvider.uriFor(this, apk) ?: return@runOnUiThread stop(R.string.update_invalid_apk)
                    val install =
                        Intent(Intent.ACTION_INSTALL_PACKAGE)
                            .setDataAndType(uri, "application/vnd.android.package-archive")
                            .putExtra(Intent.EXTRA_RETURN_RESULT, true)
                            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                    // Grant access only to the OS package installer.
                    val handler =
                        packageManager
                            .queryIntentActivities(
                                install,
                                PackageManager.MATCH_SYSTEM_ONLY or PackageManager.MATCH_DEFAULT_ONLY,
                            ).map { it.activityInfo }
                            .firstOrNull {
                                it.applicationInfo.flags and ApplicationInfo.FLAG_SYSTEM != 0 &&
                                    packageManager.checkPermission(
                                        android.Manifest.permission.INSTALL_PACKAGES,
                                        it.packageName,
                                    ) == PackageManager.PERMISSION_GRANTED
                            }
                            ?: return@runOnUiThread stop(R.string.update_install_unavailable)
                    install.setClassName(handler.packageName, handler.name)
                    waiting = 2
                    startActivityForResult(install, waiting)
                } catch (_: RuntimeException) {
                    stop(R.string.update_install_unavailable)
                }
            }
        }
    }

    @Suppress("DEPRECATION")
    private fun matchesInstalled(
        apk: File,
        expected: Long,
    ): Boolean {
        val flags = if (Build.VERSION.SDK_INT >= 28) PackageManager.GET_SIGNING_CERTIFICATES else PackageManager.GET_SIGNATURES
        val old = packageManager.getPackageInfo(packageName, flags)
        // Android 9's archive parser only collects certificates with GET_SIGNATURES.
        val archiveFlags = if (Build.VERSION.SDK_INT == 28) flags or PackageManager.GET_SIGNATURES else flags
        val next = packageManager.getPackageArchiveInfo(apk.absolutePath, archiveFlags) ?: return false

        fun code(p: PackageInfo) = if (Build.VERSION.SDK_INT >= 28) p.longVersionCode else p.versionCode.toLong()
        if (next.packageName != packageName || code(next) != expected || expected <= code(old)) return false
        if (Build.VERSION.SDK_INT < 28) {
            val current = old.signatures?.toSet().orEmpty()
            return current.isNotEmpty() && current == next.signatures?.toSet()
        }
        val a = old.signingInfo ?: return false
        val b = next.signingInfo ?: return false
        val current = a.apkContentsSigners.toSet()
        if (current.isEmpty()) return false
        if (a.hasMultipleSigners() || b.hasMultipleSigners()) return current == b.apkContentsSigners.toSet()
        return b.signingCertificateHistory?.contains(current.single()) == true
    }

    @Deprecated("System installer and settings results for API 26+")
    override fun onActivityResult(
        request: Int,
        result: Int,
        data: Intent?,
    ) {
        super.onActivityResult(request, result, data)
        if (request != waiting) return
        waiting = 0
        when {
            request == 1 && packageManager.canRequestPackageInstalls() -> proceed()
            request == 2 && result == Activity.RESULT_FIRST_USER -> stop(R.string.update_install_unavailable)
            else -> finish()
        }
    }

    private fun stop(message: Int) {
        Toast.makeText(this, message, Toast.LENGTH_LONG).show()
        finish()
    }

    override fun onDestroy() {
        worker.shutdownNow()
        if (isFinishing) file?.let { UpdateHost.discard(this, it) }
        super.onDestroy()
    }
}
