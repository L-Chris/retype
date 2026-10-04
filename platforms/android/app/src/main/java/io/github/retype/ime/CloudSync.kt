package io.github.retype.ime

import android.content.Context
import androidx.work.*
import java.io.File
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.*
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import org.json.JSONObject

object CloudSync {
    private val mutex = Mutex()

    fun schedule(context: Context) {
        val manager = WorkManager.getInstance(context)
        if (!AppStore(context).cloud().optBoolean("enabled")) {
            manager.cancelUniqueWork("retype-sync")
            return
        }
        val request =
            PeriodicWorkRequestBuilder<SyncWorker>(1, TimeUnit.HOURS)
                .setConstraints(
                    Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build()
                )
                .build()
        manager.enqueueUniquePeriodicWork("retype-sync", ExistingPeriodicWorkPolicy.KEEP, request)
    }

    suspend fun run(
        context: Context,
        confirm: Boolean = false,
        resolve: JSONObject? = null,
        test: Boolean = false,
    ): JSONObject =
        withContext(Dispatchers.IO) {
            mutex.withLock {
                val store = AppStore(context)
                val cfg = store.cloud()
                val base = store.values()
                val cfgString = cfg.toString()
                val password = SecretVault(context).get("cloud:${store.cloudAccount(cfg)}")
                val operation =
                    JSONObject()
                        .put("type", if (test) "syncTest" else "sync")
                        .put("config", cfg)
                        .put("password", password)
                if (!test) {
                    val dictionary = DictionaryAssets.prepare(context)
                    operation
                        .put("root", File(context.filesDir, "sync").absolutePath)
                        .put("dictionary", dictionary.absolutePath)
                        .put("database", File(context.filesDir, "learning.db").absolutePath)
                        .put("values", base)
                        .put("statistics", TypingStatistics.get(context).export())
                        .put("confirm", confirm)
                    if (resolve != null)
                        operation
                            .put("resolveKey", resolve.getString("key"))
                            .put("resolveStamp", resolve.getJSONObject("stamp"))
                            .put("useRemote", resolve.getBoolean("remote"))
                }
                val response = JSONObject(NativeBridge.feature(operation.toString()))
                check(store.cloud().toString() == cfgString) { "云盘配置已变化，本次未应用同步结果" }
                // A connection test must not replace the last sync/merge status.
                if (test) return@withLock response
                if (response.has("values")) {
                    check(store.values().toString() == base.toString()) { "本机设置已变化，本次未覆盖，请重新同步" }
                    store.applyValues(response.getJSONObject("values"))
                    TypingStatistics.get(context).merge(response.getJSONArray("statistics"))
                }
                store.prefs
                    .edit()
                    .putString("syncStatus", response.toString())
                    .putLong("lastSync", System.currentTimeMillis())
                    .commit()
                if (!test && response.has("values")) DictionaryPacks.ensureEnabled(context)
                response
            }
        }
}

class SyncWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {
    override suspend fun doWork(): Result {
        if (!AppStore(applicationContext).cloud().optBoolean("enabled")) return Result.success()
        return try {
            CloudSync.run(applicationContext)
            Result.success()
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            // Messages never contain input text or credentials.
            AppStore(applicationContext)
                .prefs
                .edit()
                .putString(
                    "syncStatus",
                    JSONObject().put("message", e.message ?: "同步失败").toString(),
                )
                .commit()
            if (runAttemptCount < 3) Result.retry() else Result.failure()
        }
    }
}
