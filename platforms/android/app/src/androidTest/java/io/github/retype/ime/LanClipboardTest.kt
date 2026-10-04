package io.github.retype.ime

import android.content.ClipData
import android.content.ClipboardManager
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import kotlinx.coroutines.*
import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class LanClipboardTest {
  @Test
  fun encryptedPairingAndBidirectionalClipboard() = runBlocking {
    val port = InstrumentationRegistry.getArguments().getString("lanPort")
    assumeTrue("Opt-in synthetic desktop fixture required", port != null)
    val instrumentation = InstrumentationRegistry.getInstrumentation()
    val context = instrumentation.targetContext
    val lan = LanClipboard.get(context)
    try {
      lan.unlink()
      lan.pairForTest("10.0.2.2:$port", "123456", InstrumentationRegistry.getArguments().getString("phoneShow") == "true")
      withTimeout(15_000) { while (lan.state.value.text != "电脑文字\nHello 👋") delay(50) }
      assertEquals("电脑文字\nHello 👋", lan.pasteText())
      instrumentation.runOnMainSync {
        val clipboard = context.getSystemService(ClipboardManager::class.java)
        assertEquals("电脑文字\nHello 👋", clipboard.primaryClip?.getItemAt(0)?.text?.toString())
        clipboard.setPrimaryClip(ClipData.newPlainText("LAN test", "手机文字\nworld 👋"))
      }
      withTimeout(5000) { while (lan.state.value.text != null) delay(50) }
      delay(500)
      lan.enable(false)
      assertNull(lan.pasteText())
    } finally {
      lan.unlink()
    }
  }
}
