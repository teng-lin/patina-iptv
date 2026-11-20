package com.hantekllc.iptvrs.host

/**
 * When [AudioFocusHost] holds the audio focus, free of Android so it is
 * unit-tested on the JVM. Players are any keys (the host's components).
 *
 * - The focus is wanted while a player means to play aloud ([player]).
 * - Another app taking it for good ([lost]) blocks taking it back until a
 *   player that was playing aloud goes quiet and plays again (a new play
 *   intent): the app's pause arriving a moment after the loss must not
 *   steal it back from the other app.
 * - A call taking it for a while ([lostForAWhile]) keeps the request
 *   while a stream is open, so the system gives it back ([gained]); once
 *   every stream is stopped, closed or in the background, it goes.
 * - When it comes back, the request stays until the app's decision
 *   arrives: a player playing aloud again, or [decided] after a timeout.
 */
internal class AudioFocusState {
    /** What the host must do with the system. */
    enum class Do { NOTHING, REQUEST, ABANDON }

    private val aloud = HashSet<Any>()
    private val open = HashSet<Any>()

    /** Playing aloud when the focus was lost for good: not a new intent. */
    private val blocked = HashSet<Any>()

    /** The focus is requested (granted, or delayed by the system). */
    var held = false
        private set

    /** A call's pause waits for the focus to come back. */
    var waiting = false
        private set

    /** The focus came back: the app decides whether playback resumes. */
    var deciding = false
        private set

    /**
     * Every player stays silent: a call or a prompt holds the focus. A
     * tile the viewer chooses meanwhile plays no sound over it; the app's
     * pauses are left as they are, and the sound returns with the focus.
     */
    val silenced: Boolean get() = waiting

    /**
     * `player` plays aloud (a source, set to play, not muted, buffering or
     * ready, the activity started) and has an `open` stream (a source,
     * buffering or ready, the activity started).
     */
    fun player(
        player: Any,
        aloud: Boolean,
        open: Boolean,
    ): Do {
        if (aloud) {
            if (this.aloud.add(player)) deciding = false
        } else {
            this.aloud.remove(player)
            blocked.remove(player)
        }
        if (open) this.open.add(player) else this.open.remove(player)
        return settle()
    }

    fun removed(player: Any): Do = player(player, aloud = false, open = false)

    /** The system's answer to [Do.REQUEST]. */
    fun requested(
        granted: Boolean,
        delayed: Boolean,
    ) {
        held = granted || delayed
        if (delayed) waiting = true
    }

    /** Another app took the focus for good: the host gives it up. */
    fun lost(): Do {
        waiting = false
        deciding = false
        blocked.addAll(aloud)
        return abandon()
    }

    /** A call or a prompt took it for a while. */
    fun lostForAWhile() {
        waiting = true
        deciding = false
    }

    /** It came back. Whether the app hears of it (a call's pause waited). */
    fun gained(): Boolean {
        if (!waiting) return false
        waiting = false
        deciding = true
        return true
    }

    /** The app's decision had its time: hold the focus only if wanted. */
    fun decided(): Do {
        deciding = false
        return settle()
    }

    /** The activity is going. */
    fun destroyed(): Do {
        waiting = false
        deciding = false
        aloud.clear()
        open.clear()
        return abandon()
    }

    private fun abandon(): Do {
        if (!held) return Do.NOTHING
        held = false
        return Do.ABANDON
    }

    private fun settle(): Do {
        // Every stream stopped, closed, failed or in the background: no
        // call's pause or decision is waiting for the focus any more.
        if (open.isEmpty()) {
            waiting = false
            deciding = false
        }
        val wanted = aloud.any { it !in blocked }
        return when {
            wanted && !held -> Do.REQUEST
            !wanted && held && !waiting && !deciding -> abandon()
            else -> Do.NOTHING
        }
    }
}
