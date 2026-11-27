package com.hantekllc.iptvrs.host

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.AtomicFile
import java.io.File
import java.io.InputStream
import java.security.KeyStore
import java.security.SecureRandom
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/** Only authenticated ciphertext is stored beside the catalog; the wrapping
 * key is non-exportable and owned by Android Keystore for this application's UID. */
object CredentialKey {
    private const val ALIAS = "iptv-rs.catalog.v1"
    /** The wrapped file: GCM IV (12 bytes), the 32-byte key, GCM tag (16). */
    internal const val WRAPPED_SIZE = 60
    private val SQLITE_HEADER = "SQLite format 3\u0000".toByteArray(Charsets.US_ASCII)
    @JvmStatic private external fun installDatabaseKey(key: ByteArray): Boolean

    @Synchronized fun initialize(context: Context): Boolean = try {
        val wrapped = AtomicFile(File(context.noBackupFilesDir, "catalog-key.v1"))
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        val exists = wrapped.baseFile.exists() || File(wrapped.baseFile.path + ".bak").exists()
        val catalog = File(context.filesDir, "iptv.sqlite")
        if (refuses(exists, store.containsAlias(ALIAS), encryptedCatalog(catalog))) {
            false // Never replace a lost wrapping key or overwrite its ciphertext.
        } else {
            val wrapping = if (store.containsAlias(ALIAS)) {
                store.getKey(ALIAS, null) as SecretKey
            } else {
                KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").apply {
                    init(KeyGenParameterSpec.Builder(ALIAS,
                        KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                        .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                        .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                        .setKeySize(256).build())
                }.generateKey()
            }
            val key = if (exists) {
                unwrap(wrapping, wrapped.openRead().use(::readWrapped))
            } else {
                ByteArray(32).also { bytes ->
                    SecureRandom().nextBytes(bytes)
                    try {
                        val payload = wrap(wrapping, bytes)
                        val output = wrapped.startWrite()
                        try {
                            output.write(payload)
                            wrapped.finishWrite(output)
                        } catch (error: Exception) {
                            wrapped.failWrite(output)
                            throw error
                        }
                    } catch (error: Exception) {
                        bytes.fill(0)
                        throw error
                    }
                }
            }
            try {
                require(key.size == 32)
                installDatabaseKey(key)
            } finally { key.fill(0) }
        }
    } catch (_: Exception) {
        // No exception message or credential material enters Android logs.
        false
    }

    /**
     * Whether to give up instead of making a new key: the wrapped key's file
     * is there but the Keystore key that unwraps it is gone, or there is no
     * wrapped key but the catalog is already encrypted (with a key now lost).
     */
    internal fun refuses(wrappedExists: Boolean, wrappingExists: Boolean, encryptedCatalog: Boolean) =
        (wrappedExists && !wrappingExists) || (!wrappedExists && encryptedCatalog)

    /** Whether `catalog` has content that is not plaintext SQLite (SQLCipher's, or unreadable). */
    internal fun encryptedCatalog(catalog: File): Boolean =
        catalog.exists() && catalog.length() > 0 && catalog.inputStream().use {
            val header = ByteArray(16)
            it.read(header) != 16 || !header.contentEquals(SQLITE_HEADER)
        }

    /** The wrapped key's [WRAPPED_SIZE] bytes; anything shorter or longer is refused. */
    internal fun readWrapped(input: InputStream): ByteArray {
        val data = ByteArray(WRAPPED_SIZE + 1)
        var count = 0
        while (count < data.size) {
            val n = input.read(data, count, data.size - count)
            if (n < 0) break
            count += n
        }
        require(count == WRAPPED_SIZE && input.read() == -1)
        return data.copyOf(count)
    }

    /** `key` encrypted with `wrapping` (AES-GCM): IV, ciphertext and tag. */
    internal fun wrap(
        wrapping: SecretKey,
        key: ByteArray,
        associatedData: ByteArray? = null,
    ): ByteArray {
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, wrapping)
        require(cipher.iv.size == 12)
        associatedData?.let(cipher::updateAAD)
        return cipher.iv + cipher.doFinal(key)
    }

    /** The key [wrap] wrapped; throws if `payload` was altered or another key wrapped it. */
    internal fun unwrap(
        wrapping: SecretKey,
        payload: ByteArray,
        associatedData: ByteArray? = null,
    ): ByteArray =
        Cipher.getInstance("AES/GCM/NoPadding").run {
            init(Cipher.DECRYPT_MODE, wrapping, GCMParameterSpec(128, payload.copyOfRange(0, 12)))
            associatedData?.let(::updateAAD)
            doFinal(payload, 12, payload.size - 12)
        }
}
