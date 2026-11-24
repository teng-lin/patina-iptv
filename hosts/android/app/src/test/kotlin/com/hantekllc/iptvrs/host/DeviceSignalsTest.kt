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
    fun theDecoderProbeCountsHardwareH264DecodersOnly() {
        fun codec(
            encoder: Boolean = false,
            hardware: Boolean = true,
            avc: Boolean = true,
            max: Int,
        ) = DeviceSignals.Decoder(encoder, hardware, avc, max)
        assertEquals(null, DeviceSignals.decoders(emptyList()))
        // Software decoders and encoders do not bound the tiles.
        assertEquals(null, DeviceSignals.decoders(listOf(codec(hardware = false, max = 32), codec(encoder = true, max = 1))))
        assertEquals(1, DeviceSignals.decoders(listOf(codec(max = 1), codec(hardware = false, max = 32))))
        assertEquals(16, DeviceSignals.decoders(listOf(codec(max = 2), codec(max = 16), codec(avc = false, max = 64))))
        assertEquals(null, DeviceSignals.decoders(listOf(codec(max = 0))))
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
}
