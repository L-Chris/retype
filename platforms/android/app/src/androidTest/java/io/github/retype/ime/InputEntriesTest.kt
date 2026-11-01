package io.github.retype.ime

import android.content.ComponentName
import android.os.Build
import android.os.SystemClock
import android.view.inputmethod.InputMethodManager
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.uiautomator.*
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class InputEntriesTest {
  @Test
  fun systemPickerSelectsIndependentPredictiveEntries() {
    org.junit.Assume.assumeTrue(Build.VERSION.SDK_INT >= 34)
    val instrumentation = InstrumentationRegistry.getInstrumentation()
    val context = instrumentation.targetContext
    kotlinx.coroutines.runBlocking { ResourceFixtures.prepare(context) }
    val device = UiDevice.getInstance(instrumentation)
    val manager = context.getSystemService(InputMethodManager::class.java)
    val ime = ComponentName(context, RetypeImeService::class.java).flattenToShortString()
    val own = manager.inputMethodList.first { it.id == ime }
    val previous = manager.getEnabledInputMethodSubtypeList(own, false).map { it.hashCode() }.toIntArray()
    val oldIme = device.executeShellCommand("settings get secure default_input_method").trim()
    val enabled = device.executeShellCommand("ime list -s").lineSequence().any { it.trim() == ime }
    fun find(selector: BySelector): UiObject2 {
      val deadline = SystemClock.uptimeMillis() + 10000
      do {
        instrumentation.uiAutomation.clearCache()
        device.findObject(selector)?.let { return it }
        SystemClock.sleep(100)
      } while (SystemClock.uptimeMillis() < deadline)
      throw AssertionError("Missing $selector")
    }
    fun select(label: String, language: String) {
      instrumentation.runOnMainSync { manager.showInputMethodPicker() }
      find(By.textContains(label)).click()
      find(By.desc(if (language == "en-US") "切换至中文" else "切换至英文"))
      assertEquals(language, manager.currentInputMethodSubtype?.languageTag)
    }
    try {
      // A user's explicit language choices make both subtypes visible in the
      // standard picker. This test changes only its private validation IME.
      device.executeShellCommand("ime enable $ime")
      manager.setExplicitlyEnabledInputMethodSubtypes(ime, intArrayOf(1, 2))
      device.executeShellCommand("ime set $ime")
      device.executeShellCommand("am start -W -n ${context.packageName}/io.github.retype.ime.EditorFixtureActivity")
      find(By.desc("editor-normal")).click()
      find(By.desc("retype"))
      select("retype English", "en-US")
      for (letter in "hel") find(By.text(letter.toString())).click()
      find(By.desc("展开候选")).click()
      find(By.desc("candidate-hello")).click()
      assertTrue(find(By.desc("editor-normal")).text.endsWith("hello "))
      // Select the Chinese entry through Android, not retype's internal button.
      select("retype 中文", "zh-CN")
      find(By.text("a")).click()
      find(By.desc("candidate-阿"))
      find(By.desc("editor-password")).click()
      find(By.text("安全输入"))
      find(By.desc("editor-normal")).click()
      find(By.desc("切换至英文"))
      assertEquals("zh-CN", manager.currentInputMethodSubtype?.languageTag)
    } finally {
      manager.setExplicitlyEnabledInputMethodSubtypes(ime, previous)
      if (oldIme != "null" && oldIme.isNotEmpty()) device.executeShellCommand("ime set $oldIme")
      if (!enabled) device.executeShellCommand("ime disable $ime")
    }
  }
}
