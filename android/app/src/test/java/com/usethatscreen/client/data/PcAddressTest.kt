package com.usethatscreen.client.data

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class PcAddressTest {
    @Test fun acceptsLanAndTailscaleAddresses() {
        for (address in listOf("10.0.0.1", "172.31.0.1", "192.168.1.10", "100.64.0.0", "100.127.255.255")) {
            assertEquals(address, parsePcAddress(" $address ")?.hostAddress)
        }
    }

    @Test fun rejectsOutOfRangeAndMalformedAddresses() {
        for (address in listOf("100.63.255.255", "100.128.0.0", "8.8.8.8", "127.0.0.1", "100.64.0.256", "100.64..1", "100.64.0.1:47820")) {
            assertNull(address, parsePcAddress(address))
        }
    }
}
