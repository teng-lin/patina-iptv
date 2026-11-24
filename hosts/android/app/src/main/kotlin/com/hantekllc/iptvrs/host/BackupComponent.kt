package com.hantekllc.iptvrs.host

import android.app.Activity
import android.content.ContentResolver
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Handler
import android.os.Looper
import android.provider.OpenableColumns
import android.system.Os
import android.system.OsConstants
import android.view.View
import com.longbridge.nsh.NshComponent
import com.longbridge.nsh.NshComponentContext
import java.io.File
import java.io.FileOutputStream
import java.io.InputStream
import java.io.OutputStream
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.SynchronousQueue
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

/**
 * App-owned bridge to Android's Storage Access Framework. Rust prepares a
 * private archive off the UI thread, then mounts `tv.backup` with strings
 * `archivePath` and `requestId`. `operation` is export (default) or import;
 * import streams a selected document into a new private file for Rust to
 * validate/stage, without loading the archive into memory. A successful import
 * leaves that file for Rust; export and all failed/canceled operations remove
 * their private temporary files. A successful import's `complete` event can
 * include a display basename for review. Paths, document URIs and exception
 * messages stay inside the bridge. After an explicit user confirmation, close
 * ends the activity and its process so staged restore activates on next launch.
 * No filesystem permission is needed.
 *
 * The component also says which document pickers resolve ([DocumentPickers]):
 * a `pickers` event with booleans `create` (export) and `open` (import), when
 * Rust sets or changes `probe` (on mount and when Settings opens) and when the
 * activity resumes. Rust offers each backup action only where its picker is.
 */
class BackupComponent(
    private val activity: Activity,
    private val nsh: NshComponentContext,
    private val host: BackupHost,
) : NshComponent {
    companion object {
        const val NAME = "tv.backup"
    }

    private val root =
        View(activity).apply {
            importantForAccessibility = View.IMPORTANT_FOR_ACCESSIBILITY_NO
            isFocusable = false
        }
    private val handler = Handler(Looper.getMainLooper())
    private var archivePath: String? = null
    private var requestId: String? = null
    private var operation = "export"
    private var handled: String? = null
    private var destroyed = false
    private val launch =
        Runnable {
            if (!destroyed) {
                val id = requestId
                val path = archivePath
                if (!id.isNullOrEmpty() && !path.isNullOrEmpty() && handled != id) {
                    handled = id
                    host.transfer(this, id, path, operation)
                }
            }
        }

    private val probe = Runnable { probe() }

    init {
        host.componentCreated(this)
    }

    override fun view(): View = root

    /** Reports which document pickers resolve now. */
    internal fun probe() {
        if (destroyed) return
        val pickers = DocumentPickers.probe(activity.packageManager)
        nsh.emit("pickers", mapOf("create" to pickers.first, "open" to pickers.second))
    }

    override fun setProp(
        name: String,
        value: Any?,
    ) {
        when (name) {
            "probe" -> {
                handler.removeCallbacks(probe)
                handler.post(probe)
                return
            }
            "archivePath" -> archivePath = value as? String
            "requestId" -> requestId = value as? String
            "operation" -> operation = (value as? String) ?: "export"
            else -> return
        }
        // NSH sets props individually. Wait until the whole update is applied.
        handler.removeCallbacks(launch)
        handler.post(launch)
    }

    internal fun complete(
        id: String,
        success: Boolean,
        canceled: Boolean,
        displayName: String? = null,
    ) {
        if (!destroyed) {
            nsh.emit(
                "complete",
                mapOf(
                    "requestId" to id,
                    "success" to success,
                    "canceled" to canceled,
                    "displayName" to displayName,
                ),
            )
        }
    }

    override fun destroy() {
        destroyed = true
        handler.removeCallbacks(launch)
        handler.removeCallbacks(probe)
        host.componentDestroyed(this)
    }
}

/**
 * The Storage Access Framework pickers a backup needs: one that creates a
 * document (export) and one that opens one (import). Many Android TV devices
 * ship neither; some answer these intents only with the TV framework's stub
 * package, whose screen says no app can open them and offers nothing to pick.
 */
internal object DocumentPickers {
    /** Packages that claim document intents without a usable picker. */
    private val STUBS = setOf("com.android.tv.frameworkpackagestubs")

    fun create(): Intent =
        Intent(Intent.ACTION_CREATE_DOCUMENT).apply {
            addCategory(Intent.CATEGORY_OPENABLE)
            type = "application/octet-stream"
            putExtra(Intent.EXTRA_TITLE, "Patina.patinabackup")
        }

    fun open(): Intent =
        Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
            addCategory(Intent.CATEGORY_OPENABLE)
            // Some file providers assign a generic MIME to backup files.
            type = "*/*"
        }

    /** Whether any of the packages whose activities resolve is a real picker. */
    fun usable(packages: Collection<String?>): Boolean = packages.any { it != null && it !in STUBS }

    private fun resolves(
        manager: PackageManager,
        intent: Intent,
    ): Boolean =
        try {
            usable(
                manager
                    .queryIntentActivities(intent, PackageManager.MATCH_DEFAULT_ONLY)
                    .map { it.activityInfo?.packageName },
            )
        } catch (_: Exception) {
            false
        }

    /** (create, open): which pickers resolve on this device now. */
    fun probe(manager: PackageManager): Pair<Boolean, Boolean> = resolves(manager, create()) to resolves(manager, open())
}

/** Owns the one document chooser/copy operation for an activity. */
class BackupHost(
    private val activity: Activity,
) {
    private companion object {
        // Distinct from NSH's picker Fragment's own request codes.
        const val TRANSFER_BACKUP = 0x5042
        const val SUFFIX = ".patinabackup"
        const val MAX_BYTES = 8L * 1024 * 1024 * 1024

        // An unresponsive provider must not hold the completed copy or queue metadata
        // requests behind a query that never returns.
        val displayNames = ThreadPoolExecutor(0, 1, 30L, TimeUnit.SECONDS, SynchronousQueue())
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
    private val components = mutableSetOf<BackupComponent>()
    private var active: Request? = null
    private var destroyed = false

    /** Only delete an app-owned, directly contained regular temporary archive. */
    private fun privateArchive(
        path: String,
        importing: Boolean,
    ): File? =
        try {
            val file = File(path)
            val canonical = file.canonicalFile
            val directory = activity.filesDir.canonicalFile
            // Rust uses /data/data/<package>/files, whereas filesDir may name
            // /data/user/0/<package>/files. Allow that Android directory alias,
            // but reject a symlink at the archive file itself.
            val direct = File(file.parentFile?.canonicalFile ?: directory, file.name)
            if (file.isAbsolute &&
                direct == canonical &&
                canonical.parentFile == directory &&
                canonical.name.startsWith(if (importing) "backup-import-" else "backup-export-") &&
                canonical.name.endsWith(SUFFIX) &&
                (if (importing) !canonical.exists() else canonical.isFile)
            ) {
                canonical
            } else {
                null
            }
        } catch (_: Exception) {
            null
        }

    @Suppress("DEPRECATION")
    internal fun transfer(
        component: BackupComponent,
        id: String,
        path: String,
        operation: String,
    ) {
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
            activity.startActivityForResult(
                if (importing) DocumentPickers.open() else DocumentPickers.create(),
                TRANSFER_BACKUP,
            )
        } catch (_: Exception) {
            finish(request, success = false, canceled = false)
        }
    }

    /** True when this result belonged to the backup chooser. */
    fun activityResult(
        code: Int,
        result: Int,
        data: Intent?,
    ): Boolean {
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
            var displayName: String? = null
            try {
                if (request.canceled.get()) throw java.io.IOException("Canceled")
                // Recheck ownership after the chooser. Import requires an
                // absent file; exclusive creation will never overwrite one.
                val archive =
                    privateArchive(request.archive.path, request.importing)
                        ?: throw java.io.IOException("Archive unavailable")
                if (request.importing) {
                    val input =
                        resolver.openInputStream(uri)
                            ?: throw java.io.IOException("Document unavailable")
                    input.use {
                        val descriptor =
                            Os.open(
                                archive.path,
                                OsConstants.O_WRONLY or OsConstants.O_CREAT or OsConstants.O_EXCL,
                                0x180,
                            )
                        request.ownsArchive.set(true)
                        FileOutputStream(descriptor).use { output ->
                            copy(request, input, output)
                            output.fd.sync()
                        }
                    }
                    displayName = tryDisplayName(resolver, uri)
                } else {
                    archive.inputStream().use { input ->
                        val output =
                            resolver.openOutputStream(uri, "wt")
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
                handler.post { finish(request, success, request.canceled.get(), displayName) }
            }
        }, "patina-backup-transfer").start()
        return true
    }

    private fun tryDisplayName(
        resolver: ContentResolver,
        uri: Uri,
    ): String? {
        val reply =
            try {
                displayNames.submit<String?> {
                    resolver
                        .query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)
                        ?.use { cursor ->
                            if (cursor.moveToFirst()) {
                                cursor
                                    .getString(0)
                                    ?.substringAfterLast('/')
                                    ?.substringAfterLast('\\')
                                    ?.take(255)
                                    ?.trimEnd { it.isHighSurrogate() }
                            } else {
                                null
                            }
                        }
                }
            } catch (_: RejectedExecutionException) {
                return null
            }
        return try {
            reply.get(500, TimeUnit.MILLISECONDS)
        } catch (_: Exception) {
            reply.cancel(true)
            null
        }
    }

    private fun copy(
        request: Request,
        input: InputStream,
        output: OutputStream,
    ) {
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

    private fun finish(
        request: Request,
        success: Boolean,
        canceled: Boolean,
        displayName: String? = null,
    ) {
        if (active !== request) return
        active = null
        val wasCanceled = canceled || request.canceled.get()
        val accepted = success && !wasCanceled && !destroyed
        if (!accepted || !request.importing) removePrivateArchive(request)
        if (!destroyed) {
            request.component.complete(
                request.id,
                accepted,
                wasCanceled,
                displayName.takeIf { accepted && request.importing },
            )
        }
    }

    internal fun componentCreated(component: BackupComponent) {
        components.add(component)
    }

    /** The activity resumed: a picker may have been installed or removed meanwhile. */
    fun resumed() {
        for (component in components.toList()) component.probe()
    }

    internal fun componentDestroyed(component: BackupComponent) {
        components.remove(component)
        val request = active ?: return
        if (request.component !== component) return
        request.canceled.set(true)
        removePrivateArchive(request)
        // Keep the request slot until its chooser/copy returns. Reusing the
        // request code now could deliver the old result to a new transfer.
    }

    fun destroy() {
        destroyed = true
        components.clear()
        active?.let {
            it.canceled.set(true)
            removePrivateArchive(it)
        }
    }
}
