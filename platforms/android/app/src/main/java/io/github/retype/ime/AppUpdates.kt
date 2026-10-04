package io.github.retype.ime

import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.provider.Settings
import androidx.core.content.FileProvider
import androidx.work.*
import java.io.File
import java.io.InputStream
import java.net.HttpURLConnection
import java.net.URL
import java.security.MessageDigest
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import org.json.JSONArray
import org.json.JSONObject

internal data class AndroidRelease(
    val version: String,
    val code: Int,
    val url: String,
    val hashUrl: String,
    val size: Long,
    val notes: String
) {
  fun json() =
      JSONObject()
          .put("version", version)
          .put("code", code)
          .put("url", url)
          .put("hashUrl", hashUrl)
          .put("size", size)
          .put("notes", notes)
}

internal data class UpdateStatus(
    val busy: Boolean = false,
    val message: String = "",
    val release: AndroidRelease? = null,
    val progress: Float? = null,
    val ready: Boolean = false
)

internal object AppUpdates {
  private const val API = "https://api.github.com/repos/L-Chris/retype/releases?per_page=30"
  private const val MAX_APK = 256L * 1024 * 1024
  private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
  private val mutex = Mutex()
  private val mutableStatus = MutableStateFlow(UpdateStatus())
  val status = mutableStatus.asStateFlow()
  private var job: Job? = null

  internal fun versionCode(version: String): Int {
    require(version.matches(Regex("[0-9]+\\.[0-9]+\\.[0-9]+"))) { "无效版本号" }
    val p = version.split('.').map { it.toInt() }
    require(p[0] <= 200000 && p[1] <= 99 && p[2] <= 99) { "版本号超出范围" }
    return p[0] * 10000 + p[1] * 100 + p[2]
  }

  internal fun parseReleases(text: String): AndroidRelease? {
    val releases = JSONArray(text)
    val available = mutableListOf<AndroidRelease>()
    for (i in 0 until releases.length()) {
      val r = releases.getJSONObject(i)
      if (r.optBoolean("draft") || r.optBoolean("prerelease")) continue
      val tag = r.optString("tag_name")
      if (!tag.startsWith("v")) continue
      val version = tag.substring(1)
      val code = runCatching { versionCode(version) }.getOrNull() ?: continue
      val name = "retype-$version-android.apk"
      val assets = r.optJSONArray("assets") ?: continue
      var apk: JSONObject? = null
      var hash: JSONObject? = null
      for (a in 0 until assets.length()) {
        val asset = assets.getJSONObject(a)
        if (asset.optString("name") == name) apk = asset
        if (asset.optString("name") == "$name.sha256") hash = asset
      }
      if (apk == null || hash == null) continue
      val prefix = "https://github.com/L-Chris/retype/releases/download/$tag/"
      val url = apk.optString("browser_download_url")
      val hashUrl = hash.optString("browser_download_url")
      val size = apk.optLong("size")
      if (url != prefix + name || hashUrl != prefix + "$name.sha256" || size !in 1..MAX_APK)
          continue
      available +=
          AndroidRelease(version, code, url, hashUrl, size, r.optString("body").take(24000))
    }
    return available.maxByOrNull { it.code }
  }

  private fun connection(url: String): HttpURLConnection {
    var current = url
    repeat(6) {
      val u = URL(current)
      require(
          u.protocol == "https" &&
              u.userInfo == null &&
              u.host in
                  setOf(
                      "api.github.com",
                      "github.com",
                      "release-assets.githubusercontent.com",
                      "objects.githubusercontent.com")) {
            "更新地址无效"
          }
      val c = u.openConnection() as HttpURLConnection
      c.connectTimeout = 15000
      c.readTimeout = 30000
      c.instanceFollowRedirects = false
      c.setRequestProperty("User-Agent", "retype-android/${BuildConfig.VERSION_NAME}")
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
        require(!next.isNullOrBlank()) { "更新下载重定向失败" }
        current = URL(u, next).toString()
      } else {
        if (response != 200) {
          c.disconnect()
          error(
              if (response == 403 || response == 429) "检查更新暂时受限，请稍后重试"
              else "更新请求失败（HTTP $response）")
        }
        return c
      }
    }
    error("更新下载重定向过多")
  }

  private suspend fun read(url: String, limit: Int): String {
    val c = connection(url)
    try {
      return c.inputStream.use { input ->
        val output = java.io.ByteArrayOutputStream()
        val buffer = ByteArray(8192)
        while (true) {
          currentCoroutineContext().ensureActive()
          val count = input.read(buffer)
          if (count < 0) break
          check(output.size() + count <= limit) { "更新响应过大" }
          output.write(buffer, 0, count)
        }
        output.toString("UTF-8")
      }
    } finally {
      c.disconnect()
    }
  }

  suspend fun check(context: Context, automatic: Boolean = false) =
      withContext(Dispatchers.IO) {
        mutex.withLock {
          val prefs = AppStore(context).prefs
          val now = System.currentTimeMillis()
          if (automatic && now - prefs.getLong("lastUpdateCheck", 0) < TimeUnit.DAYS.toMillis(1))
              return@withLock
          prefs.edit().putLong("lastUpdateCheck", now).apply()
          mutableStatus.value = mutableStatus.value.copy(busy = true, message = "检查更新中…")
          try {
            val release = parseReleases(read(API, 2 * 1024 * 1024))
            val newer = release?.takeIf { it.code > BuildConfig.VERSION_CODE }
            prefs.edit().putString("androidRelease", newer?.json()?.toString()).apply()
            val ready =
                newer != null &&
                    prefs.getInt("downloadCode", 0) == newer.code &&
                    File(context.cacheDir, "updates/update.apk").isFile
            mutableStatus.value =
                UpdateStatus(
                    message =
                        if (release == null) "尚未发布 Android 正式安装包"
                        else if (newer == null) "已是最新 Android 版本"
                        else if (ready) "下载完成，可安装更新" else "发现新版本 ${newer.version}",
                    release = newer,
                    ready = ready)
          } catch (e: CancellationException) {
            throw e
          } catch (e: Exception) {
            mutableStatus.value =
                mutableStatus.value.copy(busy = false, message = e.message ?: "检查更新失败")
          } finally {
            mutableStatus.value = mutableStatus.value.copy(busy = false)
          }
        }
      }

  fun checkNow(context: Context, automatic: Boolean = false) {
    if (job?.isActive == true || status.value.busy) return
    val app = context.applicationContext
    job = scope.launch { check(app, automatic) }
  }

  fun download(context: Context) {
    if (job?.isActive == true || status.value.busy) return
    val release = status.value.release ?: return
    val app = context.applicationContext
    job =
        scope.launch {
          mutableStatus.value =
              status.value.copy(busy = true, ready = false, progress = 0f, message = "下载更新中…")
          try {
            withContext(Dispatchers.IO) {
              mutex.withLock {
                val expected =
                    read(release.hashUrl, 8192).trim().split(Regex("\\s+"))[0].lowercase()
                check(expected.matches(Regex("[0-9a-f]{64}"))) { "更新校验信息无效" }
                val directory = File(app.cacheDir, "updates").apply { mkdirs() }
                val staging = File(directory, "download.tmp")
                val target = File(directory, "update.apk")
                target.delete()
                try {
                  val c = connection(release.url)
                  try {
                    c.inputStream.use { input ->
                      copyVerified(input, staging, release.size, expected) { progress ->
                        mutableStatus.value = status.value.copy(progress = progress)
                      }
                    }
                  } finally {
                    c.disconnect()
                  }
                  validateApk(app, staging, release.code)
                  check(staging.renameTo(target)) { "无法保存更新文件" }
                  AppStore(app)
                      .prefs
                      .edit()
                      .putString("downloadHash", expected)
                      .putInt("downloadCode", release.code)
                      .apply()
                } finally {
                  staging.delete()
                }
              }
            }
            mutableStatus.value =
                status.value.copy(
                    busy = false, ready = true, progress = null, message = "下载完成，可安装更新")
          } catch (e: CancellationException) {
            mutableStatus.value =
                status.value.copy(busy = false, progress = null, message = "已取消下载")
            throw e
          } catch (e: Exception) {
            mutableStatus.value =
                status.value.copy(busy = false, progress = null, message = e.message ?: "更新下载失败")
          }
        }
  }

  fun cancel() {
    job?.cancel()
  }

  internal suspend fun copyVerified(
      input: InputStream,
      target: File,
      size: Long,
      expected: String,
      progress: (Float) -> Unit = {}
  ) {
    require(size in 1..MAX_APK && expected.matches(Regex("[0-9a-f]{64}"))) { "更新校验信息无效" }
    val digest = MessageDigest.getInstance("SHA-256")
    var bytes = 0L
    var percent = -1
    target.outputStream().use { output ->
      val buffer = ByteArray(65536)
      while (true) {
        currentCoroutineContext().ensureActive()
        val count = input.read(buffer)
        if (count < 0) break
        bytes += count
        check(bytes <= size) { "更新文件大小异常" }
        digest.update(buffer, 0, count)
        output.write(buffer, 0, count)
        val next = (bytes * 100 / size).toInt()
        if (next != percent) {
          percent = next
          progress(bytes.toFloat() / size)
        }
      }
      output.fd.sync()
    }
    check(bytes == size) { "更新下载不完整，请重试" }
    check(digest.digest().joinToString("") { "%02x".format(it) } == expected) { "更新文件校验失败，请重试" }
  }

  @Suppress("DEPRECATION")
  internal fun validateApk(context: Context, apk: File, code: Int) {
    val pm = context.packageManager
    val flags =
        if (Build.VERSION.SDK_INT >= 28) PackageManager.GET_SIGNING_CERTIFICATES
        else PackageManager.GET_SIGNATURES
    val candidate = pm.getPackageArchiveInfo(apk.absolutePath, flags) ?: error("更新安装包无效")
    val installed = pm.getPackageInfo(context.packageName, flags)
    val version =
        if (Build.VERSION.SDK_INT >= 28) candidate.longVersionCode
        else candidate.versionCode.toLong()
    check(
        candidate.packageName == context.packageName &&
            version == code.toLong() &&
            version > BuildConfig.VERSION_CODE) {
          "更新安装包名称或版本不匹配"
        }
    fun signatures(p: android.content.pm.PackageInfo): Set<String> =
        (if (Build.VERSION.SDK_INT >= 28) p.signingInfo?.apkContentsSigners else p.signatures)
            ?.map { it.toCharsString() }
            ?.toSet() ?: emptySet()
    val expected = signatures(installed)
    check(sameSigners(expected, signatures(candidate))) { "更新签名不一致；预览版与正式版不能直接覆盖安装" }
  }

  internal fun sameSigners(installed: Set<String>, candidate: Set<String>): Boolean =
      installed.isNotEmpty() && installed == candidate

  fun install(context: Context) {
    val prefs = AppStore(context).prefs
    if (!context.packageManager.canRequestPackageInstalls()) {
      prefs.edit().putBoolean("pendingUpdateInstall", true).apply()
      context.startActivity(
          Intent(
              Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES,
              Uri.parse("package:${context.packageName}")))
      return
    }
    prefs.edit().putBoolean("pendingUpdateInstall", false).apply()
    scope.launch {
      try {
        val apk = File(context.cacheDir, "updates/update.apk")
        withContext(Dispatchers.IO) {
          val expected = prefs.getString("downloadHash", "")!!
          val digest = MessageDigest.getInstance("SHA-256")
          apk.inputStream().use { input ->
            val b = ByteArray(65536)
            while (true) {
              ensureActive()
              val n = input.read(b)
              if (n < 0) break
              digest.update(b, 0, n)
            }
          }
          check(
              expected.length == 64 &&
                  digest.digest().joinToString("") { "%02x".format(it) } == expected) {
                "下载文件已失效，请重新下载"
              }
          validateApk(context, apk, prefs.getInt("downloadCode", 0))
        }
        val uri = FileProvider.getUriForFile(context, "${context.packageName}.updates", apk)
        context.startActivity(
            Intent(Intent.ACTION_VIEW)
                .setDataAndType(uri, "application/vnd.android.package-archive")
                .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION))
      } catch (e: Exception) {
        mutableStatus.value = status.value.copy(message = e.message ?: "无法打开安装界面")
      }
    }
  }

  fun schedule(context: Context) {
    if (status.value.release == null && !status.value.busy) {
      runCatching {
        val prefs = AppStore(context).prefs
        val cached = JSONObject(prefs.getString("androidRelease", null) ?: return@runCatching)
        val release =
            AndroidRelease(
                cached.getString("version"),
                cached.getInt("code"),
                cached.getString("url"),
                cached.getString("hashUrl"),
                cached.getLong("size"),
                cached.getString("notes"))
        if (release.code > BuildConfig.VERSION_CODE) {
          val ready =
              prefs.getInt("downloadCode", 0) == release.code &&
                  File(context.cacheDir, "updates/update.apk").isFile
          mutableStatus.value =
              UpdateStatus(
                  release = release,
                  ready = ready,
                  message = if (ready) "下载完成，可安装更新" else "发现新版本 ${release.version}")
        }
      }
    }
    val manager = WorkManager.getInstance(context)
    if (!AppStore(context).prefs.getBoolean("updatesAutoCheck", true)) {
      manager.cancelUniqueWork("retype-updates")
      return
    }
    manager.enqueueUniquePeriodicWork(
        "retype-updates",
        ExistingPeriodicWorkPolicy.KEEP,
        PeriodicWorkRequestBuilder<UpdateWorker>(1, TimeUnit.DAYS)
            .setConstraints(
                Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build())
            .build())
    if (System.currentTimeMillis() - AppStore(context).prefs.getLong("lastUpdateCheck", 0) >
        TimeUnit.DAYS.toMillis(1))
        checkNow(context, automatic = true)
  }
}

class UpdateWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {
  override suspend fun doWork(): Result {
    if (AppStore(applicationContext).prefs.getBoolean("updatesAutoCheck", true))
        AppUpdates.check(applicationContext, automatic = true)
    return Result.success()
  }
}
