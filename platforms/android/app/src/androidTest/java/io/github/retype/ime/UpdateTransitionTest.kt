package io.github.retype.ime

import android.content.Intent
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.uiautomator.By
import androidx.test.uiautomator.UiDevice
import androidx.test.uiautomator.Until
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class UpdateTransitionTest {
  @Test
  fun completionCancellationFailureAndCheckingCanReplaceAnAlreadyDrawnProgressBar() {
    val instrumentation = InstrumentationRegistry.getInstrumentation()
    val context = instrumentation.targetContext
    val device = UiDevice.getInstance(instrumentation)
    context.startActivity(Intent().setClassName(context, "io.github.retype.ime.UpdateTransitionFixtureActivity").addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
    fun click(label: String, message: String) {
      val button = device.wait(Until.findObject(By.text(label)), 5000)
      assertNotNull("Missing button: $label", button)
      button.click()
      assertTrue("Update page did not survive $label", device.wait(Until.hasObject(By.text(message)), 5000))
      device.waitForIdle()
    }
    try {
      assertTrue(device.wait(Until.hasObject(By.text("测试下载中")), 10000))
      for ((button, message) in listOf("完成" to "测试下载完成", "取消" to "测试已取消", "失败" to "测试下载失败", "检查" to "测试检查中")) {
        click(button, message)
        click("开始", "测试下载中")
      }
    } finally {
      device.pressBack()
    }
  }
}
