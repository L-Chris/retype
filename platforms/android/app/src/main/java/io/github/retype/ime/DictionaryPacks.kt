package io.github.retype.ime

import android.content.Context
import java.io.File
import java.net.HttpURLConnection
import java.net.URL
import java.util.UUID
import kotlinx.coroutines.*
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import org.json.JSONArray
import org.json.JSONObject

object DictionaryPacks {
    private val mutex = Mutex()

    fun catalog(context: Context): List<JSONObject> {
        val array =
            JSONArray(
                context.assets.open("dictionary-packs.json").bufferedReader().use { it.readText() }
            )
        return List(array.length()) { array.getJSONObject(it) }
    }

    private fun installed(context: Context, pack: JSONObject): File? {
        val root = File(context.filesDir, "packs")
        val id = pack.getString("id")
        val name = File(root, "$id.current").takeIf { it.isFile }?.readText()?.trim() ?: return null
        if (!name.matches(Regex("$id-[a-f0-9]{64}\\.bin"))) return null
        return File(root, name).takeIf { it.isFile && !it.canWrite() }
    }

    fun paths(context: Context): String {
        val mask = AppStore(context).prefs.getInt("packs", 0)
        return JSONArray(
                catalog(context).mapIndexedNotNull { index, pack ->
                    if (mask and (1 shl index) != 0) installed(context, pack)?.absolutePath
                    else null
                }
            )
            .toString()
    }

    suspend fun set(
        context: Context,
        index: Int,
        enabled: Boolean,
        progress: (Float) -> Unit = {},
    ) =
        withContext(Dispatchers.IO) {
            mutex.withLock {
                val catalog = catalog(context)
                require(index in catalog.indices)
                if (enabled && installed(context, catalog[index]) == null)
                    download(context, catalog[index], progress)
                ensureActive()
                val prefs = AppStore(context).prefs
                val mask = prefs.getInt("packs", 0)
                check(
                    prefs
                        .edit()
                        .putInt(
                            "packs",
                            if (enabled) mask or (1 shl index) else mask and (1 shl index).inv(),
                        )
                        .commit()
                )
            }
        }

    suspend fun ensureEnabled(context: Context) {
        val mask = AppStore(context).prefs.getInt("packs", 0)
        catalog(context).forEachIndexed { i, _ ->
            if (mask and (1 shl i) != 0) set(context, i, true)
        }
    }

    private suspend fun download(context: Context, pack: JSONObject, progress: (Float) -> Unit) {
        val id = pack.getString("id")
        require(id.matches(Regex("[a-z]+")))
        val root = File(context.filesDir, "packs").apply { mkdirs() }
        val nonce = UUID.randomUUID().toString()
        val raw = File(root, "$id-$nonce.yaml")
        val binary = File(root, "$id-$nonce.tmp")
        val connection =
            URL(
                    "https://raw.githubusercontent.com/amzxyz/rime-wanxiang/v18.0.14/dicts/$id.dict.yaml"
                )
                .openConnection() as HttpURLConnection
        connection.instanceFollowRedirects = false
        connection.connectTimeout = 20000
        connection.readTimeout = 30000
        try {
            check(connection.responseCode == 200) { "词库下载失败（HTTP ${connection.responseCode}）" }
            var count = 0L
            val expected = pack.getLong("bytes")
            require(expected in 1..8 * 1024 * 1024)
            connection.inputStream.use { input ->
                raw.outputStream().use { out ->
                    val buffer = ByteArray(32768)
                    while (true) {
                        currentCoroutineContext().ensureActive()
                        val n = input.read(buffer)
                        if (n < 0) break
                        count += n
                        check(count <= expected) { "词库超过大小限制" }
                        out.write(buffer, 0, n)
                        progress(count.toFloat() / expected)
                    }
                    out.fd.sync()
                }
            }
            check(count == expected && sha256(raw.readBytes()) == pack.getString("sha256")) {
                "词库校验失败"
            }
            NativeBridge.feature(
                JSONObject()
                    .put("type", "compilePack")
                    .put("source", raw.absolutePath)
                    .put("destination", binary.absolutePath)
                    .toString()
            )
            currentCoroutineContext().ensureActive()
            val final = File(root, "$id-${sha256(binary.readBytes())}.bin")
            check(binary.setReadOnly() && binary.renameTo(final)) { "无法安装词库" }
            val marker = android.util.AtomicFile(File(root, "$id.current"))
            val out = marker.startWrite()
            try {
                out.write(final.name.toByteArray())
                marker.finishWrite(out)
            } catch (e: Exception) {
                marker.failWrite(out)
                throw e
            }
        } finally {
            connection.disconnect()
            raw.delete()
            binary.delete()
        }
    }
}
