package com.hantekllc.iptvrs.host

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.AtomicFile
import java.io.File
import java.io.InputStream
import java.security.KeyStore
import java.security.MessageDigest
import javax.crypto.AEADBadTagException
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey

/** Receiver credentials stay in no-backup storage under an application-owned Keystore key. */
internal object CastCredentials {
    private const val ALIAS = "iptv-rs.airplay.v1"
    internal const val MAX_CREDENTIAL_SIZE = 4096
    internal const val MAX_SEALED_SIZE = MAX_CREDENTIAL_SIZE + 12 + 16
    const val MISSING: Byte = 0
    const val PRESENT: Byte = 1
    const val INVALID: Byte = 2

    @Synchronized fun load(
        context: Context,
        key: String,
    ): ByteArray {
        val (file, associatedData) = record(context, key)
        if (!exists(file)) return byteArrayOf(MISSING)
        val wrapping = wrappingKey(create = false) ?: return byteArrayOf(INVALID)
        val sealed = file.openRead().use(::readSealed) ?: return byteArrayOf(INVALID)
        val credential =
            try {
                CredentialKey.unwrap(wrapping, sealed, associatedData)
            } catch (_: AEADBadTagException) {
                return byteArrayOf(INVALID)
            }
        return try {
            byteArrayOf(PRESENT) + credential
        } finally {
            credential.fill(0)
        }
    }

    @Synchronized fun save(
        context: Context,
        key: String,
        credential: ByteArray,
    ) {
        try {
            require(credential.size in 1..MAX_CREDENTIAL_SIZE)
            val (file, associatedData) = record(context, key)
            // An existing record with a missing wrapping key must be explicitly forgotten.
            val wrapping = checkNotNull(wrappingKey(create = !exists(file)))
            val sealed = CredentialKey.wrap(wrapping, credential, associatedData)
            val output = file.startWrite()
            try {
                output.write(sealed)
                output.fd.sync()
                file.finishWrite(output)
            } catch (error: Exception) {
                file.failWrite(output)
                throw error
            }
            // AtomicFile reports a failed rename via Android logging.
            // A successful pairing receipt requires the new record on disk.
            check(file.openRead().use(::readSealed)?.contentEquals(sealed) == true)
        } finally {
            credential.fill(0)
        }
    }

    @Synchronized fun forget(
        context: Context,
        key: String,
    ) {
        val (file) = record(context, key)
        file.delete()
        check(!exists(file) && !File(file.baseFile.path + ".new").exists())
    }

    private fun wrappingKey(create: Boolean): SecretKey? {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        if (store.containsAlias(ALIAS)) return store.getKey(ALIAS, null) as? SecretKey
        if (!create) return null
        return KeyGenerator
            .getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
            .apply {
                init(
                    KeyGenParameterSpec
                        .Builder(ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                        .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                        .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                        .setKeySize(256)
                        .build(),
                )
            }.generateKey()
    }

    private fun record(
        context: Context,
        key: String,
    ): Pair<AtomicFile, ByteArray> {
        val associatedData = key.toByteArray(Charsets.UTF_8)
        val bytes = MessageDigest.getInstance("SHA-256").digest(associatedData)
        val digest = bytes.joinToString("") { "%02x".format(it) }
        return AtomicFile(File(context.noBackupFilesDir, "airplay-v1-$digest")) to associatedData
    }

    private fun exists(file: AtomicFile): Boolean = file.baseFile.exists() || File(file.baseFile.path + ".bak").exists()

    internal fun readSealed(input: InputStream): ByteArray? {
        val bytes = ByteArray(MAX_SEALED_SIZE + 1)
        var count = 0
        while (count < bytes.size) {
            val read = input.read(bytes, count, bytes.size - count)
            if (read < 0) break
            count += read
        }
        return if (count in 29..MAX_SEALED_SIZE) bytes.copyOf(count) else null
    }
}
