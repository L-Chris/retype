package io.github.retype.ime

import org.junit.Assert.*
import org.junit.Test

class LanEndpointTest {
    @Test fun exactOverlayRangeAndExistingLanAddressesAreAccepted() {
        for (address in listOf("100.64.0.1", "100.127.255.254", "192.168.31.128", "home", "home.tailnet.ts.net")) {
            assertEquals("$address:50206", LanEndpoint.normalize(address))
        }
        assertEquals("[fd7a:115c:a1e0::1]:50206", LanEndpoint.normalize("[fd7a:115c:a1e0::1]:50206"))
        assertEquals(listOf("home:50206", "100.100.1.2:50206"), LanEndpoint.list(" home, home:50206,100.100.1.2"))
    }
    @Test fun publicAddressesAndUrlComponentsCannotBypassValidation() {
        for (address in listOf("100.63.255.255", "100.128.0.1", "8.8.8.8", "home:0", "home:65536", "home/path", "user@home", "home?x=1", "https://home", "[2001:4860:4860::8888]")) {
            assertTrue(address, runCatching { LanEndpoint.normalize(address) }.isFailure)
        }
    }
}
