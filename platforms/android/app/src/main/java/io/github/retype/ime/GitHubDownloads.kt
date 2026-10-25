package io.github.retype.ime

import java.net.HttpURLConnection
import java.net.URL
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import org.json.JSONObject

/** App-owned GitHub transport shared by package and dictionary downloads. */
internal object GitHubDownloads {
  private const val API_VERSION = "2026-03-10"
  private fun isAssetUrl(url: String): Boolean = JSONObject(NativeBridge.feature(
      JSONObject().put("type", "updateAsset").put("url", url).toString())).getBoolean("valid")

  internal fun connection(
      url: String,
      asset: Boolean = false,
      start: Long = 0,
      description: String = "更新",
      open: (URL) -> HttpURLConnection = { it.openConnection() as HttpURLConnection }
  ): HttpURLConnection {
    require(!asset || isAssetUrl(url)) { "${description}附件地址无效" }
    require(start >= 0)
    var current = url
    repeat(6) {
      val u = URL(current)
      require(
          u.protocol == "https" &&
              u.userInfo == null && u.port in setOf(-1, 443) && u.ref == null &&
              u.host in
                  setOf(
                      "api.github.com",
                      "github.com",
                      "release-assets.githubusercontent.com",
                      "objects.githubusercontent.com")) {
            "${description}地址无效"
          }
      val c = open(u)
      c.connectTimeout = 15000
      c.readTimeout = 30000
      c.instanceFollowRedirects = false
      c.useCaches = false
      c.setRequestProperty("User-Agent", "retype-android/${BuildConfig.VERSION_NAME}")
      c.setRequestProperty("Accept", if (asset) "application/octet-stream" else "application/vnd.github+json")
      c.setRequestProperty("Accept-Encoding", "identity")
      c.setRequestProperty("Cache-Control", "no-cache")
      if (start > 0) c.setRequestProperty("Range", "bytes=$start-")
      if (u.host == "api.github.com") c.setRequestProperty("X-GitHub-Api-Version", API_VERSION)
      val response =
          try {
            c.responseCode
          } catch (e: Exception) {
            c.disconnect()
            throw e
          }
      if (response in listOf(301, 302, 303, 307, 308)) {
        val next = c.getHeaderField("Location")
        c.disconnect()
        require(!next.isNullOrBlank()) { "${description}下载重定向失败" }
        current = URL(u, next).toString()
      } else {
        if (response != 200 && !(start > 0 && response == 206)) {
          c.disconnect()
          error(
              if (response == 403 || response == 429) "${description}请求暂时受限（HTTP $response），请稍后重试"
              else "${description}请求失败（HTTP $response）")
        }
        return c
      }
    }
    error("${description}下载重定向过多")
  }

  internal suspend fun read(url: String, limit: Int, asset: Boolean = false, description: String = "更新"): String {
    val c = connection(url, asset, description = description)
    try {
      return c.inputStream.use { input ->
        val output = java.io.ByteArrayOutputStream()
        val buffer = ByteArray(8192)
        while (true) {
          currentCoroutineContext().ensureActive()
          val count = input.read(buffer)
          if (count < 0) break
          check(output.size() + count <= limit) { "${description}响应过大" }
          output.write(buffer, 0, count)
        }
        output.toString("UTF-8")
      }
    } finally {
      c.disconnect()
    }
  }

}
