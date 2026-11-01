package io.github.retype.ime

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import androidx.test.platform.app.InstrumentationRegistry
import kotlinx.coroutines.*
import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Test

/** Opt-in real TLS acceptance against two isolated desktop fixtures, never personal pairings. */
class LanMultiDeviceTest {
  @Test fun multipleDevicesRemainIndependentAndShareTextAndOriginalImages() = runBlocking {
    val args = InstrumentationRegistry.getArguments()
    val aPort = args.getString("multiA")
    val bPort = args.getString("multiB")
    assumeTrue("Two explicit synthetic endpoints required", aPort != null && bPort != null)
    val a = args.getString("multiAId")!!
    val b = args.getString("multiBId")!!
    val instrumentation = InstrumentationRegistry.getInstrumentation()
    val context = instrumentation.targetContext
    val lan = LanClipboard.get(context)
    suspend fun waitFor(check: () -> Boolean) { withTimeout(30_000) { while (!check()) delay(50) } }
    fun copy(text: String) = instrumentation.runOnMainSync {
      context.getSystemService(ClipboardManager::class.java).setPrimaryClip(ClipData.newPlainText("Multi-device test", text))
    }
    fun online(id: String) = lan.state.value.peers.any { it.id == id && it.connection.isNotEmpty() }
    val store = LanPeerStore(context.getSharedPreferences("lanClipboard", Context.MODE_PRIVATE), SecretVault(context))
    try {
      lan.unlinkAllForTest()
      lan.pairForTest("10.0.2.2:$aPort", "123456")
      waitFor { online(a) }
      val originalA = store.get(a)!! to store.token(a)
      lan.showCode()
      assertTrue("Generating/cancelling another code must retain A's connection", online(a))
      lan.cancelPair()
      assertTrue(online(a))
      lan.pairForTest("10.0.2.2:$bPort", "123456")
      waitFor { online(a) && online(b) }
      assertEquals(2, lan.state.value.peers.size)
      assertEquals(originalA, store.get(a)!! to store.token(a))
      assertNotEquals(store.token(a), store.token(b))
      val bToken = store.token(b)
      // A wrong route pointing at B must fail A's certificate pin, then fall back to A.
      lan.setAddresses(a, "10.0.2.2:$bPort, 10.0.2.2:$aPort")
      waitFor { online(a) && online(b) }
      assertEquals(originalA.first.pin, store.get(a)!!.pin)
      assertEquals(originalA.second, store.token(a))
      assertEquals(bToken, store.token(b))
      for (peer in lan.state.value.peers) {
        assertTrue(peer.localAddress.substringAfterLast(':').toInt() > 0)
        assertTrue(peer.connection.substringAfterLast(':').toInt() > 0)
      }
      waitFor { lan.state.value.localAddresses.isNotEmpty() }
      assertTrue(lan.state.value.localAddresses.all { !it.contains(':') })
      waitFor { lan.pasteText() == "multi-from-a" }
      assertTrue("Receiving A's clip must not break B's authenticated connection", online(b))
      copy("multi-from-phone")
      waitFor { lan.pasteImage() != null }
      assertEquals("image/jpeg", context.contentResolver.getType(lan.pasteImage()!!))
      copy("multi-image-seen")
      waitFor { !online(a) && online(b) }
      copy("multi-while-a-offline")
      waitFor { online(a) && online(b) }
      instrumentation.runOnMainSync { lan.enable(false) }
      assertTrue(lan.state.value.peers.all { it.connection.isEmpty() })
      assertEquals(2, store.all().size)
      instrumentation.runOnMainSync { lan.enable(true) }
      waitFor { online(a) && online(b) }
      copy("multi-resumed")
      waitFor { lan.pasteText() == "multi-unlink-ready" }
      instrumentation.runOnMainSync { lan.unlink(a) }
      assertFalse(lan.state.value.peers.any { it.id == a })
      assertTrue(online(b))
      assertEquals("", store.token(a))
      assertTrue(store.token(b).isNotEmpty())
      copy("multi-b-only")
      delay(500)
    } finally { lan.unlinkAllForTest() }
  }
}
