package io.github.retype.ime

import androidx.core.content.FileProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.io.ByteArrayInputStream
import java.io.File
import java.net.HttpURLConnection
import java.net.URL
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
  private val assetPrefix = "https://api.github.com/repos/L-Chris/retype/releases/assets/"

  private fun apkId(v: String, abi: String? = null) =
      AppUpdates.versionCode(v).toLong() * 10 + when (abi) { "arm64-v8a" -> 1; "x86_64" -> 3; else -> 5 }

  private fun release(v: String, prerelease: Boolean = false, apk: Boolean = true, abi: String? = null): JSONObject {
    val name = "retype-$v-android${abi?.let { "-$it" } ?: ""}.apk"
    val prefix = "https://github.com/L-Chris/retype/releases/download/v$v/"
    val assets = JSONArray()
    if (apk)
        assets.put(
            JSONObject()
                .put("name", name)
                .put("id", apkId(v, abi))
                .put("url", "$assetPrefix${apkId(v, abi)}")
                .put("browser_download_url", prefix + name)
                .put("size", 70000000))
    assets.put(
        JSONObject()
            .put("name", "$name.sha256")
            .put("id", apkId(v, abi) + 1)
            .put("url", "$assetPrefix${apkId(v, abi) + 1}")
            .put("browser_download_url", prefix + "$name.sha256")
            .put("size", 100))
    return JSONObject().put("tag_name", "v$v").put("prerelease", prerelease).put("assets", assets)
  }

  @Test fun matchingArchitectureWinsAndOtherArchitecturesAreNeverSelected() {
    val universal = release("0.8.0")
    val arm = release("0.8.0", abi = "arm64-v8a")
    val x86 = release("0.8.0", abi = "x86_64")
    val combined = universal.getJSONArray("assets")
    for (r in listOf(arm, x86)) for (i in 0 until r.getJSONArray("assets").length()) combined.put(r.getJSONArray("assets").getJSONObject(i))
    val json = JSONArray().put(universal).toString()
    assertEquals("$assetPrefix${apkId("0.8.0", "arm64-v8a")}", AppUpdates.parseReleases(json, listOf("arm64-v8a"))!!.url)
    assertEquals("$assetPrefix${apkId("0.8.0", "x86_64")}", AppUpdates.parseReleases(json, listOf("x86_64"))!!.url)
    assertNull(AppUpdates.parseReleases(JSONArray().put(arm).toString(), listOf("x86_64")))
    arm.getJSONArray("assets").getJSONObject(0).put("url", "https://example.com/retype.apk")
    assertNull(AppUpdates.parseReleases(JSONArray().put(arm).toString(), listOf("arm64-v8a")))
    assertEquals("$assetPrefix${apkId("0.8.0")}", AppUpdates.parseReleases(JSONArray().put(release("0.8.0")).toString(), listOf("arm64-v8a"))!!.url)
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
        .put("url", "https://example.com/update.apk")
    assertNull(AppUpdates.parseReleases(JSONArray().put(malicious).toString()))
    assertEquals(610, AppUpdates.versionCode("0.6.10"))
    assertTrue(runCatching { AppUpdates.versionCode("0.6.10-beta") }.isFailure)
  }

  @Test fun onlyCanonicalApiAssetsAreAcceptedAndBrowserLinksAreUnused() {
    val fixture = release("0.8.0")
    val assets = fixture.getJSONArray("assets")
    for (i in 0 until assets.length()) assets.getJSONObject(i).remove("browser_download_url")
    val selected = AppUpdates.parseReleases(JSONArray().put(fixture).toString())!!
    assertEquals("$assetPrefix${apkId("0.8.0")}", selected.url)
    assertEquals("$assetPrefix${apkId("0.8.0") + 1}", selected.hashUrl)
    for (url in listOf("https://api.github.com/repos/other/retype/releases/assets/${apkId("0.8.0")}",
        "http://api.github.com/repos/L-Chris/retype/releases/assets/${apkId("0.8.0")}",
        selected.url + "?redirect=1", selected.url + "/extra")) {
      assets.getJSONObject(0).put("url", url)
      assertNull(AppUpdates.parseReleases(JSONArray().put(fixture).toString()))
    }
    assets.getJSONObject(0).put("url", selected.url).remove("id")
    assertNull(AppUpdates.parseReleases(JSONArray().put(fixture).toString()))
  }

  @Test fun cachedBrowserLinksMustBeRefreshedAndApiCacheRoundTrips() {
    val selected = AppUpdates.parseReleases(JSONArray().put(release("0.8.0")).toString())!!
    assertEquals(selected, AppUpdates.cachedRelease(selected.json().toString()))
    val old = selected.json().put("url", "https://github.com/L-Chris/retype/releases/download/v0.8.0/retype-0.8.0-android.apk")
        .put("hashUrl", "https://github.com/L-Chris/retype/releases/download/v0.8.0/retype-0.8.0-android.apk.sha256")
    assertNull(AppUpdates.cachedRelease(old.toString()))
    assertNull(AppUpdates.cachedRelease(selected.json().put("code", 999).toString()))
    assertNull(AppUpdates.cachedRelease(selected.json().put("size", -1).toString()))
  }

  private class Response(url: URL, private val status: Int, private val bytes: ByteArray = byteArrayOf(), private val location: String? = null) : HttpURLConnection(url) {
    var disconnected = false
    override fun connect() { }
    override fun disconnect() { disconnected = true }
    override fun usingProxy() = false
    override fun getResponseCode() = status
    override fun getInputStream() = ByteArrayInputStream(bytes)
    override fun getHeaderField(name: String?) = if (name.equals("Location", true)) location else null
  }

  @Test fun apiDownloadsNegotiateBinaryAndHandleDirectOrRedirectedBodies() {
    val api = "${assetPrefix}1"
    val body = byteArrayOf(0, 1, 2, -1)
    val direct = Response(URL(api), 200, body)
    val c = AppUpdates.connection(api, asset = true) { direct }
    try {
      assertEquals("application/octet-stream", c.getRequestProperty("Accept"))
      assertEquals("2026-03-10", c.getRequestProperty("X-GitHub-Api-Version"))
      assertEquals("identity", c.getRequestProperty("Accept-Encoding"))
      assertFalse(c.useCaches)
      assertArrayEquals(body, c.inputStream.readBytes())
    } finally { c.disconnect() }

    val cdn = "https://release-assets.githubusercontent.com/example.apk?signature=test"
    val redirect = Response(URL(api), 302, location = cdn)
    val file = Response(URL(cdn), 200, body)
    val visited = mutableListOf<String>()
    val redirected = AppUpdates.connection(api, asset = true) { url ->
      visited += url.toString()
      if (url.toString() == api) redirect else file
    }
    try {
      assertEquals(listOf(api, cdn), visited)
      assertTrue(redirect.disconnected)
      assertEquals("application/octet-stream", file.getRequestProperty("Accept"))
      assertNull(file.getRequestProperty("X-GitHub-Api-Version"))
      assertArrayEquals(body, redirected.inputStream.readBytes())
    } finally { redirected.disconnect() }

    val metadata = Response(URL("https://api.github.com/repos/L-Chris/retype/releases"), 200)
    AppUpdates.connection(metadata.url.toString()) { metadata }.disconnect()
    assertEquals("application/vnd.github+json", metadata.getRequestProperty("Accept"))
  }

  @Test fun binaryDownloadsRefuseBrowserEntrypointsAndUnsafeRedirects() {
    assertTrue(runCatching {
      AppUpdates.connection("https://github.com/L-Chris/retype/releases/download/v0.8.0/update.apk", asset = true) { error("must not connect") }
    }.isFailure)
    for (next in listOf("http://release-assets.githubusercontent.com/file.apk", "https://example.com/file.apk",
        "https://api.github.com:444/file.apk", "https://user@api.github.com/file.apk")) {
      val redirect = Response(URL("${assetPrefix}1"), 302, location = next)
      var calls = 0
      assertTrue(runCatching {
        AppUpdates.connection(redirect.url.toString(), asset = true) { calls++; redirect }
      }.isFailure)
      assertEquals(1, calls)
      assertTrue(redirect.disconnected)
    }
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
