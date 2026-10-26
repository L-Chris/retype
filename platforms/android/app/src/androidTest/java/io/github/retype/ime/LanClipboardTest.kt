package io.github.retype.ime

import android.content.ClipData
import android.content.ClipboardManager
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import java.io.ByteArrayOutputStream
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
      val prefs=context.getSharedPreferences("lanClipboard",android.content.Context.MODE_PRIVATE)
      val before=listOf(prefs.getString("peer", ""),prefs.getString("pin", ""),SecretVault(context).get("lan-token"))
      // Add a failing candidate, then reconnect through a different saved route.
      // The peer identity and credentials must remain unchanged, without a new code.
      lan.setAddresses("10.0.2.2:1, 10.0.2.2:$port")
      withTimeout(15_000) {while(lan.state.value.connection!="10.0.2.2:$port" || lan.state.value.message!="已连接") delay(50)}
      assertEquals(before,listOf(prefs.getString("peer", ""),prefs.getString("pin", ""),SecretVault(context).get("lan-token")))
      assertNull(lan.state.value.code)
      assertEquals("10.0.2.2:$port", lan.state.value.target)
      instrumentation.runOnMainSync {
        val clipboard = context.getSystemService(ClipboardManager::class.java)
        assertEquals("电脑文字\nHello 👋", clipboard.primaryClip?.getItemAt(0)?.text?.toString())
        clipboard.setPrimaryClip(ClipData.newPlainText("LAN test", "手机文字\nworld 👋"))
      }
      withTimeout(5000) { while (lan.state.value.text != null) delay(50) }
      withTimeout(15_000) { while (lan.state.value.image == null) delay(50) }
      assertNull(lan.pasteText())
      val image = lan.pasteImage()
      assertNotNull(image)
      context.contentResolver.openInputStream(image!!).use { stream ->
        val bitmap = BitmapFactory.decodeStream(stream)
        assertNotNull(bitmap)
        assertTrue(bitmap.width > 0 && bitmap.height > 0)
        bitmap.recycle()
      }
      val bitmap = Bitmap.createBitmap(2, 1, Bitmap.Config.ARGB_8888)
      bitmap.setPixels(intArrayOf(0xffff0000.toInt(), 0xff0000ff.toInt()), 0, 2, 0, 0, 2, 1)
      val out = ByteArrayOutputStream()
      bitmap.compress(Bitmap.CompressFormat.PNG, 100, out)
      bitmap.recycle()
      val outgoing = ClipboardImageProvider.publish(context, out.toByteArray())
      instrumentation.runOnMainSync {
        val clipboard = context.getSystemService(ClipboardManager::class.java)
        assertEquals(image, clipboard.primaryClip?.getItemAt(0)?.uri)
        assertTrue(clipboard.primaryClip!!.description.hasMimeType("image/png"))
        clipboard.setPrimaryClip(ClipData.newUri(context.contentResolver, "LAN image test", outgoing))
      }
      withTimeout(5000) { while (lan.state.value.image != null) delay(50) }
      for (mime in listOf("image/jpeg", "image/webp")) {
        withTimeout(15_000) { while (lan.state.value.image == null) delay(50) }
        val original = lan.pasteImage()!!
        assertEquals(mime, context.contentResolver.getType(original))
        val before = context.contentResolver.openInputStream(original)!!.use { it.readBytes() }
        assertTrue(before.isNotEmpty())
        instrumentation.runOnMainSync {
          val clipboard = context.getSystemService(ClipboardManager::class.java)
          assertTrue(clipboard.primaryClip!!.description.hasMimeType(mime))
          clipboard.setPrimaryClip(ClipData.newUri(context.contentResolver, "LAN original test", original))
        }
        withTimeout(5000) { while (lan.state.value.image != null) delay(50) }
      }
      delay(1000)
      instrumentation.runOnMainSync { lan.enable(false) }
      assertNull(lan.pasteText())
    } finally {
      lan.unlink()
    }
  }
}
