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

  internal fun versionCode(version: String): Int = JSONObject(NativeBridge.feature(
      JSONObject().put("type", "updateVersionCode").put("version", version).toString())).getInt("code")

  internal fun parseReleases(text: String, supportedAbis: List<String> = Build.SUPPORTED_ABIS.toList()): AndroidRelease? {
    val releases = JSONArray(text)
    return runCatching {
      val result = JSONObject(NativeBridge.feature(JSONObject().put("type", "updateSelect")
          .put("releases", releases).put("abis", JSONArray(supportedAbis)).toString()))
      val r = result.optJSONObject("release") ?: return null
      AndroidRelease(r.getString("version"), r.getInt("code"), r.getString("url"), r.getString("hashUrl"), r.getLong("size"), r.getString("notes"))
    }.getOrNull()
  }

  private fun isAssetUrl(url: String): Boolean = JSONObject(NativeBridge.feature(
      JSONObject().put("type", "updateAsset").put("url", url).toString())).getBoolean("valid")

  internal fun cachedRelease(text: String): AndroidRelease? = runCatching {
    val cached = JSONObject(text)
    AndroidRelease(
        cached.getString("version"), cached.getInt("code"), cached.getString("url"),
        cached.getString("hashUrl"), cached.getLong("size"), cached.getString("notes"))
        .takeIf {
          it.code == versionCode(it.version) && it.size in 1..MAX_APK &&
              isAssetUrl(it.url) && isAssetUrl(it.hashUrl) && it.url != it.hashUrl
        }
  }.getOrNull()

  internal fun connection(
      url: String,
      asset: Boolean = false,
      open: (URL) -> HttpURLConnection = { it.openConnection() as HttpURLConnection }
  ): HttpURLConnection = GitHubDownloads.connection(url, asset, open = open)

  internal suspend fun read(url: String, limit: Int, asset: Boolean = false): String =
      GitHubDownloads.read(url, limit, asset)

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
                    prefs.getString("downloadUrl", newer.url) == newer.url &&
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
                    read(release.hashUrl, 8192, asset = true).trim().split(Regex("\\s+"))[0].lowercase()
                check(expected.matches(Regex("[0-9a-f]{64}"))) { "更新校验信息无效" }
                val directory = File(app.cacheDir, "updates").apply { mkdirs() }
                val staging = File(directory, "download.tmp")
                val target = File(directory, "update.apk")
                target.delete()
                try {
                  val c = connection(release.url, asset = true)
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
                      .putString("downloadUrl", release.url)
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
        val cached = prefs.getString("androidRelease", null) ?: return@runCatching
        val release = cachedRelease(cached) ?: run {
          // Old browser download links have no asset IDs; refresh metadata through the API.
          prefs.edit().remove("androidRelease").putLong("lastUpdateCheck", 0).apply()
          return@runCatching
        }
        if (release.code > BuildConfig.VERSION_CODE) {
          val ready =
              prefs.getInt("downloadCode", 0) == release.code &&
                  prefs.getString("downloadUrl", release.url) == release.url &&
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
