package io.github.retype.ime

import android.os.Looper
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.io.IOException
import java.net.Socket
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class LanSocketLifecycleTest {
  @Test fun disableClosesOffMainThread() = checkClose { it.enable(false) }
  @Test fun cancelClosesOffMainThread() = checkClose { it.cancelPair(false) }
  @Test fun unlinkClosesOffMainThread() = checkClose { it.unlink() }

  @Test fun reconnectClosesOffMainThread() = checkClose { lan ->
    val context = InstrumentationRegistry.getInstrumentation().targetContext
    context.getSharedPreferences("lanClipboard", android.content.Context.MODE_PRIVATE).edit()
      .putString("peer", "synthetic-peer").putBoolean("enabled", true).commit()
    lan.setAddresses("127.0.0.1:1")
  }

  private fun checkClose(action: (LanClipboard) -> Unit) {
    val instrumentation = InstrumentationRegistry.getInstrumentation()
    val lan = LanClipboard.get(instrumentation.targetContext)
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
        try {
          release.await(5, TimeUnit.SECONDS)
          throw IOException("Synthetic close failure")
        } finally {
          finished.countDown()
        }
      }
    }
    val replacement = object : Socket() {
      override fun close() { replacementClosed.set(true) }
    }
    val field = LanClipboard::class.java.getDeclaredField("socket").apply { isAccessible = true }
    @Suppress("UNCHECKED_CAST")
    val active = field.get(lan) as AtomicReference<Socket?>
    try {
      instrumentation.runOnMainSync {
        lan.unlink()
        active.set(original)
        action(lan)
      }
      assertTrue("Close must be scheduled without blocking the UI", closed.await(5, TimeUnit.SECONDS))
      assertEquals(false, onMain.get())
      assertNull(active.get())
      active.set(replacement)
      release.countDown()
      assertTrue("Close failures must stay in the background", finished.await(5, TimeUnit.SECONDS))
      assertSame("Old close must not detach a replacement connection", replacement, active.get())
      assertFalse(replacementClosed.get())
    } finally {
      release.countDown()
      active.compareAndSet(replacement, null)
      instrumentation.runOnMainSync { lan.unlink() }
    }
  }
}
