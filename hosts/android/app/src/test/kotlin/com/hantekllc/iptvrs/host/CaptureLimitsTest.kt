package com.hantekllc.iptvrs.host

import android.content.pm.ServiceInfo
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class CaptureLimitsTest {
    @Test
    fun lowPowerStandbyStopsACaptureOnlyWhenOnAndNotExempt() {
        // Before Android 13 there is no Low Power Standby.
        assertFalse(CaptureLimits.standbyStops(sdk = 32, enabled = true, exempt = false))
        // Android 13 cannot say whether Patina is exempt: on counts.
        assertTrue(CaptureLimits.standbyStops(sdk = 33, enabled = true, exempt = true))
        assertFalse(CaptureLimits.standbyStops(sdk = 33, enabled = false, exempt = false))
        // Android 14 and later: an exemption keeps the capture running.
        assertTrue(CaptureLimits.standbyStops(sdk = 34, enabled = true, exempt = false))
        assertFalse(CaptureLimits.standbyStops(sdk = 34, enabled = true, exempt = true))
        assertFalse(CaptureLimits.standbyStops(sdk = 36, enabled = false, exempt = false))
    }

    @Test
    fun androidTvUsesMediaPlaybackForRecording() {
        assertEquals(ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK, CaptureLimits.foregroundServiceType(tv = true))
        assertEquals(ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC, CaptureLimits.foregroundServiceType(tv = false))
    }

    @Test
    fun theSixHourLimitAppliesToHandheldsFromAndroid15() {
        assertFalse(CaptureLimits.backgroundHours(sdk = 34, tv = false))
        assertTrue(CaptureLimits.backgroundHours(sdk = 35, tv = false))
        assertTrue(CaptureLimits.backgroundHours(sdk = 36, tv = false))
        assertFalse(CaptureLimits.backgroundHours(sdk = 35, tv = true))
        assertFalse(CaptureLimits.backgroundHours(sdk = 36, tv = true))
    }
}
