package io.github.retype.ime

import android.text.InputType
import android.view.inputmethod.EditorInfo
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.io.File
import kotlinx.coroutines.runBlocking
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class NativeSessionTest {
    private val context = InstrumentationRegistry.getInstrumentation().targetContext

    private fun open(
        flypy: Boolean = false,
        chinese: Boolean = true,
        learn: Boolean = false,
    ): Long {
        val dictionary = runBlocking { DictionaryAssets.prepare(context) }
        return NativeBridge.create(
            dictionary.absolutePath,
            File(context.cacheDir, "test-learning.db").absolutePath,
            flypy,
            chinese,
            learn,
        )
    }

    private fun send(handle: Long, type: String, value: String = ""): JSONObject =
        JSONObject(
            NativeBridge.dispatch(
                handle,
                JSONObject().put("type", type).put("value", value).toString(),
            )
        )

    private fun input(handle: Long, text: String): JSONObject {
        var result = JSONObject()
        text.forEach { result = send(handle, "key", it.toString()) }
        return result
    }

    @Test
    fun nativeDictionaryAndBothSchemes() {
        for ((flypy, codes) in
            listOf(
                false to listOf("nihao" to "你好"),
                true to listOf("nihc" to "你好", "wo" to "我", "wou" to "我是", "woui" to "我是", "edu" to "额度"),
            )) {
            val handle = open(flypy)
            try {
                for ((code, word) in codes) {
                    send(handle, "reset")
                    val update = input(handle, code)
                    val candidates = update.getJSONArray("candidates")
                    assertEquals("Every key must reach the native composition", code, update.getString("composition"))
                    if (flypy && code in listOf("wou", "woui"))
                        assertEquals("Phrase must lead the candidates", "我是", candidates.getString(0))
                    val index =
                        (0 until candidates.length()).firstOrNull {
                            candidates.getString(it) == word
                        }
                    assertNotNull("$code must produce $word", index)
                    val chosen =
                        JSONObject(
                            NativeBridge.dispatch(
                                handle,
                                JSONObject()
                                    .put("type", "choose")
                                    .put("index", index)
                                    .put("generation", update.getLong("generation"))
                                    .toString(),
                            )
                        )
                    assertEquals(word, chosen.getJSONArray("commits").getString(0))
                }
            } finally {
                NativeBridge.destroy(handle)
            }
        }
    }

    @Test
    fun englishLearningPersistsAndPrivateSessionDoesNotLoadIt() {
        val handle = open(chinese = false, learn = true)
        try {
            repeat(2) {
                input(handle, "retypemobileword")
                val committed = send(handle, "key", "space")
                NativeBridge.dispatch(
                    handle,
                    JSONObject()
                        .put("type", "ack")
                        .put("generation", committed.getLong("generation"))
                        .put("accepted", true)
                        .toString(),
                )
            }
        } finally {
            NativeBridge.destroy(handle)
        }
        for (learn in listOf(true, false)) {
            val reopened = open(chinese = false, learn = learn)
            try {
                val candidates = input(reopened, "retypemobilewo").getJSONArray("candidates")
                assertEquals(
                    learn,
                    (0 until candidates.length()).any {
                        candidates.getString(it) == "retypemobileword"
                    },
                )
            } finally {
                NativeBridge.destroy(reopened)
            }
        }
    }

    @Test
    fun sensitiveEditorPolicy() {
        for (variation in
            listOf(
                InputType.TYPE_TEXT_VARIATION_PASSWORD,
                InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD,
                InputType.TYPE_TEXT_VARIATION_WEB_PASSWORD,
            )) {
            val info = EditorInfo().apply { inputType = InputType.TYPE_CLASS_TEXT or variation }
            val policy = EditorPolicy.from(info)
            assertTrue(policy.password)
            assertTrue(policy.literal)
            assertFalse(policy.learning)
        }
        val private =
            EditorInfo().apply {
                inputType = InputType.TYPE_CLASS_TEXT
                imeOptions = EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING
            }
        assertFalse(EditorPolicy.from(private).learning)
        val email =
            EditorInfo().apply {
                inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_EMAIL_ADDRESS
            }
        assertTrue(EditorPolicy.from(email).ascii)
    }
}
