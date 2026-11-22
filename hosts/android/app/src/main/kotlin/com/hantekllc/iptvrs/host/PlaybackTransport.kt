package com.hantekllc.iptvrs.host

import org.json.JSONObject
import java.io.IOException

/**
 * Per-player bounded Rust transport; Media3 never receives identity headers.
 * `relay` stands in for the Rust side in JVM tests; null: the native one.
 */
internal class PlaybackTransport(relay: Relay? = null) {
    /** The Rust relay's lifetime calls (`crates/iptv-app/src/host/native/android_transport.rs`). */
    internal interface Relay {
        /** A new relay for `url`; its local address. Throws [IOException]. */
        fun create(url: String, headers: Map<String, String>, publicOnly: Boolean): String
        fun updateHeaders(local: String, headers: Map<String, String>)
        fun release(local: String)
        fun takeErrorCode(local: String): String?
    }

    private val relay: Relay = relay ?: object : Relay {
        override fun create(url: String, headers: Map<String, String>, publicOnly: Boolean) =
            this@PlaybackTransport.create(url, JSONObject(headers).toString(), publicOnly)
        override fun updateHeaders(local: String, headers: Map<String, String>) =
            this@PlaybackTransport.updateHeaders(local, JSONObject(headers).toString())
        override fun release(local: String) = this@PlaybackTransport.release(local)
        override fun takeErrorCode(local: String) = this@PlaybackTransport.takeErrorCode(local)
    }

    private var target: String? = null
    private var local: String? = null

    /**
     * The open relay's capability prefix ([RelayGuard]), or null when none
     * is open: every load the player makes must start with it. Read on
     * Media3's loader threads.
     */
    @Volatile var prefix: String? = null
        private set

    fun open(url: String, headers: Map<String, String>, publicOnly: Boolean): String {
        if (target != url) close()
        val active = local
        if (active != null) {
            update(headers)
            return active
        }
        val created = relay.create(url, headers, publicOnly)
        // Not a relay address: nothing could load through it.
        val capability = RelayGuard.prefixOf(created) ?: run {
            relay.release(created)
            throw IOException("Playback transport unavailable")
        }
        target = url
        local = created
        prefix = capability
        return created
    }

    fun update(headers: Map<String, String>) {
        local?.let { relay.updateHeaders(it, headers) }
    }

    fun takeError(): String? = local?.let { relay.takeErrorCode(it) }

    fun close() {
        prefix = null
        local?.let { relay.release(it) }
        local = null
        target = null
    }

    private external fun create(url: String, headers: String, publicOnly: Boolean): String
    private external fun updateHeaders(local: String, headers: String)
    private external fun release(local: String)
    private external fun takeErrorCode(local: String): String?
}
