package com.hantekllc.iptvrs.host

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeNoException
import org.junit.Test
import java.io.File
import java.nio.file.FileSystemException
import java.nio.file.Files

class UpdateCacheTest {
    @Test fun directUpdateRequiresAStagedApkInsideItsOwnCache() {
        val cache = Files.createTempDirectory("synthetic-cache").toFile()
        try {
            assertTrue(UpdateHost.owns(cache, File(cache, "patina-update-test.apk")))
            assertFalse(UpdateHost.owns(cache, File(cache, "another.apk")))
            assertFalse(UpdateHost.owns(cache, File(cache.parentFile, "patina-update-test.apk")))
        } finally {
            cache.delete()
        }
    }

    @Test fun directUpdateAcceptsAnAliasedCacheAndRejectsLinkedFiles() {
        val root = Files.createTempDirectory("synthetic-update")
        try {
            val cache = Files.createDirectory(root.resolve("cache"))
            val alias =
                try {
                    Files.createSymbolicLink(root.resolve("alias"), cache).toFile()
                } catch (error: FileSystemException) {
                    if (!System.getProperty("os.name").startsWith("Windows")) throw error
                    assumeNoException("Creating a symlink requires Windows developer mode or privilege", error)
                    return
                }
            val apk = File(alias, "patina-update-test.apk").apply { createNewFile() }
            assertTrue(UpdateHost.owns(alias, apk))
            val outside = Files.createFile(root.resolve("outside.apk"))
            val link = Files.createSymbolicLink(alias.toPath().resolve("patina-update-link.apk"), outside)
            assertFalse(UpdateHost.owns(alias, link.toFile()))
            assertFalse(UpdateHost.owns(alias, outside.toFile()))
        } finally {
            root.toFile().deleteRecursively()
        }
    }
}
