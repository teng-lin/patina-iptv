package com.hantekllc.iptvrs.host

import android.content.Context
import android.content.Intent
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import java.net.InetAddress
import java.util.concurrent.ConcurrentHashMap

/** Android owns OS leases. Rust owns the receiver session and its media relay. */
internal object CastHost {
    @Volatile private var application: Context? = null

    @Volatile var visible = false
    private val main = Handler(Looper.getMainLooper())
    private val requests = ConcurrentHashMap<Long, CastStartRequest>()

    const val READY = 0
    const val STOP = 1
    const val PLAY = 2
    const val PAUSE = 3

    @JvmStatic private external fun initialize()

    @JvmStatic external fun playbackEvent(
        id: Long,
        event: Int,
    )

    fun attach(context: Context) {
        application = context.applicationContext
        initialize()
    }

    @JvmStatic fun multicast(): WifiManager.MulticastLock? {
        val context = application ?: return null
        if (lanNetworks(context).none { it.wifi }) return null
        val wifi = checkNotNull(context.getSystemService(WifiManager::class.java))
        return wifi.createMulticastLock("patina:cast-discovery").apply {
            setReferenceCounted(false)
            acquire()
        }
    }

    /** A route selected from the picker must still be on the same LAN. */
    @Suppress("DEPRECATION")
    internal fun lanNetworks(context: Context): List<CastLanNetwork> {
        val connectivity = context.getSystemService(ConnectivityManager::class.java) ?: return emptyList()
        return connectivity.allNetworks.mapNotNull { network ->
            val caps = connectivity.getNetworkCapabilities(network) ?: return@mapNotNull null
            if (!caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_VPN)) return@mapNotNull null
            val wifi = caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI)
            if (!wifi && !caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET)) return@mapNotNull null
            val links = connectivity.getLinkProperties(network) ?: return@mapNotNull null
            CastLanNetwork(network.networkHandle, wifi, links.linkAddresses.map { it.address })
        }
    }

    @JvmStatic fun topology(): String? = application?.let { CastLanNetwork.topology(lanNetworks(it)) }

    @JvmStatic fun start(
        id: Long,
        address: String,
        topology: String,
    ): Boolean {
        val context = application ?: return false
        if (!visible) return false
        // Rust passes a numeric address from the owned connection's socket.
        return try {
            requests[id] = CastStartRequest(InetAddress.getByName(address), topology)
            val intent =
                Intent(context, CastPlaybackService::class.java)
                    .putExtra(CastPlaybackService.ID, id)
            if (Build.VERSION.SDK_INT >= 26) context.startForegroundService(intent) else context.startService(intent)
            true
        } catch (_: Exception) {
            requests.remove(id)
            false
        }
    }

    internal fun request(id: Long): CastStartRequest? = requests[id]

    @JvmStatic fun release(id: Long) {
        requests.remove(id)
        main.post { CastPlaybackService.instance?.release(id) }
    }

    @JvmStatic fun playback(
        id: Long,
        state: Int,
        controls: Int,
    ) {
        main.post { CastPlaybackService.instance?.playback(id, state, controls) }
    }

    @JvmStatic fun loadCredential(key: String): ByteArray = CastCredentials.load(checkNotNull(application), key)

    @JvmStatic fun saveCredential(
        key: String,
        credential: ByteArray,
    ) = CastCredentials.save(checkNotNull(application), key, credential)

    @JvmStatic fun forgetCredential(key: String) = CastCredentials.forget(checkNotNull(application), key)
}

internal data class CastStartRequest(
    val address: InetAddress,
    val topology: String,
)

internal data class CastLanNetwork(
    val handle: Long,
    val wifi: Boolean,
    val addresses: List<InetAddress>,
) {
    companion object {
        fun topology(networks: List<CastLanNetwork>): String? =
            networks
                .flatMap { network ->
                    network.addresses.map { "${network.handle}:${it.hostAddress}" }
                }.sorted()
                .joinToString(",")
                .ifEmpty { null }

        fun select(
            networks: List<CastLanNetwork>,
            request: CastStartRequest,
        ): CastLanNetwork? {
            if (topology(networks) != request.topology) return null
            return networks.singleOrNull { request.address in it.addresses }
        }
    }
}
