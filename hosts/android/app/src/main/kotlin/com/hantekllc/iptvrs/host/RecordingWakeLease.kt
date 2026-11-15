package com.hantekllc.iptvrs.host

/** Main-thread lifecycle for a bounded CPU lease, independent of Android's
 * foreground-service time budget. Cancellation never leaves a permanent lock. */
internal class RecordingWakeLease(
    private val acquire: (Long) -> Unit,
    private val schedule: (Runnable, Long) -> Unit,
    private val cancel: (Runnable) -> Unit,
    private val release: () -> Unit,
) {
    private var active = false
    private val renewal =
        object : Runnable {
            override fun run() {
                if (!active) return
                acquire(10 * 60 * 1000L)
                schedule(this, 5 * 60 * 1000L)
            }
        }

    fun start() {
        if (active) return
        active = true
        renewal.run()
    }

    fun stop() {
        if (!active) return
        active = false
        cancel(renewal)
        release()
    }
}
