package io.github.retype.ime

import android.content.Context
import java.io.File

object DictionaryAssets {
    suspend fun available(context: Context): File? = BaseResources.refresh(context).base?.let(::File)
    suspend fun prepare(context: Context): File = available(context) ?: error("请先下载基础词库")
}
