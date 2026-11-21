package com.hantekllc.iptvrs.host

import android.app.Activity
import android.content.Intent
import android.os.Handler
import android.os.Looper
import android.system.Os
import android.system.OsConstants
import android.view.View
import com.longbridge.nsh.NshComponent
import com.longbridge.nsh.NshComponentContext
import java.io.File
import java.io.FileOutputStream
import java.io.InputStream
import java.io.OutputStream
import java.util.concurrent.atomic.AtomicBoolean

/**
 * App-owned bridge to Android's Storage Access Framework. Rust prepares a
 * private archive off the UI thread, then mounts `tv.backup` with strings
 * `archivePath` and `requestId`. `operation` is export (default) or import;
 * import streams a selected document into a new private file for Rust to
 * validate/stage, without loading the archive into memory. A successful import
 * leaves that file for Rust; export and all failed/canceled operations remove
 * their private temporary files. The `complete` event has only requestId,
 * success and canceled; neither paths, document URIs nor exception messages
 * cross the event/log boundary. After an explicit user confirmation, close
 * ends the activity and its process so staged restore activates on next launch.
 * No filesystem permission is needed.
 */
class BackupComponent(
    activity: Activity,
    private val nsh: NshComponentContext,
    private val host: BackupHost,
) : NshComponent {
    companion object { const val NAME = "tv.backup" }

    private val root = View(activity).apply {
        importantForAccessibility = View.IMPORTANT_FOR_ACCESSIBILITY_NO
        isFocusable = false
    }
    private val handler = Handler(Looper.getMainLooper())
    private var archivePath: String? = null
    private var requestId: String? = null
    private var operation = "export"
    private var handled: String? = null
    private var destroyed = false
    private val launch = Runnable {
        if (!destroyed) {
            val id = requestId
            val path = archivePath
            if (!id.isNullOrEmpty() && !path.isNullOrEmpty() && handled != id) {
                handled = id
                host.transfer(this, id, path, operation)
            }
        }
    }

    override fun view(): View = root

    override fun setProp(name: String, value: Any?) {
        when (name) {
            "archivePath" -> archivePath = value as? String
            "requestId" -> requestId = value as? String
            "operation" -> operation = (value as? String) ?: "export"
            else -> return
        }
        // NSH sets props individually. Wait until the whole update is applied.
        handler.removeCallbacks(launch)
        handler.post(launch)
    }

    internal fun complete(id: String, success: Boolean, canceled: Boolean) {
        if (!destroyed) {
            nsh.emit("complete", mapOf(
                "requestId" to id,
                "success" to success,
                "canceled" to canceled,
            ))
        }
    }

    override fun destroy() {
        destroyed = true
        handler.removeCallbacks(launch)
        host.componentDestroyed(this)
    }
}

/** Owns the one document chooser/copy operation for an activity. */
class BackupHost(private val activity: Activity) {
    private companion object {
        // Distinct from NSH's picker Fragment's own request codes.
        const val TRANSFER_BACKUP = 0x5042
        const val SUFFIX = ".patinabackup"
        const val MAX_BYTES = 8L * 1024 * 1024 * 1024
    }

    private class Request(
        val component: BackupComponent,
        val id: String,
        val archive: File,
        val importing: Boolean,
    ) {
        val canceled = AtomicBoolean(false)
        val ownsArchive = AtomicBoolean(!importing)
    }

    private val handler = Handler(Looper.getMainLooper())
    private var active: Request? = null
    private var destroyed = false

    /** Only delete an app-owned, directly contained regular temporary archive. */
    private fun privateArchive(path: String, importing: Boolean): File? = try {
        val file = File(path)
        val canonical = file.canonicalFile
        val directory = activity.filesDir.canonicalFile
        // Rust uses /data/data/<package>/files, whereas filesDir may name
        // /data/user/0/<package>/files. Allow that Android directory alias,
        // but reject a symlink at the archive file itself.
        val direct = File(file.parentFile?.canonicalFile ?: directory, file.name)
        if (file.isAbsolute && direct == canonical && canonical.parentFile == directory
            && canonical.name.startsWith(if (importing) "backup-import-" else "backup-export-")
            && canonical.name.endsWith(SUFFIX)
            && (if (importing) !canonical.exists() else canonical.isFile)) canonical else null
    } catch (_: Exception) { null }

    @Suppress("DEPRECATION")
    internal fun transfer(component: BackupComponent, id: String, path: String, operation: String) {
        if (operation == "close") {
            // Only the Rust confirmation dialog's explicit Close Patina action
            // sends this operation. Reopening an Activity alone may reuse old
            // Rust workers; end the process before the next launcher start.
            activity.finishAffinity()
            handler.postDelayed({ android.os.Process.killProcess(android.os.Process.myPid()) }, 100L)
            return
        }
        if (operation != "import" && operation != "export") {
            component.complete(id, success = false, canceled = false)
            return
        }
        val importing = operation == "import"
        val archive = privateArchive(path, importing)
        if (destroyed || archive == null) {
            component.complete(id, success = false, canceled = false)
            return
        }
        if (active != null) {
            // Never delete the archive of an already active request.
            if (!importing && archive != active?.archive) archive.delete()
            component.complete(id, success = false, canceled = false)
            return
        }
        val request = Request(component, id, archive, importing)
        active = request
        try {
            val action = if (importing) Intent.ACTION_OPEN_DOCUMENT else Intent.ACTION_CREATE_DOCUMENT
            activity.startActivityForResult(Intent(action).apply {
                addCategory(Intent.CATEGORY_OPENABLE)
                // Some file providers assign a generic MIME to backup files.
                type = if (importing) "*/*" else "application/octet-stream"
                if (!importing) putExtra(Intent.EXTRA_TITLE, "Patina.patinabackup")
            }, TRANSFER_BACKUP)
        } catch (_: Exception) {
            finish(request, success = false, canceled = false)
        }
    }

    /** True when this result belonged to the backup chooser. */
    fun activityResult(code: Int, result: Int, data: Intent?): Boolean {
        if (code != TRANSFER_BACKUP) return false
        val request = active ?: return true
        val uri = data?.data
        if (result != Activity.RESULT_OK || uri == null) {
            finish(request, success = false, canceled = result == Activity.RESULT_CANCELED)
            return true
        }
        val resolver = activity.applicationContext.contentResolver
        Thread({
            var success = false
            try {
                if (request.canceled.get()) throw java.io.IOException("Canceled")
                // Recheck ownership after the chooser. Import requires an
                // absent file; exclusive creation will never overwrite one.
                val archive = privateArchive(request.archive.path, request.importing)
                    ?: throw java.io.IOException("Archive unavailable")
                if (request.importing) {
                    val input = resolver.openInputStream(uri)
                        ?: throw java.io.IOException("Document unavailable")
                    input.use {
                        val descriptor = Os.open(archive.path,
                            OsConstants.O_WRONLY or OsConstants.O_CREAT or OsConstants.O_EXCL, 0x180)
                        request.ownsArchive.set(true)
                        FileOutputStream(descriptor).use { output ->
                            copy(request, input, output)
                            output.fd.sync()
                        }
                    }
                } else {
                    archive.inputStream().use { input ->
                        val output = resolver.openOutputStream(uri, "wt")
                            ?: throw java.io.IOException("Document unavailable")
                        output.use { copy(request, input, it) }
                    }
                }
                success = !request.canceled.get()
            } catch (_: Exception) {
                // Provider errors may quote URIs or credentials: never log them.
            } finally {
                // An imported provider document is read-only to this bridge;
                // never delete it, even when its private copy cannot be made.
                // A provider may return an existing document for CREATE_DOCUMENT.
                // Ownership is unknown, so never delete its URI on a failed copy.
                if (!success || !request.importing) removePrivateArchive(request)
                handler.post { finish(request, success, request.canceled.get()) }
            }
        }, "patina-backup-transfer").start()
        return true
    }

    private fun copy(request: Request, input: InputStream, output: OutputStream) {
        val buffer = ByteArray(64 * 1024)
        var total = 0L
        while (true) {
            if (request.canceled.get()) throw java.io.IOException("Canceled")
            val count = input.read(buffer)
            if (count < 0) break
            total += count
            if (total > MAX_BYTES) throw java.io.IOException("Backup too large")
            output.write(buffer, 0, count)
        }
        output.flush()
    }

    private fun removePrivateArchive(request: Request) {
        if (request.ownsArchive.get()) request.archive.delete()
    }

    private fun finish(request: Request, success: Boolean, canceled: Boolean) {
        if (active !== request) return
        active = null
        val wasCanceled = canceled || request.canceled.get()
        val accepted = success && !wasCanceled && !destroyed
        if (!accepted || !request.importing) removePrivateArchive(request)
        if (!destroyed) request.component.complete(request.id, accepted, wasCanceled)
    }

    internal fun componentDestroyed(component: BackupComponent) {
        val request = active ?: return
        if (request.component !== component) return
        request.canceled.set(true)
        removePrivateArchive(request)
        // Keep the request slot until its chooser/copy returns. Reusing the
        // request code now could deliver the old result to a new transfer.
    }

    fun destroy() {
        destroyed = true
        active?.let {
            it.canceled.set(true)
            removePrivateArchive(it)
        }
    }
}
