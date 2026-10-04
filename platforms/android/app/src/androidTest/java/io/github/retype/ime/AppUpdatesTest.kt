package io.github.retype.ime

import androidx.core.content.FileProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.io.ByteArrayInputStream
import java.io.File
import java.security.MessageDigest
import kotlinx.coroutines.runBlocking
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class AppUpdatesTest {
  private val context = InstrumentationRegistry.getInstrumentation().targetContext

  private fun release(v: String, prerelease: Boolean = false, apk: Boolean = true): JSONObject {
    val name = "retype-$v-android.apk"
    val prefix = "https://github.com/L-Chris/retype/releases/download/v$v/"
    val assets = JSONArray()
    if (apk)
        assets.put(
            JSONObject()
                .put("name", name)
                .put("browser_download_url", prefix + name)
                .put("size", 70000000))
    assets.put(
        JSONObject()
            .put("name", "$name.sha256")
            .put("browser_download_url", prefix + "$name.sha256")
            .put("size", 100))
    return JSONObject().put("tag_name", "v$v").put("prerelease", prerelease).put("assets", assets)
  }

  @Test
  fun releaseSelectionRequiresStableAndroidPackageAndChecksum() {
    val array =
        JSONArray()
            .put(release("0.7.0", apk = false))
            .put(release("0.8.0", prerelease = true))
            .put(release("0.6.9"))
            .put(release("0.6.10"))
    assertEquals("0.6.10", AppUpdates.parseReleases(array.toString())!!.version)
    assertNull(AppUpdates.parseReleases(JSONArray().put(release("0.7.0", apk = false)).toString()))
    val malicious = release("0.7.0")
    malicious
        .getJSONArray("assets")
        .getJSONObject(0)
        .put("browser_download_url", "https://example.com/update.apk")
    assertNull(AppUpdates.parseReleases(JSONArray().put(malicious).toString()))
    assertEquals(610, AppUpdates.versionCode("0.6.10"))
    assertTrue(runCatching { AppUpdates.versionCode("0.6.10-beta") }.isFailure)
  }

  @Test
  fun streamingDownloadRejectsTruncationOversizeAndCorruption() = runBlocking {
    val data = ByteArray(2 * 1024 * 1024) { (it % 251).toByte() }
    val hash =
        MessageDigest.getInstance("SHA-256").digest(data).joinToString("") { "%02x".format(it) }
    val target = File(context.cacheDir, "update-copy-test.tmp")
    try {
      AppUpdates.copyVerified(ByteArrayInputStream(data), target, data.size.toLong(), hash)
      assertEquals(data.size.toLong(), target.length())
      assertTrue(
          runCatching {
                AppUpdates.copyVerified(
                    ByteArrayInputStream(data.copyOf(data.size - 1)),
                    target,
                    data.size.toLong(),
                    hash)
              }
              .isFailure)
      assertTrue(
          runCatching { AppUpdates.copyVerified(ByteArrayInputStream(data), target, 100, hash) }
              .isFailure)
      val corrupt = data.clone().apply { this[0] = 1 }
      assertTrue(
          runCatching {
                AppUpdates.copyVerified(
                    ByteArrayInputStream(corrupt), target, data.size.toLong(), hash)
              }
              .isFailure)
    } finally {
      target.delete()
    }
  }

  @Test
  fun installerRejectsDowngradeAndProviderExposesOnlyUpdateDirectory() {
    assertTrue(AppUpdates.sameSigners(setOf("original"), setOf("original")))
    assertFalse(AppUpdates.sameSigners(setOf("original"), setOf("different")))
    assertFalse(AppUpdates.sameSigners(emptySet(), emptySet()))
    assertFalse(AppUpdates.sameSigners(setOf("original"), setOf("original", "additional")))
    assertTrue(
        runCatching {
              AppUpdates.validateApk(
                  context, File(context.applicationInfo.sourceDir), BuildConfig.VERSION_CODE)
            }
            .isFailure)
    val allowed =
        File(context.cacheDir, "updates/provider-test.apk").apply {
          parentFile!!.mkdirs()
          writeText("test")
        }
    val privateFile = File(context.filesDir, "private-provider-test").apply { writeText("private") }
    try {
      assertEquals(
          "content",
          FileProvider.getUriForFile(context, "${context.packageName}.updates", allowed).scheme)
      assertTrue(
          runCatching {
                FileProvider.getUriForFile(context, "${context.packageName}.updates", privateFile)
              }
              .isFailure)
    } finally {
      allowed.delete()
      privateFile.delete()
    }
  }
}
