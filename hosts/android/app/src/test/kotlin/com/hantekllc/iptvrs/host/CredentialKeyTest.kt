package com.hantekllc.iptvrs.host

import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.ByteArrayInputStream
import javax.crypto.AEADBadTagException
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey

/** The pure parts of [CredentialKey]; a software AES key stands in for the Keystore's. */
class CredentialKeyTest {
    @get:Rule val folder = TemporaryFolder()

    private fun aes(): SecretKey = KeyGenerator.getInstance("AES").apply { init(256) }.generateKey()

    @Test fun aLostKeyOrAnOrphanedCatalogIsNeverReplaced() {
        // A first run, and a later one with both halves of the key.
        assertFalse(CredentialKey.refuses(wrappedExists = false, wrappingExists = false, encryptedCatalog = false))
        assertFalse(CredentialKey.refuses(wrappedExists = false, wrappingExists = true, encryptedCatalog = false))
        assertFalse(CredentialKey.refuses(wrappedExists = true, wrappingExists = true, encryptedCatalog = true))
        assertFalse(CredentialKey.refuses(wrappedExists = true, wrappingExists = true, encryptedCatalog = false))
        // The Keystore lost its key (a restore to another device, a reset):
        // the wrapped key cannot be read and must not be overwritten.
        assertTrue(CredentialKey.refuses(wrappedExists = true, wrappingExists = false, encryptedCatalog = false))
        assertTrue(CredentialKey.refuses(wrappedExists = true, wrappingExists = false, encryptedCatalog = true))
        // An encrypted catalog without its wrapped key: a new key could not open it.
        assertTrue(CredentialKey.refuses(wrappedExists = false, wrappingExists = false, encryptedCatalog = true))
        assertTrue(CredentialKey.refuses(wrappedExists = false, wrappingExists = true, encryptedCatalog = true))
    }

    @Test fun onlyAPlaintextSqliteHeaderCountsAsUnencrypted() {
        val catalog = folder.root.resolve("iptv.sqlite")
        assertFalse(CredentialKey.encryptedCatalog(catalog))
        catalog.writeBytes(ByteArray(0))
        assertFalse(CredentialKey.encryptedCatalog(catalog))
        catalog.writeBytes("SQLite format 3\u0000".toByteArray(Charsets.US_ASCII) + ByteArray(84))
        assertFalse(CredentialKey.encryptedCatalog(catalog))
        catalog.writeBytes(ByteArray(4096) { (it * 31 + 7).toByte() })
        assertTrue(CredentialKey.encryptedCatalog(catalog))
        // Shorter than the header: not something SQLite wrote in the clear.
        catalog.writeBytes("SQLite format".toByteArray(Charsets.US_ASCII))
        assertTrue(CredentialKey.encryptedCatalog(catalog))
    }

    @Test fun theWrappedKeyRoundTripsAndRefusesTampering() {
        val wrapping = aes()
        val key = ByteArray(32) { it.toByte() }
        val payload = CredentialKey.wrap(wrapping, key)
        assertEquals(CredentialKey.WRAPPED_SIZE, payload.size)
        assertArrayEquals(key, CredentialKey.unwrap(wrapping, payload))
        // A fresh IV every time.
        assertFalse(payload.copyOfRange(0, 12).contentEquals(CredentialKey.wrap(wrapping, key).copyOfRange(0, 12)))
        for (index in listOf(0, 11, 12, 43, 44, 59)) {
            val altered = payload.copyOf().also { it[index] = (it[index].toInt() xor 1).toByte() }
            assertThrows(AEADBadTagException::class.java) { CredentialKey.unwrap(wrapping, altered) }
        }
        assertThrows(AEADBadTagException::class.java) { CredentialKey.unwrap(aes(), payload) }
    }

    @Test fun theWrappedFileMustBeExactlyItsSize() {
        val payload = ByteArray(CredentialKey.WRAPPED_SIZE) { it.toByte() }
        assertArrayEquals(payload, CredentialKey.readWrapped(ByteArrayInputStream(payload)))
        // Delivered in small pieces, as a stream may.
        val trickle = object : java.io.InputStream() {
            var at = 0
            override fun read() = if (at < payload.size) payload[at++].toInt() and 0xff else -1
            override fun read(b: ByteArray, off: Int, len: Int): Int {
                if (at >= payload.size) return -1
                b[off] = payload[at++]
                return 1
            }
        }
        assertArrayEquals(payload, CredentialKey.readWrapped(trickle))
        for (size in listOf(0, 1, CredentialKey.WRAPPED_SIZE - 1, CredentialKey.WRAPPED_SIZE + 1, 4096)) {
            assertThrows(IllegalArgumentException::class.java) {
                CredentialKey.readWrapped(ByteArrayInputStream(ByteArray(size)))
            }
        }
    }

    @Test fun aReceiverCredentialCannotBeMovedToAnotherReceiver() {
        val wrapping = aes()
        val credential = ByteArray(256) { it.toByte() }
        val receiver = "generated-receiver-1".toByteArray()
        val sealed = CredentialKey.wrap(wrapping, credential, receiver)
        assertArrayEquals(credential, CredentialKey.unwrap(wrapping, sealed, receiver))
        assertThrows(AEADBadTagException::class.java) {
            CredentialKey.unwrap(wrapping, sealed, "generated-receiver-2".toByteArray())
        }
        assertThrows(AEADBadTagException::class.java) {
            CredentialKey.unwrap(wrapping, sealed)
        }
    }

    @Test fun aPairingFileHasABoundedAuthenticatedPayload() {
        val wrapping = aes()
        val receiver = "generated-receiver".toByteArray()
        for (size in listOf(1, CastCredentials.MAX_CREDENTIAL_SIZE)) {
            val sealed = CredentialKey.wrap(wrapping, ByteArray(size), receiver)
            assertArrayEquals(sealed, CastCredentials.readSealed(ByteArrayInputStream(sealed)))
        }
        for (size in listOf(0, 28, CastCredentials.MAX_SEALED_SIZE + 1, 8192)) {
            assertNull(CastCredentials.readSealed(ByteArrayInputStream(ByteArray(size))))
        }
    }
}
