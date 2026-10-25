package io.github.retype.ime

import androidx.test.platform.app.InstrumentationRegistry
import java.io.File
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.runBlocking
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Test

/** Opt-in check of the exact published English dictionary that failed on 0.8.5. */
class ResourceDownloadNetworkTest {
  @Test fun publishedEnglishArchiveDownloadsAndInstallsThroughTheApi() = runBlocking(Dispatchers.IO) {
    assumeTrue("Opt in to public dictionary download", InstrumentationRegistry.getArguments().getString("retypeLiveResourceDownload") == "true")
    val context = InstrumentationRegistry.getInstrumentation().targetContext
    val root = File(context.cacheDir, "live-resource-test-${System.nanoTime()}").apply { mkdirs() }
    try {
      val catalog = BaseResources.catalog()
      val component = (0 until catalog.length()).map { catalog.getJSONObject(it) }.first { it.getString("id") == "english" }
      val release = JSONObject(GitHubDownloads.read(component.getString("releaseUrl"), 2 * 1024 * 1024, description = "词库"))
      val url = JSONObject(NativeBridge.feature(JSONObject().put("type", "resourceDownloadUrl").put("id", "english").put("release", release).toString())).getString("url")
      assertTrue(url.startsWith("https://api.github.com/repos/L-Chris/retype/releases/assets/"))
      val archive = File(root, "english.part")
      ResourceDownloads.fetch(url, archive, component.getLong("compressedBytes"), component.getString("compressedSha256"), progress = {})
      val installed = JSONObject(NativeBridge.feature(JSONObject().put("type", "resourceInstall").put("root", root.absolutePath).put("source", archive.absolutePath).put("id", "english").toString()))
      assertFalse(installed.isNull("english"))
      assertEquals(component.getLong("bytes"), File(installed.getString("english")).length())
    } finally { root.deleteRecursively() }
  }
}
