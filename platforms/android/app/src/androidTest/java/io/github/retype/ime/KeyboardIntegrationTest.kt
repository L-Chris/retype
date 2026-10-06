package io.github.retype.ime

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.uiautomator.*
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class KeyboardIntegrationTest {
  @Test
  fun touchCompositionEnglishAndPrivateFieldSwitch() {
    val instrumentation = InstrumentationRegistry.getInstrumentation()
    val device = UiDevice.getInstance(instrumentation)
    // Compose updates render correctly, but an in-process instrumentation runner can
    // retain old accessibility nodes. Query a fresh tree after every input change.
    fun find(selector: BySelector, timeout: Long = 5000): UiObject2? {
      val deadline = android.os.SystemClock.uptimeMillis() + timeout
      do {
        if (android.os.Build.VERSION.SDK_INT >= 33) instrumentation.uiAutomation.clearCache()
        device.findObject(selector)?.let {
          return it
        }
        android.os.SystemClock.sleep(100)
      } while (android.os.SystemClock.uptimeMillis() < deadline)
      return null
    }
    val prefs = instrumentation.targetContext.getSharedPreferences("settings", 0)
    val oldFlypy = prefs.getBoolean("flypy", false)
    val oldChinese = prefs.getBoolean("chinese", true)
    val statistics = TypingStatistics.get(instrumentation.targetContext)
    val initialWords = statistics.summary(0).current.englishWords
    fun expectWords(words: Long) {
      val deadline = android.os.SystemClock.uptimeMillis() + 5000
      while (statistics.summary(0).current.englishWords != words && android.os.SystemClock.uptimeMillis() < deadline) {
        android.os.SystemClock.sleep(50)
      }
      assertEquals(words, statistics.summary(0).current.englishWords)
    }
    val oldIme = device.executeShellCommand("settings get secure default_input_method").trim()
    val ime = "io.github.retype.ime/.RetypeImeService"
    val wasEnabled =
        device.executeShellCommand("ime list -s").lineSequence().any { it.trim() == ime }
    try {
      prefs.edit().putBoolean("flypy", true).putBoolean("chinese", true).commit()
      device.executeShellCommand("ime enable $ime")
      device.executeShellCommand("ime set $ime")
      val launched =
          device.executeShellCommand("am start -W -n io.github.retype.ime/.EditorFixtureActivity")
      assertFalse(launched, launched.contains("Error:"))
      val editor = find(By.desc("editor-normal"), 10000)
      assertNotNull(editor)
      editor!!.click()
      assertNotNull(find(By.desc("retype"), 10000))
      for (letter in "wo") find(By.text(letter.toString()))!!.click()
      assertNotNull(find(By.desc("candidate-我")))
      find(By.text("u"))!!.click()
      assertNotNull("Third letter must refresh phrase candidates", find(By.desc("candidate-我是")))
      find(By.text("i"))!!.click()
      find(By.desc("展开候选"))!!.click()
      assertNotNull(find(By.desc("收起候选")))
      assertNull("Expanded candidates must replace the letter keys", find(By.text("q"), 500))
      val phrase = find(By.desc("candidate-我是"))
      assertNotNull("woui must match 我是", phrase)
      phrase!!.click()
      assertNotNull("Choosing a word must restore the keyboard", find(By.text("q")))
      assertEquals("我是", find(By.desc("editor-normal"))!!.text)
      find(By.text("⌫"))!!.click()
      find(By.text("⌫"))!!.click()
      for (letter in "nihc") find(By.text(letter.toString()))!!.click()
      val candidate = find(By.desc("candidate-你好"), 10000)
      assertNotNull(candidate)
      candidate!!.click()
      assertNotNull(find(By.text("你好")))
      val mode = device.findObject(By.desc("切换至英文"))
      mode.click()
      device.waitForIdle()
      if (android.os.Build.VERSION.SDK_INT >= 33) instrumentation.uiAutomation.clearCache()
      val switched = find(By.desc("切换至中文")) != null
      if (!switched) {
        device.dumpWindowHierarchy(
            java.io.File(instrumentation.targetContext.cacheDir, "switch-failure.xml"))
        device.executeShellCommand("screencap -p /sdcard/Download/retype-switch-failure.png")
      }
      assertTrue("English mode must update the keyboard", switched)
      for (letter in "hel") find(By.text(letter.toString()))!!.click()
      val hello = find(By.desc("candidate-hello"))
      assertNotNull(hello)
      hello!!.click()
      assertEquals("你好hello ", find(By.desc("editor-normal"))!!.text)
      expectWords(initialWords + 1)
      for (letter in "ni") find(By.text(letter.toString()))!!.click()
      find(By.desc("editor-password"))!!.click()
      assertNotNull(find(By.text("安全输入")))
      assertFalse(device.hasObject(By.desc("candidate-hello")))
      find(By.text("a"))!!.click()
      expectWords(initialWords + 2)
      find(By.desc("editor-email"))!!.click()
      assertNotNull(find(By.desc("retype"), 10000))
      assertTrue(device.hasObject(By.desc("切换至中文")))
      assertTrue(device.findObject(By.desc("editor-email")).text in listOf("", "email"))
      assertEquals(
          "你好hello ni",
          find(By.desc("editor-normal"))!!.text,
      ) // Old composition stays in its old field.
    } finally {
      prefs.edit().putBoolean("flypy", oldFlypy).putBoolean("chinese", oldChinese).commit()
      if (oldIme != "null" && oldIme.isNotEmpty())
          device.executeShellCommand("ime set '${oldIme.replace("'", "'\\''")}'")
      if (!wasEnabled) device.executeShellCommand("ime disable $ime")
    }
  }
}
