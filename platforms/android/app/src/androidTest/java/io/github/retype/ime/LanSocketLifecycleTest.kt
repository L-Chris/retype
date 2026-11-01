package io.github.retype.ime

import android.os.Looper
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.io.IOException
import java.net.Socket
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference
import kotlinx.coroutines.*
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class LanSocketLifecycleTest {
  @Test fun closeIsOffMainThreadAndCannotDetachOtherDevicesOrReplacementSockets() {
    val instrumentation = InstrumentationRegistry.getInstrumentation()
    val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    val link = LanConnection("first")
    val other = LanConnection("second")
    val closed = CountDownLatch(1)
    val release = CountDownLatch(1)
    val finished = CountDownLatch(1)
    val onMain = AtomicReference<Boolean>()
    val replacementClosed = AtomicReference(false)
    val original = object : Socket() {
      override fun close() {
        onMain.set(Looper.myLooper() == Looper.getMainLooper())
        closed.countDown()
        if (onMain.get() == true) throw android.os.NetworkOnMainThreadException()
        try { release.await(5, TimeUnit.SECONDS); throw IOException("Synthetic close failure") }
        finally { finished.countDown() }
      }
    }
    val replacement = object : Socket() {
      override fun close() { replacementClosed.set(true) }
    }
    other.socket.set(replacement)
    other.outgoing.set(JSONObject().put("seq", 2))
    try {
      link.socket.set(original)
      instrumentation.runOnMainSync { link.close(scope) }
      assertTrue(closed.await(5, TimeUnit.SECONDS))
      assertEquals(false, onMain.get())
      assertNull(link.socket.get())
      link.socket.set(replacement)
      release.countDown()
      assertTrue(finished.await(5, TimeUnit.SECONDS))
      assertSame(replacement, link.socket.get())
      assertSame(replacement, other.socket.get())
      assertEquals(2, other.outgoing.get()!!.getInt("seq"))
      assertTrue(other.active.get())
      assertFalse(replacementClosed.get())
    } finally { release.countDown(); scope.cancel() }
  }
}
