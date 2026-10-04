package io.github.retype.ime

import android.content.Context
import java.io.File
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext

object DictionaryAssets {
    private val mutex = Mutex()

    suspend fun prepare(context: Context): File =
        withContext(Dispatchers.IO) {
            mutex.withLock {
                val file = File(context.filesDir, "dict-${BuildConfig.VERSION_NAME}.bin")
                if (!file.isFile) {
                    val staging = File(context.filesDir, "dict-${BuildConfig.VERSION_NAME}.tmp")
                    check(!staging.exists() || staging.delete()) { "无法清理未完成的词库安装" }
                    context.assets.open("retype-dict.bin").use { source ->
                        staging.outputStream().use { output ->
                            source.copyTo(output)
                            output.fd.sync()
                        }
                    }
                    check(staging.setReadOnly()) { "无法设置词库只读权限" }
                    check(staging.renameTo(file)) { "无法完成词库安装" }
                    context.filesDir
                        .listFiles()
                        ?.filter {
                            it.name.startsWith("dict-") && it.name.endsWith(".bin") && it != file
                        }
                        ?.forEach { it.delete() }
                }
                file
            }
        }
}
