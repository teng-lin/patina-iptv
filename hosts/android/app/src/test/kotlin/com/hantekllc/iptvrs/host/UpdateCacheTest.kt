package com.hantekllc.iptvrs.host

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File
import java.nio.file.Files

class UpdateCacheTest {
    @Test fun directUpdateAcceptsAnAliasedCacheAndRejectsLinkedFiles() {
        val root = Files.createTempDirectory("synthetic-update")
        try {
            val cache = Files.createDirectory(root.resolve("cache"))
            val alias = Files.createSymbolicLink(root.resolve("alias"), cache).toFile()
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
