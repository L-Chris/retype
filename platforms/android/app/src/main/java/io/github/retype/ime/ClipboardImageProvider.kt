package io.github.retype.ime

import android.content.ContentProvider
import android.content.ContentValues
import android.database.Cursor
import android.database.MatrixCursor
import android.net.Uri
import android.os.ParcelFileDescriptor
import android.provider.OpenableColumns
import java.io.FileNotFoundException
import java.util.UUID

/** One original image in memory; URI grants come from the system clipboard, never broad storage access. */
class ClipboardImageProvider : ContentProvider() {
  companion object {
    private data class Image(val uri: Uri, val bytes: ByteArray, val mime: String, val name: String)
    private var current: Image? = null
    @Synchronized internal fun publish(context: android.content.Context, bytes: ByteArray, mime: String = "image/png"): Uri {
      val extension = when (mime) { "image/jpeg" -> "jpg"; "image/webp" -> "webp"; "image/gif" -> "gif"; "image/bmp" -> "bmp"; "image/tiff" -> "tiff"; else -> "png" }
      val name = "retype-image.$extension"
      val uri = Uri.parse("content://${context.packageName}.clipboard/${UUID.randomUUID()}/$name")
      current = Image(uri, bytes, mime, name)
      return uri
    }
    @Synchronized private fun image(uri: Uri): Image? = current?.takeIf { it.uri == uri }
  }
  override fun onCreate() = true
  override fun getType(uri: Uri) = image(uri)?.mime

  override fun query(uri: Uri, projection: Array<out String>?, selection: String?, selectionArgs: Array<out String>?, sortOrder: String?): Cursor? {
    val data = image(uri) ?: return null
    val columns = projection ?: arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE)
    return MatrixCursor(columns).apply {
      addRow(columns.map { when (it) { OpenableColumns.DISPLAY_NAME -> data.name; OpenableColumns.SIZE -> data.bytes.size; else -> null } })
    }
  }
  override fun openFile(uri: Uri, mode: String): ParcelFileDescriptor {
    if (mode != "r") throw FileNotFoundException("Read only")
    val data = image(uri) ?: throw FileNotFoundException("Clipboard image expired")
    return openPipeHelper(uri, data.mime, null, data.bytes) { pipe, _, _, _, bytes ->
      runCatching { ParcelFileDescriptor.AutoCloseOutputStream(pipe).use { it.write(bytes) } }
    }
  }
  override fun insert(uri: Uri, values: ContentValues?): Uri? = throw UnsupportedOperationException()
  override fun update(uri: Uri, values: ContentValues?, selection: String?, selectionArgs: Array<out String>?) = 0
  override fun delete(uri: Uri, selection: String?, selectionArgs: Array<out String>?) = 0
}
