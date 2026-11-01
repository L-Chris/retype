package io.github.retype.ime

import android.content.Context
import android.content.Intent
import androidx.test.platform.app.InstrumentationRegistry
import java.io.File
import kotlinx.coroutines.*
import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Test

/** Opt-in companion to the desktop mDNS browser; never touches the system clipboard or IME selection. */
class LanDiscoveryHostTest {
  @Test fun advertisePairingModesToIndependentDesktopBrowser() = runBlocking {
    assumeTrue(InstrumentationRegistry.getArguments().getString("discoveryHost") == "true")
    val context = InstrumentationRegistry.getInstrumentation().targetContext
    val lan = LanClipboard.get(context)
    val oldPort = lan.state.value.port
    try {
      lan.unlinkAllForTest()
      lan.setPort(LanEndpoint.PORT.toString())
      context.startActivity(Intent(context, SettingsActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
      File(context.cacheDir, "lan-discovery-id.txt").writeText(context.getSharedPreferences("lanClipboard", Context.MODE_PRIVATE).getString("id", "")!!)
      lan.showCode()
      withTimeout(15_000) { while (!lan.state.value.listening) delay(50) }
      delay(20_000)
      lan.join("123456")
      delay(20_000)
      lan.cancelPair()
      delay(20_000)
      lan.enable(false)
      assertFalse(lan.state.value.listening)
      delay(5000)
    } finally { lan.unlinkAllForTest(); lan.setPort(oldPort.toString()) }
  }
}
