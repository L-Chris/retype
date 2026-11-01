package io.github.retype.ime

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Intent
import androidx.test.platform.app.InstrumentationRegistry
import kotlinx.coroutines.*
import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Test

/** Opt-in acceptance with the production Rust desktop client through adb forwarding. */
class LanDesktopInboundTest {
  @Test fun desktopDialsPhoneAndResumesWithDesktopProofs() = runBlocking {
    assumeTrue(InstrumentationRegistry.getArguments().getString("desktopInbound") == "true")
    val instrumentation = InstrumentationRegistry.getInstrumentation()
    val context = instrumentation.targetContext
    val lan = LanClipboard.get(context)
    val oldPort = lan.state.value.port
    suspend fun waitFor(check: () -> Boolean) { withTimeout(45_000) { while (!check()) delay(50) } }
    fun copy(text: String) = instrumentation.runOnMainSync {
      context.getSystemService(ClipboardManager::class.java).setPrimaryClip(ClipData.newPlainText("Isolated desktop inbound test", text))
    }
    try {
      lan.unlinkAllForTest()
      lan.setPort(LanEndpoint.PORT.toString())
      context.startActivity(Intent(context, SettingsActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
      lan.join("123456")
      waitFor { lan.pasteText() == "desktop-to-android-listener" }
      assertTrue(lan.state.value.listening)
      assertTrue(lan.state.value.peers.single().localAddress.endsWith(":${LanEndpoint.PORT}"))
      copy("android-server-received")
      waitFor { lan.pasteImage() != null }
      assertEquals("image/png", context.contentResolver.getType(lan.pasteImage()!!))
      assertTrue(context.contentResolver.openInputStream(lan.pasteImage()!!)!!.use { it.readBytes() }.isNotEmpty())
      copy("android-server-image-received")
      waitFor { lan.pasteText() == "desktop-to-android-resumed" }
      copy("android-server-resumed")
      delay(500)
    } finally { lan.unlinkAllForTest(); lan.setPort(oldPort.toString()) }
  }
}
