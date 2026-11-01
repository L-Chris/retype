package io.github.retype.ime

import android.content.Context
import android.content.Intent
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.uiautomator.*
import java.io.File
import org.junit.Assert.*
import org.junit.Test

class LanPageTest {
  @Test fun deviceListAndLocalIpv4AddressesAreVisible() {
    val instrumentation = InstrumentationRegistry.getInstrumentation()
    val context = instrumentation.targetContext
    val prefs = context.getSharedPreferences("lanClipboard", Context.MODE_PRIVATE)
    val store = LanPeerStore(prefs, SecretVault(context))
    val lan = LanClipboard.get(context)
    val device = UiDevice.getInstance(instrumentation)
    fun find(selector: BySelector): UiObject2? {
      if (android.os.Build.VERSION.SDK_INT >= 33) instrumentation.uiAutomation.clearCache()
      return device.findObject(selector)
    }
    try {
      lan.unlinkAllForTest()
      store.upsert(LanPeer("a".repeat(32), "Test MacBook", "synthetic-pin", "10.0.2.2:1",
          manualAddresses = "[fd7a:115c:a1e0::1]:50206, 100.64.0.1:50206"))
      store.upsert(LanPeer("b".repeat(32), "Test Windows", "synthetic-pin", "10.0.2.2:2"))
      instrumentation.runOnMainSync { lan.enable(true) }
      context.startActivity(Intent(context, SettingsActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK))
      assertTrue(device.wait(Until.hasObject(By.text("retype 输入法")), 5000))
      repeat(8) {
        if (find(By.text("跨设备")) == null) {
          device.swipe(device.displayWidth / 2, device.displayHeight * 4 / 5,
              device.displayWidth / 2, device.displayHeight / 3, 20)
        }
      }
      val navigation = find(By.text("跨设备"))
      assertNotNull("Cross-device navigation must be reachable", navigation)
      navigation!!.click()
      assertTrue(device.wait(Until.hasObject(By.text("Test MacBook")), 5000))
      assertTrue(device.hasObject(By.text("Test Windows")))
      find(By.desc("Test MacBook的连接地址"))!!.click()
      val xml = File(context.cacheDir, "lan-device-list.xml")
      device.dumpWindowHierarchy(xml)
      val hierarchy = xml.readText()
      assertFalse(hierarchy.contains("fd7a:"))
      assertTrue(hierarchy.contains("100.64.0.1:50206"))
      repeat(5) {
        if (find(By.textContains("其他设备可填写以上地址")) == null) {
          device.swipe(device.displayWidth / 2, device.displayHeight * 4 / 5,
              device.displayWidth / 2, device.displayHeight / 3, 20)
        }
      }
      assertNotNull(find(By.text("本机地址")))
      assertNotNull(find(By.textContains("其他设备可填写以上地址")))
      assertTrue(lan.state.value.localAddresses.isNotEmpty())
      assertTrue(lan.state.value.localAddresses.all { LanEndpoint.visible(it) && it.endsWith(":${lan.state.value.port}") })
      device.takeScreenshot(File(context.cacheDir, "lan-multiple-ipv4.png"))
    } finally {
      device.dumpWindowHierarchy(File(context.cacheDir, "lan-ui-final.xml"))
      device.takeScreenshot(File(context.cacheDir, "lan-ui-final.png"))
      lan.unlinkAllForTest()
      device.pressBack()
    }
  }
}
