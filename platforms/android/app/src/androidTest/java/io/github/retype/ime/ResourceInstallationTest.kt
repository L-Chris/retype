package io.github.retype.ime

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.io.File
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class ResourceInstallationTest {
  @Test fun applicationHasNoBundledVocabularyAndImportActivatesBothLanguages() {
    val instrumentation = InstrumentationRegistry.getInstrumentation()
    val context = instrumentation.targetContext
    assertFalse(context.assets.list("")!!.any { it.endsWith(".bin") || it.endsWith(".retype-resources") })
    val root = File(context.cacheDir, "resource-test-${System.nanoTime()}")
    val bundle = File(context.cacheDir, "resource-fixture-${System.nanoTime()}.tmp")
    fun call(type: String) = JSONObject(NativeBridge.feature(JSONObject().put("type", type)
        .put("root", root.absolutePath).put("source", bundle.absolutePath).toString()))
    try {
      assertFalse(call("resourceStatus").getBoolean("ready"))
      bundle.writeText("RTRES001invalid")
      try { call("resourceImport"); fail("corrupt bundle accepted") } catch (_: IllegalStateException) { }
      assertFalse(call("resourceStatus").getBoolean("ready"))
      instrumentation.context.assets.open("retype-base.retype-resources").use { input -> bundle.outputStream().use { input.copyTo(it) } }
      val installed = call("resourceImport")
      assertTrue(installed.getBoolean("ready"))
      val base = File(installed.getString("base"))
      assertFalse(base.canWrite())
      val modified = base.lastModified()
      assertTrue(call("resourceImport").getBoolean("ready"))
      assertEquals(modified, base.lastModified())
      val handle = NativeBridge.create(base.absolutePath, File(context.cacheDir, "resource-input.db").absolutePath, true, true, false)
      try {
        var update: NativeUpdate? = null
        for (key in "nihc") update = NativeBridge.send(handle, NativeCommand.key(key.toString()))
        assertTrue(update!!.candidates!!.contains("你好"))
        NativeBridge.send(handle, NativeCommand.key("escape"))
        NativeBridge.send(handle, NativeCommand.toggle())
        for (key in "hel") update = NativeBridge.send(handle, NativeCommand.key(key.toString()))
        assertTrue(update!!.candidates!!.any { it.equals("hello", true) })
      } finally { NativeBridge.destroy(handle) }
    } finally { bundle.delete(); root.deleteRecursively() }
  }
}
