package io.github.retype.ime

import androidx.test.platform.app.InstrumentationRegistry
import java.io.ByteArrayInputStream
import java.io.File
import java.net.HttpURLConnection
import java.net.URL
import java.security.MessageDigest
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.CancellationException
import org.junit.Assert.*
import org.junit.Test

class ResourceDownloadsTest {
  private val url = "https://api.github.com/repos/L-Chris/retype/releases/assets/42"
  private class Response(url: URL, private val status: Int, private val body: ByteArray, private val range: String? = null) : HttpURLConnection(url) {
    var closed = false
    override fun connect() {}
    override fun disconnect() { closed = true }
    override fun usingProxy() = false
    override fun getResponseCode() = status
    override fun getInputStream() = ByteArrayInputStream(body)
    override fun getHeaderField(name: String?) = when {
      name.equals("Content-Length", true) -> body.size.toString()
      name.equals("Content-Range", true) -> range
      else -> null
    }
  }
  private fun hash(bytes: ByteArray) = MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }

  @Test fun interruptedArchivesResumeOrRestartAndAlreadyVerifiedFilesNeedNoNetwork() = runBlocking {
    val body = "verified dictionary archive".toByteArray()
    val file = File(InstrumentationRegistry.getInstrumentation().targetContext.cacheDir, "resume-resource-test.part")
    try {
      for (resumed in listOf(true, false)) {
        file.writeBytes(body.copyOfRange(0, 3))
        val response = Response(URL(url), if (resumed) 206 else 200, if (resumed) body.copyOfRange(3, body.size) else body, "bytes 3-${body.size-1}/${body.size}")
        ResourceDownloads.fetch(url, file, body.size.toLong(), hash(body), progress = {}, open = { _, start ->
          assertEquals(3L, start); response
        })
        assertArrayEquals(body, file.readBytes())
        assertTrue(response.closed)
      }
      ResourceDownloads.fetch(url, file, body.size.toLong(), hash(body), progress = {}, open = { _, _ -> error("verified file must not download again") })
    } finally { file.delete() }
  }

  @Test fun wrongRangeAndTruncatedOrCorruptedArchivesNeverActivate() = runBlocking {
    val body = "verified dictionary archive".toByteArray()
    val file = File(InstrumentationRegistry.getInstrumentation().targetContext.cacheDir, "invalid-resource-test.part")
    try {
      file.writeBytes(body.copyOfRange(0, 3))
      val wrongRange = Response(URL(url), 206, body.copyOfRange(3, body.size), "bytes 2-${body.size-1}/${body.size}")
      assertTrue(runCatching { ResourceDownloads.fetch(url, file, body.size.toLong(), hash(body), progress = {}, open = { _, _ -> wrongRange }) }.isFailure)
      assertEquals(3L, file.length())
      assertTrue(wrongRange.closed)
      file.delete()
      assertTrue(runCatching { ResourceDownloads.fetch(url, file, body.size.toLong(), hash(body), progress = {}, open = { _, _ -> Response(URL(url), 200, body.copyOfRange(0, 3)) }) }.isFailure)
      assertTrue(runCatching { ResourceDownloads.fetch(url, file, body.size.toLong(), "0".repeat(64), progress = {}, open = { _, _ -> Response(URL(url), 200, body) }) }.isFailure)
      assertFalse(file.exists())
    } finally { file.delete() }
  }

  @Test fun cancellationKeepsThePartialArchiveAndRetryFinishesIt() = runBlocking {
    val body = ByteArray(128 * 1024) { (it % 251).toByte() }
    val file = File(InstrumentationRegistry.getInstrumentation().targetContext.cacheDir, "cancel-resource-test.part")
    try {
      file.delete()
      assertTrue(runCatching {
        ResourceDownloads.fetch(url, file, body.size.toLong(), hash(body), progress = { throw CancellationException("test cancellation") }, open = { _, _ -> Response(URL(url), 200, body) })
      }.exceptionOrNull() is CancellationException)
      assertEquals(65536L, file.length())
      ResourceDownloads.fetch(url, file, body.size.toLong(), hash(body), progress = {}, open = { _, start ->
        assertEquals(65536L, start)
        Response(URL(url), 206, body.copyOfRange(start.toInt(), body.size), "bytes $start-${body.size-1}/${body.size}")
      })
      assertArrayEquals(body, file.readBytes())
    } finally { file.delete() }
  }

  @Test fun dictionaryRequestsKeepBinaryAndRangeHeadersAcrossRedirectsAndReportHttpErrors() {
    val redirect = object : HttpURLConnection(URL(url)) {
      override fun connect() {}
      override fun disconnect() {}
      override fun usingProxy() = false
      override fun getResponseCode() = 302
      override fun getHeaderField(name: String?) = if (name.equals("Location", true)) "https://release-assets.githubusercontent.com/resource.gz" else null
    }
    val body = Response(URL("https://release-assets.githubusercontent.com/resource.gz"), 206, byteArrayOf(1), "bytes 3-3/4")
    val result = GitHubDownloads.connection(url, asset = true, start = 3, description = "词库") { address ->
      if (address.host == "api.github.com") redirect else body
    }
    for (request in listOf(redirect, body)) {
      assertEquals("application/octet-stream", request.getRequestProperty("Accept"))
      assertEquals("bytes=3-", request.getRequestProperty("Range"))
      assertEquals("identity", request.getRequestProperty("Accept-Encoding"))
    }
    result.disconnect()
    val error = runCatching { GitHubDownloads.connection(url, asset = true, description = "词库") { Response(URL(url), 403, byteArrayOf()) } }.exceptionOrNull()
    assertTrue(error!!.message!!.contains("HTTP 403"))
    assertTrue(error.message!!.contains("词库"))
  }
}
