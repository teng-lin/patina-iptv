package com.hantekllc.iptvrs.host

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class RecordingWakeLeaseTest {
    @Test
    fun longRecordingsKeepABoundedLeaseAndDestructionCancelsRenewal() {
        var now = 0L
        var expires = 0L
        var acquisitions = 0
        var releases = 0
        var pending: Pair<Runnable, Long>? = null
        val lease =
            RecordingWakeLease(
                acquire = { timeout ->
                    assertTrue(timeout in 1..30 * 60 * 1000L)
                    expires = now + timeout
                    acquisitions++
                },
                schedule = { job, delay -> pending = job to now + delay },
                cancel = { job -> if (pending?.first === job) pending = null },
                release = {
                    expires = 0
                    releases++
                },
            )
        lease.start()
        lease.start()
        assertEquals(1, acquisitions)
        // Simulated time covers the longest accepted capture, beyond the old six-hour expiry.
        while (now < 24 * 60 * 60 * 1000L) {
            val (job, at) = checkNotNull(pending)
            assertTrue(at in (now + 1) until expires)
            pending = null
            now = at
            job.run()
            assertTrue(expires > now)
        }
        assertTrue(acquisitions > 1)
        val stale = pending!!.first
        lease.stop()
        lease.stop()
        assertNull(pending)
        assertEquals(0L, expires)
        assertEquals(1, releases)
        val stoppedAt = acquisitions
        stale.run()
        assertEquals(stoppedAt, acquisitions)
        assertNull(pending)
    }
}
