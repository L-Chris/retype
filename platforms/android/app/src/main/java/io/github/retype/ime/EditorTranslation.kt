package io.github.retype.ime

import android.view.inputmethod.ExtractedTextRequest
import android.view.inputmethod.InputConnection

data class EditorText(val text: String, val selectionStart: Int, val selectionEnd: Int)

object EditorTranslation {
    fun read(connection: InputConnection): EditorText {
        val request =
            ExtractedTextRequest().apply {
                hintMaxChars = 65537
                hintMaxLines = 10000
            }
        val extracted = connection.getExtractedText(request, 0) ?: error("当前应用不允许读取完整文字")
        val text = extracted.text?.toString() ?: error("当前应用不允许读取文字")
        check(
            extracted.startOffset == 0 &&
                extracted.partialStartOffset < 0 &&
                extracted.partialEndOffset < 0
        ) {
            "当前应用只允许读取部分文字"
        }
        check(text.length <= 65536) { "输入框文字过长" }
        check(text.isNotBlank()) { "输入框为空" }
        check(
            extracted.selectionStart in 0..text.length && extracted.selectionEnd in 0..text.length
        ) {
            "当前应用返回的选区无效"
        }
        return EditorText(text, extracted.selectionStart, extracted.selectionEnd)
    }

    fun replace(connection: InputConnection, source: EditorText, translated: String): Boolean {
        if (runCatching { read(connection) }.getOrNull() != source) return false
        connection.beginBatchEdit()
        try {
            if (!connection.setSelection(0, source.text.length)) return false
            if (!connection.commitText(translated, 1)) {
                connection.setSelection(source.selectionStart, source.selectionEnd)
                return false
            }
            return true
        } finally {
            connection.endBatchEdit()
        }
    }
}
