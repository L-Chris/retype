package io.github.retype.ime

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import kotlinx.coroutines.*
import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith

/** Opt-in LAN acceptance: associates the phone; never clears settings or writes clipboard text. */
@RunWith(AndroidJUnit4::class)
class PhysicalLanPairingTest {
  @Test fun discoverAndAssociate() = runBlocking {
    val arguments = InstrumentationRegistry.getArguments()
    val code = arguments.getString("lanCode")
    assumeTrue("Explicit physical pairing code required", code != null)
    val instrumentation = InstrumentationRegistry.getInstrumentation()
    val lan = LanClipboard.get(instrumentation.targetContext)
    instrumentation.runOnMainSync { lan.enable(true); lan.join(code!!) }
    withTimeout(35_000) {
      while (lan.state.value.message != "已连接") delay(100)
    }
    assertTrue(lan.state.value.name.isNotEmpty())
    assertNull(lan.state.value.code)
  }
}
