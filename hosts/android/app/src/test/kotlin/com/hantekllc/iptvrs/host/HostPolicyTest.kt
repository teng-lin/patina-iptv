package com.hantekllc.iptvrs.host

import android.view.KeyEvent
import androidx.media3.common.C
import dev.nshtv.RemoteKeys
import org.junit.Assert.*
import org.junit.Test
import java.io.File
import javax.xml.parsers.DocumentBuilderFactory

class HostPolicyTest {
    @Test fun transientLoadsRetryWithABoundedDelayButRefusalsDoNot() {
        for (status in listOf(401, 403, 429, 458, 509)) {
            assertTrue(LiveLoadPolicy.refused(status))
            assertEquals(C.TIME_UNSET, LiveLoadPolicy.retryDelay(1, status, 0))
        }
        for (status in listOf(null, 404, 500, 503)) {
            assertFalse(LiveLoadPolicy.refused(status))
            assertEquals(1000L, LiveLoadPolicy.retryDelay(1, status, 0))
            assertEquals(2000L, LiveLoadPolicy.retryDelay(2, status, 1000))
            assertEquals(3000L, LiveLoadPolicy.retryDelay(100, status, 3000))
        }
        assertEquals(C.TIME_UNSET, LiveLoadPolicy.retryDelay(1, null, C.TIME_UNSET))
        assertEquals(6, LiveLoadPolicy().getMinimumLoadableRetryCount(C.DATA_TYPE_MEDIA))
    }

    @Test
    fun systemKeysKeepTheirDefaultHandling() {
        val systemKeys =
            listOf(
                KeyEvent.KEYCODE_BACK,
                KeyEvent.KEYCODE_HOME,
                KeyEvent.KEYCODE_VOLUME_UP,
                KeyEvent.KEYCODE_DPAD_CENTER,
                KeyEvent.KEYCODE_A,
                KeyEvent.KEYCODE_UNKNOWN,
            )
        val appKeys =
            listOf(
                KeyEvent.KEYCODE_SEARCH,
                KeyEvent.KEYCODE_GUIDE,
                KeyEvent.KEYCODE_MEDIA_PLAY_PAUSE,
            )
        systemKeys.forEach { assertFalse(RemoteKeys.appOwned(it)) }
        appKeys.forEach { assertTrue(RemoteKeys.appOwned(it)) }
    }

    @Test fun onlyTheProfileBuildIsProfileable() {
        val parser = DocumentBuilderFactory.newInstance().apply { isNamespaceAware = true }.newDocumentBuilder()
        val main = parser.parse(File("src/main/AndroidManifest.xml"))
        assertEquals(0, main.getElementsByTagName("profileable").length)
        val profile = parser.parse(File("src/profile/AndroidManifest.xml"))
        val profileable = profile.getElementsByTagName("profileable").item(0) as org.w3c.dom.Element
        assertEquals("true", profileable.getAttributeNS("http://schemas.android.com/apk/res/android", "shell"))
        // No other source set may bring it back into a release.
        assertFalse(File("src/release").exists())
    }

    @Test fun cloudAndTransferRulesExcludeEveryCredentialStorageDomain() {
        val manifest = DocumentBuilderFactory.newInstance().apply { isNamespaceAware = true }
            .newDocumentBuilder().parse(File("src/main/AndroidManifest.xml"))
        val app = manifest.getElementsByTagName("application").item(0) as org.w3c.dom.Element
        val android = "http://schemas.android.com/apk/res/android"
        assertEquals("false", app.getAttributeNS(android, "allowBackup"))
        assertEquals("@xml/backup_rules", app.getAttributeNS(android, "fullBackupContent"))
        assertEquals("@xml/data_extraction_rules", app.getAttributeNS(android, "dataExtractionRules"))
        val required = setOf("root", "file", "database", "sharedpref", "external", "device_root", "device_file", "device_database", "device_sharedpref")
        val parser = DocumentBuilderFactory.newInstance().newDocumentBuilder()
        for ((file, sections) in listOf("backup_rules.xml" to listOf("full-backup-content"),
            "data_extraction_rules.xml" to listOf("cloud-backup", "device-transfer"))) {
            val document = parser.parse(File("src/main/res/xml/$file"))
            for (section in sections) {
                val element = document.getElementsByTagName(section).item(0) as org.w3c.dom.Element
                val exclusions = element.getElementsByTagName("exclude")
                val domains = (0 until exclusions.length).map {
                    val rule = exclusions.item(it) as org.w3c.dom.Element
                    assertEquals(".", rule.getAttribute("path"))
                    rule.getAttribute("domain")
                }.toSet()
                assertEquals(required, domains)
            }
        }
    }

    @Test fun leanbackLauncherHasNoPipWhileHandheldKeepsIt() {
        val parser = DocumentBuilderFactory.newInstance().apply { isNamespaceAware = true }.newDocumentBuilder()
        val manifest = parser.parse(File("src/main/AndroidManifest.xml"))
        val android = "http://schemas.android.com/apk/res/android"
        val nodes = manifest.getElementsByTagName("activity")
        val activities = (0 until nodes.length).map { nodes.item(it) as org.w3c.dom.Element }

        fun launcher(category: String): org.w3c.dom.Element =
            activities.single { activity ->
                val filters = activity.getElementsByTagName("intent-filter")
                (0 until filters.length).any { index ->
                    val filter = filters.item(index) as org.w3c.dom.Element
                    val actions = filter.getElementsByTagName("action")
                    val categories = filter.getElementsByTagName("category")
                    val main =
                        (0 until actions.length).any {
                            (actions.item(it) as org.w3c.dom.Element).getAttributeNS(android, "name") == "android.intent.action.MAIN"
                        }
                    val matchingCategory =
                        (0 until categories.length).any {
                            (categories.item(it) as org.w3c.dom.Element).getAttributeNS(android, "name") == category
                        }
                    main && matchingCategory
                }
            }
        val handheld = launcher("android.intent.category.LAUNCHER")
        val tv = launcher("android.intent.category.LEANBACK_LAUNCHER")
        assertEquals(".TvActivity", handheld.getAttributeNS(android, "name"))
        assertEquals("true", handheld.getAttributeNS(android, "supportsPictureInPicture"))
        assertEquals(".LeanbackActivity", tv.getAttributeNS(android, "name"))
        assertFalse(tv.hasAttributeNS(android, "supportsPictureInPicture"))
        for (attribute in listOf("exported", "resizeableActivity", "launchMode", "configChanges", "windowSoftInputMode")) {
            assertEquals(handheld.getAttributeNS(android, attribute), tv.getAttributeNS(android, attribute))
        }
        assertEquals(TvActivity::class.java, launcherActivity(false))
        assertEquals(LeanbackActivity::class.java, launcherActivity(true))
    }

    @Test fun staleShortcutsRedirectToTheDeviceHostBeforeItMounts() {
        assertNull(redirectedActivity(TvActivity::class.java, false))
        assertNull(redirectedActivity(LeanbackActivity::class.java, true))
        assertEquals(LeanbackActivity::class.java, redirectedActivity(TvActivity::class.java, true))
        assertEquals(TvActivity::class.java, redirectedActivity(LeanbackActivity::class.java, false))
    }
}
