package io.github.retype.ime

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import androidx.test.platform.app.InstrumentationRegistry
import java.io.ByteArrayOutputStream
import kotlinx.coroutines.*
import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Test

/** Run on two isolated Android devices; each retains the shared default port. */
class LanPhoneToPhoneTest {
  @Test fun phonesPairAndExchangeTextAndOriginalImages() = runBlocking {
    val args = InstrumentationRegistry.getArguments()
    val role = args.getString("phoneRole")
    assumeTrue(role == "server" || role == "client")
    val instrumentation = InstrumentationRegistry.getInstrumentation()
    val context = instrumentation.targetContext
    val lan = LanClipboard.get(context)
    val clipboard = context.getSystemService(ClipboardManager::class.java)
    val oldPort = lan.state.value.port
    suspend fun waitFor(check: () -> Boolean) { withTimeout(40_000) { while (!check()) delay(50) } }
    fun copy(text: String) = instrumentation.runOnMainSync { clipboard.setPrimaryClip(ClipData.newPlainText("Isolated phone test", text)) }
    try {
      lan.unlinkAllForTest()
      lan.setPort(LanEndpoint.PORT.toString())
      context.startActivity(Intent(context, SettingsActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
      if (role == "server") {
        lan.join("123456")
        waitFor { lan.pasteText() == "phone-client-text" }
        assertTrue(lan.state.value.peers.single().localAddress.endsWith(":${LanEndpoint.PORT}"))
        copy("phone-server-text")
        waitFor { lan.pasteImage() != null }
        val uri = lan.pasteImage()!!
        val original = context.contentResolver.openInputStream(uri)!!.use { it.readBytes() }
        val bitmap = BitmapFactory.decodeByteArray(original, 0, original.size)
        assertEquals(2, bitmap.width)
        assertEquals(0xffff0000.toInt(), bitmap.getPixel(0, 0))
        assertEquals(0xff0000ff.toInt(), bitmap.getPixel(1, 0))
        bitmap.recycle()
        instrumentation.runOnMainSync { clipboard.setPrimaryClip(ClipData.newUri(context.contentResolver, "Phone image roundtrip", uri)) }
        waitFor { lan.pasteText() == "phone-roundtrip-ok" }
        delay(500)
      } else {
        val address = args.getString("phoneAddress") ?: error("Explicit isolated phone endpoint required")
        lan.pairForTest(address, "123456", showing = true)
        waitFor { lan.state.value.peers.any { it.connection.isNotEmpty() } }
        copy("phone-client-text")
        waitFor { lan.pasteText() == "phone-server-text" }
        val bitmap = Bitmap.createBitmap(2, 1, Bitmap.Config.ARGB_8888)
        bitmap.setPixels(intArrayOf(0xffff0000.toInt(), 0xff0000ff.toInt()), 0, 2, 0, 0, 2, 1)
        val output = ByteArrayOutputStream()
        bitmap.compress(Bitmap.CompressFormat.PNG, 100, output)
        bitmap.recycle()
        val bytes = output.toByteArray()
        val uri = ClipboardImageProvider.publish(context, bytes)
        instrumentation.runOnMainSync { clipboard.setPrimaryClip(ClipData.newUri(context.contentResolver, "Isolated phone image", uri)) }
        waitFor { lan.pasteImage() != null }
        assertArrayEquals(bytes, context.contentResolver.openInputStream(lan.pasteImage()!!)!!.use { it.readBytes() })
        copy("phone-roundtrip-ok")
        delay(500)
      }
    } finally { lan.unlinkAllForTest(); lan.setPort(oldPort.toString()) }
  }
}
