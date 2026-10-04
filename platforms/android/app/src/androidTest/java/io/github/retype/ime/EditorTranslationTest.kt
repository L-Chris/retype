package io.github.retype.ime

import android.view.inputmethod.BaseInputConnection
import android.view.inputmethod.ExtractedText
import android.view.inputmethod.ExtractedTextRequest
import android.widget.EditText
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class EditorTranslationTest {
    @Test
    fun keyboardTranslationReplacesRealEditorAndNoticeExpires() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val device = androidx.test.uiautomator.UiDevice.getInstance(instrumentation)
        val store = AppStore(context)
        val original = store.ai()
        val originalShortcuts = store.prefs.getString("shortcuts", null)
        val oldIme = device.executeShellCommand("settings get secure default_input_method").trim()
        val ime = "io.github.retype.ime/.RetypeImeService"
        val wasEnabled =
            device.executeShellCommand("ime list -s").lineSequence().any { it.trim() == ime }
        val server =
            java.net.ServerSocket(0, 4, java.net.InetAddress.getByName("127.0.0.1")).apply {
                soTimeout = 15000
            }
        val thread =
            Thread {
                    try {
                        repeat(2) {
                            server.accept().use { s ->
                                s.soTimeout = 10000
                                val input = s.getInputStream()
                                val h = StringBuilder()
                                while (!h.endsWith("\r\n\r\n")) {
                                    val c = input.read()
                                    check(c >= 0)
                                    h.append(c.toChar())
                                }
                                val length =
                                    Regex("(?i)content-length: (\\d+)")
                                        .find(h)
                                        ?.groupValues
                                        ?.get(1)
                                        ?.toInt() ?: 0
                                var remaining = length
                                val buffer = ByteArray(4096)
                                while (remaining > 0) {
                                    val n = input.read(buffer, 0, minOf(remaining, buffer.size))
                                    check(n > 0)
                                    remaining -= n
                                }
                                val body =
                                    """{"choices":[{"finish_reason":"stop","message":{"content":"你好"}}]}"""
                                        .toByteArray()
                                val out = s.getOutputStream()
                                out.write(
                                    "HTTP/1.1 200 OK\r\nContent-Length: ${body.size}\r\nConnection: close\r\n\r\n"
                                        .toByteArray()
                                )
                                out.write(body)
                                out.flush()
                            }
                        }
                    } catch (e: java.net.SocketException) {} catch (
                        e: java.net.SocketTimeoutException) {}
                }
                .apply { start() }
        fun find(
            selector: androidx.test.uiautomator.BySelector,
            timeout: Long = 10000,
        ): androidx.test.uiautomator.UiObject2? {
            val until = android.os.SystemClock.uptimeMillis() + timeout
            do {
                if (android.os.Build.VERSION.SDK_INT >= 33)
                    instrumentation.uiAutomation.clearCache()
                device.findObject(selector)?.let {
                    return it
                }
                android.os.SystemClock.sleep(100)
            } while (android.os.SystemClock.uptimeMillis() < until)
            return null
        }
        try {
            val p =
                JSONObject(
                        """{"id":"test-translation","name":"Test","preset":"自定义","kind":"Compatible","models":["test-model"]}"""
                    )
                    .put("base_url", "http://127.0.0.1:${server.localPort}/v1")
            val ai =
                JSONObject(original.toString())
                    .put("providers", org.json.JSONArray().put(p))
                    .put("provider", "test-translation")
                    .put("model", "test-model")
                    .put("preview", false)
                    .put("reasoning", "none")
            store.saveAi(ai)
            store.prefs
                .edit()
                .putString(
                    "shortcuts",
                    """{"mode":{"vk":16,"modifiers":0},"translate":{"vk":48,"modifiers":3}}""",
                )
                .commit()
            device.executeShellCommand("ime enable $ime")
            device.executeShellCommand("ime set $ime")
            device.executeShellCommand("am start -W -n io.github.retype.ime/.EditorFixtureActivity")
            val editor = find(androidx.test.uiautomator.By.desc("editor-normal"))!!
            editor.click()
            editor.text = "Hello"
            find(androidx.test.uiautomator.By.desc("翻译"))!!.click()
            assertNotNull(
                "Translation must replace the real EditText",
                find(androidx.test.uiautomator.By.text("你好"), 15000),
            )
            assertNotNull(find(androidx.test.uiautomator.By.text("已翻译"), 2000))
            android.os.SystemClock.sleep(3500)
            if (android.os.Build.VERSION.SDK_INT >= 33) instrumentation.uiAutomation.clearCache()
            assertFalse(device.hasObject(androidx.test.uiautomator.By.text("已翻译")))
            find(androidx.test.uiautomator.By.desc("editor-normal"))!!.text = "Hello again"
            device.executeShellCommand("input keycombination 113 57 7")
            assertNotNull(
                "Ctrl+Alt+0 must trigger translation",
                find(androidx.test.uiautomator.By.text("你好"), 15000),
            )
        } finally {
            store.saveAi(original)
            store.prefs.edit().putString("shortcuts", originalShortcuts).commit()
            server.close()
            thread.join(1000)
            if (oldIme != "null" && oldIme.isNotBlank())
                device.executeShellCommand("ime set $oldIme")
            if (!wasEnabled) device.executeShellCommand("ime disable $ime")
        }
    }

    @Test
    fun changedEditorsAndRejectedWritesNeverOverwriteText() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        instrumentation.runOnMainSync {
            val view = EditText(instrumentation.targetContext)
            val connection =
                object : BaseInputConnection(view, true) {
                    var text = "Hello"
                    var start = 5
                    var end = 5
                    var commits = 0
                    var reject = false

                    override fun getExtractedText(request: ExtractedTextRequest?, flags: Int) =
                        ExtractedText().also {
                            it.text = text
                            it.startOffset = 0
                            it.partialStartOffset = -1
                            it.partialEndOffset = -1
                            it.selectionStart = start
                            it.selectionEnd = end
                        }

                    override fun setSelection(s: Int, e: Int): Boolean {
                        start = s
                        end = e
                        return true
                    }

                    override fun commitText(value: CharSequence?, position: Int): Boolean {
                        commits++
                        if (reject) return false
                        text = value.toString()
                        return true
                    }
                }
            val source = EditorTranslation.read(connection)
            connection.text = "New text"
            assertFalse(EditorTranslation.replace(connection, source, "你好"))
            assertEquals(0, connection.commits)
            connection.text = "Hello"
            connection.start = 0
            connection.end = 0
            assertFalse(EditorTranslation.replace(connection, source, "你好"))
            assertEquals(0, connection.commits)
            connection.start = 5
            connection.end = 5
            connection.reject = true
            assertFalse(EditorTranslation.replace(connection, source, "你好"))
            assertEquals("Hello", connection.text)
            assertEquals(5, connection.start)
            connection.reject = false
            assertTrue(EditorTranslation.replace(connection, source, "你好"))
            assertEquals("你好", connection.text)
        }
    }
}
