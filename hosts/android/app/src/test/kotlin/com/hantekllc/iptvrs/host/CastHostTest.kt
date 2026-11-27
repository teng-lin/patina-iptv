package com.hantekllc.iptvrs.host

import java.net.InetAddress
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class CastHostTest {
    private val address = InetAddress.getByName("192.0.2.7")
    private val other = InetAddress.getByName("198.51.100.4")

    @Test fun aReusedAddressOnAnotherLanCannotStartTheRelay() {
        val before = CastLanNetwork(1, true, listOf(address))
        val request = CastStartRequest(address, CastLanNetwork.topology(listOf(before))!!)
        val reconnected = before.copy(handle = 2)
        assertEquals(before, CastLanNetwork.select(listOf(before), request))
        assertNull(CastLanNetwork.select(listOf(reconnected), request))
        assertNull(CastLanNetwork.select(emptyList(), request))
    }

    @Test fun aMovedOrAmbiguousAddressRequiresNewDiscovery() {
        val wifi = CastLanNetwork(1, true, listOf(address, other))
        val wire = CastLanNetwork(2, false, listOf(other))
        val request = CastStartRequest(address, CastLanNetwork.topology(listOf(wifi, wire))!!)
        assertEquals(wifi.copy(addresses = listOf(other, address)), CastLanNetwork.select(
            listOf(wire, wifi.copy(addresses = listOf(other, address))), request,
        ))
        assertNull(CastLanNetwork.select(listOf(wifi.copy(addresses = listOf(other)), wire), request))
        val duplicate = wire.copy(addresses = listOf(address))
        val ambiguous = CastStartRequest(address, CastLanNetwork.topology(listOf(wifi, duplicate))!!)
        assertNull(CastLanNetwork.select(listOf(wifi, duplicate), ambiguous))
    }
}
