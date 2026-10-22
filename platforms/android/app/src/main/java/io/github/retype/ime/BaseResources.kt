package io.github.retype.ime

import android.app.DownloadManager
import android.content.Context
import android.net.Uri
import androidx.work.*
import java.io.File
import java.util.concurrent.atomic.AtomicBoolean
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import org.json.JSONArray
import org.json.JSONObject

data class ResourceState(val checked: Boolean = false, val ready: Boolean = false,
    val base: String? = null, val bytes: Long = 0, val busy: Boolean = false,
    val progress: Float = 0f, val message: String = "")

object BaseResources {
  private const val WORK = "retype-base-resources"
  private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
  private val mutex = Mutex()
  private val watching = AtomicBoolean()
  private val mutable = MutableStateFlow(ResourceState())
  val state = mutable.asStateFlow()
  fun root(context: Context) = File(context.filesDir, "dictionaries")
  private fun operation(context: Context, type: String) = JSONObject().put("type", type).put("root", root(context).absolutePath)
  fun catalog(): JSONArray = JSONObject(NativeBridge.feature("{\"type\":\"resourceCatalog\"}")).getJSONArray("components")

  suspend fun refresh(context: Context): ResourceState = withContext(Dispatchers.IO) {
    mutex.withLock {
      val legacy = context.filesDir.listFiles()?.filter { it.name.startsWith("dict-") && it.name.endsWith(".bin") }?.sortedByDescending { it.lastModified() } ?: emptyList()
      val result = JSONObject(NativeBridge.feature(operation(context, "resourcePrepare").put("legacy", JSONArray(legacy.map { it.absolutePath })).toString()))
      mutable.value = mutable.value.copy(checked = true, ready = result.getBoolean("ready"),
          base = if (result.isNull("base")) null else result.getString("base"), bytes = result.getLong("downloadBytes"))
      mutable.value
    }
  }
  fun observe(context: Context) {
    if (!watching.compareAndSet(false, true)) return
    val app = context.applicationContext
    scope.launch {
      runCatching {
        val current = refresh(app)
        // Upgrade an existing offline installation on Wi-Fi; fresh installs ask first.
        val prefs = app.getSharedPreferences("base-downloads", 0)
        val signature = catalog().toString()
        if (current.base != null && !current.ready && prefs.getString("automaticCatalog", "") != signature) {
          prefs.edit().putString("automaticCatalog", signature).apply()
          start(app, automatic = true)
        }
      }.onFailure { mutable.value = mutable.value.copy(checked = true, message = it.message ?: "词库检查失败") }
      WorkManager.getInstance(app).getWorkInfosForUniqueWorkFlow(WORK).collect { infos ->
        val id = app.getSharedPreferences("base-downloads", 0).getString("work", "")
        val info = infos.firstOrNull { it.id.toString() == id } ?: return@collect
        mutable.value = mutable.value.copy(busy = !info.state.isFinished, progress = info.progress.getFloat("progress", 0f),
            message = if (info.state == WorkInfo.State.FAILED) info.outputData.getString("error") ?: "词库安装失败，请重试"
                else if (info.state == WorkInfo.State.ENQUEUED) "等待网络连接…"
                else info.progress.getString("message") ?: "")
        if (info.state.isFinished) runCatching { refresh(app) }
      }
    }
  }
  fun start(context: Context, source: Uri? = null, automatic: Boolean = false) {
    observe(context)
    if (mutable.value.busy) return
    val request = OneTimeWorkRequestBuilder<ResourceWorker>()
        .setInputData(workDataOf("source" to source?.toString(), "automatic" to automatic))
        .setConstraints(Constraints.Builder().apply { if (source == null) setRequiredNetworkType(if (automatic) NetworkType.UNMETERED else NetworkType.CONNECTED) }.build()).build()
    context.getSharedPreferences("base-downloads", 0).edit().putString("work", request.id.toString()).apply()
    mutable.value = mutable.value.copy(busy = true, progress = 0f, message = "准备词库…")
    WorkManager.getInstance(context).enqueueUniqueWork(WORK, ExistingWorkPolicy.REPLACE, request)
  }
  fun cancel(context: Context) {
    WorkManager.getInstance(context).cancelUniqueWork(WORK)
    val prefs = context.getSharedPreferences("base-downloads", 0)
    val manager = context.getSystemService(DownloadManager::class.java)
    for (key in listOf("base", "english")) { val id = prefs.getLong(key, -1); if (id >= 0) manager.remove(id) }
    prefs.edit().remove("base").remove("english").apply()
    mutable.value = mutable.value.copy(busy = false, message = "")
  }
  suspend fun install(context: Context, type: String, file: File, id: String? = null) {
    withContext(Dispatchers.IO) {
      val request = operation(context, type).put("source", file.absolutePath)
      if (id != null) request.put("id", id)
      NativeBridge.feature(request.toString())
      refresh(context)
    }
  }
}

class ResourceWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {
  override suspend fun doWork(): Result = withContext(Dispatchers.IO) {
    val context = applicationContext
    val temporary = File(context.cacheDir, "resource-$id.tmp")
    try {
      val source = inputData.getString("source")
      if (source != null) {
        val components = BaseResources.catalog()
        val maximum = (0 until components.length()).sumOf { components.getJSONObject(it).getLong("compressedBytes") } + 24
        context.contentResolver.openInputStream(Uri.parse(source))?.use { input ->
          temporary.outputStream().use { out ->
            val buffer = ByteArray(65536); var total = 0L
            while (true) { currentCoroutineContext().ensureActive(); val n = input.read(buffer); if (n < 0) break; total += n; check(total <= maximum) { "词库包超出大小限制" }; out.write(buffer, 0, n) }
            out.fd.sync()
          }
        } ?: error("无法读取词库包")
        setProgress(workDataOf("message" to "校验并安装词库…"))
        BaseResources.install(context, "resourceImport", temporary)
      } else {
        val manager = context.getSystemService(DownloadManager::class.java)
        val prefs = context.getSharedPreferences("base-downloads", 0)
        val components = BaseResources.catalog()
        for (index in 0 until components.length()) {
          val component = components.getJSONObject(index); val key = component.getString("id")
          val current = JSONObject(NativeBridge.feature(JSONObject().put("type", "resourceStatus").put("root", BaseResources.root(context).absolutePath).toString()))
          if (!(0 until current.getJSONArray("missing").length()).any { current.getJSONArray("missing").getString(it) == key }) continue
          var download = prefs.getLong(key, -1)
          if (download < 0) {
            download = manager.enqueue(DownloadManager.Request(Uri.parse(component.getString("url")))
                .setTitle("retype 基础词库").setDescription(if (key == "base") "中文词库" else "英文词库")
                .setAllowedOverMetered(!inputData.getBoolean("automatic", false))
                .setNotificationVisibility(DownloadManager.Request.VISIBILITY_VISIBLE))
            prefs.edit().putLong(key, download).commit()
          }
          while (true) {
            currentCoroutineContext().ensureActive()
            val finished = manager.query(DownloadManager.Query().setFilterById(download)).use { cursor ->
              if (!cursor.moveToFirst()) { prefs.edit().remove(key).commit(); error("下载任务不存在，请重试") }
              val status = cursor.getInt(cursor.getColumnIndexOrThrow(DownloadManager.COLUMN_STATUS))
              if (status == DownloadManager.STATUS_FAILED) { manager.remove(download); prefs.edit().remove(key).commit(); error("词库下载失败，请检查网络后重试") }
              val received = cursor.getLong(cursor.getColumnIndexOrThrow(DownloadManager.COLUMN_BYTES_DOWNLOADED_SO_FAR))
              setProgress(workDataOf("progress" to ((index + received.toFloat() / component.getLong("compressedBytes")) / components.length()).coerceIn(0f, 1f), "message" to "下载${if (key == "base") "中文" else "英文"}词库…"))
              status == DownloadManager.STATUS_SUCCESSFUL
            }
            if (finished) break
            delay(350)
          }
          try {
            manager.openDownloadedFile(download).use { descriptor ->
              android.os.ParcelFileDescriptor.AutoCloseInputStream(descriptor).use { input ->
                temporary.outputStream().use { output ->
                  val bytes = component.getLong("compressedBytes")
                  var total = 0L
                  val buffer = ByteArray(65536)
                  while (true) {
                    currentCoroutineContext().ensureActive()
                    val n = input.read(buffer)
                    if (n < 0) break
                    total += n
                    check(total <= bytes) { "词库下载超出大小限制" }
                    output.write(buffer, 0, n)
                  }
                  check(total == bytes) { "词库下载不完整" }
                  output.fd.sync()
                }
              }
            }
            setProgress(workDataOf("message" to "校验并安装词库…"))
            BaseResources.install(context, "resourceInstall", temporary, key)
          } finally {
            manager.remove(download)
            prefs.edit().remove(key).commit()
          }
        }
      }
      BaseResources.refresh(context)
      Result.success()
    } catch (e: CancellationException) { throw e }
    catch (e: Exception) { Result.failure(workDataOf("error" to (e.message ?: "基础词库安装失败"))) }
    finally { temporary.delete() }
  }
}
