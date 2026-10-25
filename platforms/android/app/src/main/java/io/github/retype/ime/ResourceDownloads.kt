package io.github.retype.ime

import java.io.File
import java.io.FileOutputStream
import java.net.HttpURLConnection
import java.security.MessageDigest
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

/** Keep incomplete archives across worker cancellation/retry; activate only verified files. */
internal object ResourceDownloads {
  private val mutex = Mutex()
  private suspend fun checksum(file: File): String {
    val digest = MessageDigest.getInstance("SHA-256")
    file.inputStream().use { input ->
      val buffer = ByteArray(65536)
      while (true) {
        currentCoroutineContext().ensureActive()
        val count = input.read(buffer)
        if (count < 0) break
        digest.update(buffer, 0, count)
      }
    }
    return digest.digest().joinToString("") { "%02x".format(it) }
  }

  suspend fun fetch(
      url: String,
      file: File,
      size: Long,
      hash: String,
      progress: suspend (Long) -> Unit,
      open: (String, Long) -> HttpURLConnection = { address, start ->
        GitHubDownloads.connection(address, asset = true, start = start, description = "词库")
      },
  ) = mutex.withLock {
    require(size in 1..128L * 1024 * 1024 && hash.matches(Regex("[0-9a-f]{64}"))) { "词库校验清单无效" }
    file.parentFile?.mkdirs()
    var start = file.length()
    if (start == size && checksum(file) == hash) { progress(size); return@withLock }
    if (start >= size) start = 0
    val connection = open(url, start)
    try {
      if (connection.responseCode == 206) {
        check(connection.getHeaderField("Content-Range") == "bytes $start-${size - 1}/$size") { "词库续传响应无效" }
      } else {
        check(connection.responseCode == 200) { "词库请求失败（HTTP ${connection.responseCode}）" }
        start = 0
      }
      check(connection.contentLengthLong < 0 || connection.contentLengthLong == size - start) { "词库下载大小无效" }
      var received = start
      var lastProgress = 0L
      connection.inputStream.use { input ->
        FileOutputStream(file, start > 0).use { output ->
          val buffer = ByteArray(65536)
          while (true) {
            currentCoroutineContext().ensureActive()
            val count = input.read(buffer)
            if (count < 0) break
            check(received + count <= size) { "词库下载超出大小限制" }
            output.write(buffer, 0, count)
            received += count
            val now = System.nanoTime()
            if (now - lastProgress >= 100_000_000 || received == size) { progress(received); lastProgress = now }
          }
          output.fd.sync()
        }
      }
      check(received == size) { "词库下载未完成，可重试继续下载" }
      if (checksum(file) != hash) {
        file.delete()
        error("词库下载校验失败，请重试")
      }
    } finally {
      connection.disconnect()
    }
  }
}
