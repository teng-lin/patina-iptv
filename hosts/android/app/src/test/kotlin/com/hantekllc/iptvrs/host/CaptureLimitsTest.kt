package com.hantekllc.iptvrs.host

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
    fun theSixHourLimitAppliesFromAndroid15() {
        assertFalse(CaptureLimits.backgroundHours(34))
        assertTrue(CaptureLimits.backgroundHours(35))
        assertTrue(CaptureLimits.backgroundHours(36))
    }
}
