package com.hantekllc.iptvrs.host

import org.junit.Assert.*
import org.junit.Test
import java.io.File
import javax.xml.parsers.DocumentBuilderFactory

class ExternalPlayerProbeTest {
    private val own = "com.hantekllc.iptvrs.host"

    @Test fun anotherInstalledPlayerTakesTheHandoff() {
        assertTrue(ExternalPlayerHost.player("org.example.player", own))
        assertTrue(ExternalPlayerHost.player("org.example.frameworkpackagestubs.player", own))
    }

    @Test fun patinaAndTheTvFrameworkStubsAreNotPlayers() {
        assertFalse(ExternalPlayerHost.player(own, own))
        assertFalse(ExternalPlayerHost.player("com.android.tv.frameworkpackagestubs", own))
        assertFalse(ExternalPlayerHost.player("com.android.frameworkpackagestubs", own))
        assertTrue(ExternalPlayerHost.stub("com.android.tv.frameworkpackagestubs"))
    }

    @Test fun oneOrTheDefaultPlayerStartsDirectlyAndSeveralGoToTheChooser() {
        val one = "org.example.one"
        val two = "org.example.two"
        assertEquals(one, ExternalPlayerHost.target(listOf(one), null))
        // A default outside the players (a resolver, a stub) is not one.
        assertEquals(one, ExternalPlayerHost.target(listOf(one), "android.resolver"))
        assertEquals(two, ExternalPlayerHost.target(listOf(one, two), two))
        assertNull(ExternalPlayerHost.target(listOf(one, two), null))
        assertNull(ExternalPlayerHost.target(listOf(one, two), "android.resolver"))
        assertNull(ExternalPlayerHost.target(emptyList<String>(), null))
    }

    /** Without a matching `<queries>` entry, Android 11 and later hide every
     * other player from the probe, and the option would never appear. */
    @Test fun theManifestMakesThePlayersOfTheHandoffVisible() {
        val android = "http://schemas.android.com/apk/res/android"
        val manifest = DocumentBuilderFactory.newInstance().apply { isNamespaceAware = true }
            .newDocumentBuilder().parse(File("src/main/AndroidManifest.xml"))
        val queries = manifest.getElementsByTagName("queries").item(0) as org.w3c.dom.Element
        val intents = queries.getElementsByTagName("intent")
        val visible = (0 until intents.length).any { index ->
            val intent = intents.item(index) as org.w3c.dom.Element
            val action = intent.getElementsByTagName("action").item(0) as org.w3c.dom.Element?
            val data = intent.getElementsByTagName("data")
            action?.getAttributeNS(android, "name") == "android.intent.action.VIEW" &&
                (0 until data.length).any {
                    val element = data.item(it) as org.w3c.dom.Element
                    element.getAttributeNS(android, "scheme") == ExternalPlayerHost.SCHEME &&
                        element.getAttributeNS(android, "mimeType") == ExternalPlayerHost.MIME_TYPE
                }
        }
        assertTrue(visible)
    }
}
