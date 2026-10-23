package io.github.retype.ime

import android.os.Build
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.io.File
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.runBlocking
import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith

/** Explicit public-network smoke test; downloads into test cache without installing. */
@RunWith(AndroidJUnit4::class)
class AppUpdateNetworkTest {
  @Suppress("DEPRECATION")
  @Test fun downloadPublishedApkAndChecksumWithoutAuthentication() = runBlocking(Dispatchers.IO) {
    assumeTrue("Opt in to public GitHub downloads", InstrumentationRegistry.getArguments()
        .getString("retypeLiveUpdateDownload") == "true")
    val context = InstrumentationRegistry.getInstrumentation().targetContext
    val releases = AppUpdates.read("https://api.github.com/repos/L-Chris/retype/releases?per_page=30", 2 * 1024 * 1024)
    val release = AppUpdates.parseReleases(releases) ?: error("No published Android package")
    val expected = AppUpdates.read(release.hashUrl, 8192, asset = true).trim().split(Regex("\\s+"))[0].lowercase()
    assertTrue(expected.matches(Regex("[a-f0-9]{64}")))
    val file = File(context.cacheDir, "live-update-download-test.apk")
    try {
      val connection = AppUpdates.connection(release.url, asset = true)
      try {
        connection.inputStream.use { AppUpdates.copyVerified(it, file, release.size, expected) }
      } finally { connection.disconnect() }
      val archive = context.packageManager.getPackageArchiveInfo(file.absolutePath, 0)
      assertNotNull("Downloaded asset must be an APK", archive)
      assertEquals(context.packageName.removeSuffix(".validation"), archive!!.packageName)
      assertEquals(release.code.toLong(), if (Build.VERSION.SDK_INT >= 28) archive.longVersionCode else archive.versionCode.toLong())
    } finally { file.delete() }
  }
}
