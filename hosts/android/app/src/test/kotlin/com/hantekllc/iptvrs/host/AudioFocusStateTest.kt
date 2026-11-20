package com.hantekllc.iptvrs.host

import com.hantekllc.iptvrs.host.AudioFocusState.Do
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** When the activity's one audio focus is held ([AudioFocusState]). */
class AudioFocusStateTest {
    private val main = "main"
    private val tile = "tile"

    /** A playing player, its focus granted. */
    private fun playing(): AudioFocusState =
        AudioFocusState().also {
            assertEquals(Do.REQUEST, it.player(main, aloud = true, open = true))
            it.requested(granted = true, delayed = false)
        }

    @Test fun aPermanentLossIsNeverTakenBackBeforeANewPlay() {
        val s = playing()
        assertEquals(Do.ABANDON, s.lost())
        // The app's pause has not arrived: the player still reports aloud.
        assertEquals(Do.NOTHING, s.player(main, aloud = true, open = true))
        assertFalse(s.held)
        // The pause arrives, then the viewer plays again: a new intent.
        assertEquals(Do.NOTHING, s.player(main, aloud = false, open = true))
        assertEquals(Do.REQUEST, s.player(main, aloud = true, open = true))
    }

    @Test fun aCallsGainKeepsTheFocusUntilTheAppDecides() {
        val s = playing()
        s.lostForAWhile()
        // The app's pause: the request stays for the gain.
        assertEquals(Do.NOTHING, s.player(main, aloud = false, open = true))
        assertTrue(s.held)
        assertTrue(s.gained())
        // Nothing is given up before the app's resume arrives.
        assertEquals(Do.NOTHING, s.player(main, aloud = false, open = true))
        assertTrue(s.held)
        assertEquals(Do.NOTHING, s.player(main, aloud = true, open = true))
        assertFalse(s.deciding)
        assertTrue(s.held)
        // A second gain is no news.
        assertFalse(s.gained())
    }

    @Test fun anAppThatDoesNotResumeGivesTheFocusUpAfterItsTime() {
        val s = playing()
        s.lostForAWhile()
        s.player(main, aloud = false, open = true)
        assertTrue(s.gained())
        assertEquals(Do.ABANDON, s.decided())
        assertFalse(s.held)
    }

    @Test fun stoppingOrLeavingDuringACallGivesTheFocusUp() {
        for (close in listOf<(AudioFocusState) -> Do>(
            { it.player(main, aloud = false, open = false) },
            { it.removed(main) },
        )) {
            val s = playing()
            s.lostForAWhile()
            assertEquals(Do.NOTHING, s.player(main, aloud = false, open = true))
            assertEquals(Do.ABANDON, close(s))
            assertFalse(s.waiting)
            assertFalse(s.held)
        }
    }

    @Test fun anEndedOrFailedStreamHoldsNoFocus() {
        val s = playing()
        // Ended: the player still has its source and is set to play.
        assertEquals(Do.ABANDON, s.player(main, aloud = false, open = false))
    }

    @Test fun aDelayedGrantIsACallsPause() {
        val s = AudioFocusState()
        assertEquals(Do.REQUEST, s.player(main, aloud = true, open = true))
        s.requested(granted = false, delayed = true)
        assertTrue(s.held)
        assertTrue(s.waiting)
        assertTrue(s.gained())
    }

    @Test fun multiviewTilesShareOneFocus() {
        val s = playing()
        assertEquals(Do.NOTHING, s.player(tile, aloud = true, open = true))
        assertEquals(Do.NOTHING, s.player(main, aloud = false, open = true))
        assertTrue(s.held)
        assertEquals(Do.ABANDON, s.player(tile, aloud = false, open = true))
        assertEquals(Do.NOTHING, s.destroyed())
    }
}
