package com.hantekllc.iptvrs.host

import android.content.res.Configuration
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class DeviceSignalsTest {
    @Test
    fun aTvIsATvWhateverItsDisplay() {
        val television = Configuration.UI_MODE_TYPE_TELEVISION or Configuration.UI_MODE_NIGHT_YES
        assertTrue(DeviceSignals.tv(television, leanback = false))
        assertTrue(DeviceSignals.tv(Configuration.UI_MODE_TYPE_NORMAL, leanback = true))
        assertFalse(DeviceSignals.tv(Configuration.UI_MODE_TYPE_NORMAL, leanback = false))
        assertEquals("tv", DeviceSignals.form(tv = true, displaySmallestDp = 360))
    }

    @Test
    fun phonesAndTabletsAreClassedByTheDisplayNotTheWindow() {
        // The display's smallest width is passed, never the window's: a
        // tablet's 400 dp split-screen window still reports its 800 dp display.
        assertEquals("phone", DeviceSignals.form(tv = false, displaySmallestDp = 360))
        assertEquals("phone", DeviceSignals.form(tv = false, displaySmallestDp = 599))
        assertEquals("tablet", DeviceSignals.form(tv = false, displaySmallestDp = 600))
        assertEquals("tablet", DeviceSignals.form(tv = false, displaySmallestDp = 800))
        assertEquals("tablet", DeviceSignals.form(tv = false, displaySmallestDp = 0))
    }

    @Test
    fun theDecoderProbeReadsTheHardwareH264DecoderThePlayerPicks() {
        fun codec(
            encoder: Boolean = false,
            hardware: Boolean = true,
            avc: Boolean = true,
            max: Int,
            secureOnly: Boolean = false,
            alias: Boolean = false,
        ) = DeviceSignals.Decoder(encoder, hardware, avc, max, secureOnly, alias)
        assertEquals(null, DeviceSignals.decoders(emptyList()))
        // Software decoders and encoders do not bound the tiles.
        assertEquals(null, DeviceSignals.decoders(listOf(codec(hardware = false, max = 32), codec(encoder = true, max = 1))))
        assertEquals(1, DeviceSignals.decoders(listOf(codec(hardware = false, max = 32), codec(max = 1))))
        // The first one listed is picked, not the one with the most.
        assertEquals(2, DeviceSignals.decoders(listOf(codec(avc = false, max = 64), codec(max = 2), codec(max = 16))))
        // A secure-only decoder or an alias is never picked for a clear stream.
        assertEquals(8, DeviceSignals.decoders(listOf(codec(max = 1, secureOnly = true), codec(max = 1, alias = true), codec(max = 8))))
        // A decoder that reports no limit leaves the budget as it is.
        assertEquals(null, DeviceSignals.decoders(listOf(codec(max = 0), codec(max = 1))))
    }

    @Test
    fun aKeyboardOrDPadCountsOnlyWhileAttachedAndShown() {
        assertTrue(DeviceSignals.keyboard(Configuration.KEYBOARD_QWERTY, Configuration.HARDKEYBOARDHIDDEN_NO))
        assertFalse(DeviceSignals.keyboard(Configuration.KEYBOARD_QWERTY, Configuration.HARDKEYBOARDHIDDEN_YES))
        assertFalse(DeviceSignals.keyboard(Configuration.KEYBOARD_NOKEYS, Configuration.HARDKEYBOARDHIDDEN_NO))
        assertFalse(DeviceSignals.keyboard(Configuration.KEYBOARD_12KEY, Configuration.HARDKEYBOARDHIDDEN_NO))
        assertTrue(DeviceSignals.dpad(Configuration.NAVIGATION_DPAD, Configuration.NAVIGATIONHIDDEN_NO))
        assertFalse(DeviceSignals.dpad(Configuration.NAVIGATION_DPAD, Configuration.NAVIGATIONHIDDEN_YES))
        assertFalse(DeviceSignals.dpad(Configuration.NAVIGATION_NONAV, Configuration.NAVIGATIONHIDDEN_NO))
    }

    @Test
    fun switchServicesAreToldFromOtherAccessibilityServices() {
        assertTrue(
            AccessibilitySignals.switchAccess(
                listOf("com.google.android.marvin.talkback/com.google.android.accessibility.switchaccess.SwitchAccessService"),
            ),
        )
        assertTrue(
            AccessibilitySignals.switchAccess(
                listOf("com.samsung.accessibility/.universalswitch.UniversalSwitchService"),
            ),
        )
        assertTrue(
            AccessibilitySignals.switchAccess(
                listOf("com.google.android.accessibility.switchaccess/com.android.switchaccess.SwitchAccessService"),
            ),
        )
        // A screen reader alone, a password manager, a lookalike, or nothing.
        assertFalse(
            AccessibilitySignals.switchAccess(
                listOf(
                    "com.google.android.marvin.talkback/com.google.android.marvin.talkback.TalkBackService",
                    "org.example.passwords/.AutofillAccessibilityService",
                    "org.example.switchaccessibility/.ReaderService",
                    "org.example.reader/.SwitchAccessServiceHelper",
                ),
            ),
        )
        assertFalse(AccessibilitySignals.switchAccess(emptyList()))
    }
}
