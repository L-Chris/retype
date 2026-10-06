package io.github.retype.ime

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.io.File
import java.net.InetAddress
import java.net.ServerSocket
import java.time.ZonedDateTime
import kotlinx.coroutines.runBlocking
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class MobileFeaturesTest {
    private val context = InstrumentationRegistry.getInstrumentation().targetContext

    @Test
    fun verifiedOptionalDictionaryDownload() = runBlocking {
        val prefs = AppStore(context).prefs
        val old = prefs.getInt("packs", 0)
        try {
            DictionaryPacks.set(context, 0, true)
            val paths = JSONArray(DictionaryPacks.paths(context))
            assertTrue(paths.length() > 0)
            assertTrue(
                (0 until paths.length()).any {
                    File(paths.getString(it)).name.startsWith("mingren-")
                }
            )
            DictionaryPacks.set(context, 0, false)
            assertFalse(DictionaryPacks.paths(context).contains("mingren-"))
        } finally {
            prefs.edit().putInt("packs", old).commit()
        }
    }

    @Test
    fun encryptedSecretsAndPortableSettingsExcludeCredentials() {
        val vault = SecretVault(context)
        val id = "test-a3-secret"
        val secret = "test-key-DO-NOT-SYNC"
        try {
            vault.save(id, secret)
            assertEquals(secret, vault.get(id))
            val raw = context.getSharedPreferences("secrets", 0).getString(id, "")!!
            assertFalse(raw.contains(secret))
            assertFalse(AppStore(context).values().toString().contains(secret))
            assertFalse(AppStore(context).values().has("cloud.password"))
        } finally {
            vault.save(id, "")
        }
        val b =
            JSONObject("""{"mode":{"vk":16,"modifiers":0},"translate":{"vk":48,"modifiers":3}}""")
        Shortcuts.validate(b)
        assertEquals("Ctrl + Alt + 0", Shortcuts.label(b.getJSONObject("translate")))
        b.put("translate", b.getJSONObject("mode"))
        assertTrue(runCatching { Shortcuts.validate(b) }.isFailure)
    }

    @Test
    fun idleClockAndStatisticsMergeAreIdempotent() {
        val clock = ActivityClock()
        clock.tick(true, 1000)
        clock.tick(true, 2000)
        clock.tick(true, 19000)
        assertEquals(Counts(chinese = 20, chineseMs = 1000), clock.commit("字".repeat(20), true))
        clock.tick(false, 20000)
        assertEquals(Counts(english = 10), clock.commit("a".repeat(10), false))
        val db = TypingStatistics.get(context)
        val device = "f".repeat(32)
        val minute = ZonedDateTime.parse("2020-01-02T12:00:00Z").toEpochSecond() / 60
        val rows =
            JSONArray()
                .put(
                    JSONObject()
                        .put("device", device)
                        .put("stream", "android.log")
                        .put("minute", minute)
                        .put(
                            "counts",
                            JSONObject()
                                .put("chinese", 20)
                                .put("english", 10)
                                .put("chinese_ms", 10000)
                                .put("english_ms", 10000),
                        )
                )
        try {
            // A synced desktop bucket at the same minute must remain exportable,
            // without changing mobile counts, speeds or chart bars.
            rows.put(
                JSONObject(rows.getJSONObject(0).toString())
                    .put("stream", "test-desktop.log")
                    .put("counts", JSONObject()
                        .put("chinese", 2000).put("english", 1000)
                        .put("chinese_ms", 10000).put("english_ms", 10000))
            )
            db.merge(rows)
            db.merge(rows)
            val now = ZonedDateTime.parse("2020-01-02T12:02:00Z")
            for (period in 0..3) {
                val summary = db.summary(period, now)
                assertEquals(20L, summary.current.chinese)
                assertEquals(10L, summary.current.english)
                assertEquals(120L, summary.current.speed(true))
                assertEquals(20L, summary.bars.sumOf { it.second.chinese })
            }
            assertEquals(Counts(20, 10, 10000, 10000), db.recent(now))
            val exported = db.export()
            assertEquals(2, (0 until exported.length()).count {
                exported.getJSONObject(it).getString("device") == device
            })
        } finally {
            db.writableDatabase.delete(
                "buckets",
                "device=? AND stream IN ('android.log','test-desktop.log')",
                arrayOf(device),
            )
        }
    }

    @Test
    fun realNativeHttpModelsAndTranslationCarryCredentialsAndReasoning() {
        val socket = ServerSocket(0, 4, InetAddress.getByName("127.0.0.1"))
        socket.soTimeout = 10000
        val requests = mutableListOf<String>()
        val server =
            Thread {
                    repeat(2) { i ->
                        socket.accept().use { s ->
                            s.soTimeout = 10000
                            val input = s.getInputStream()
                            val header = StringBuilder()
                            while (!header.endsWith("\r\n\r\n")) {
                                val c = input.read()
                                if (c < 0) error("Disconnected")
                                header.append(c.toChar())
                                check(header.length < 16384)
                            }
                            val length =
                                Regex("(?i)content-length: (\\d+)")
                                    .find(header)
                                    ?.groupValues
                                    ?.get(1)
                                    ?.toInt() ?: 0
                            val bytes = ByteArray(length)
                            var at = 0
                            while (at < length) {
                                val n = input.read(bytes, at, length - at)
                                check(n > 0)
                                at += n
                            }
                            synchronized(requests) {
                                requests.add(header.toString() + bytes.toString(Charsets.UTF_8))
                            }
                            val body =
                                if (i == 0) """{"data":[{"id":"test-model"}]}"""
                                else
                                    """{"choices":[{"finish_reason":"stop","message":{"content":"你好"}}]}"""
                            val out = s.getOutputStream()
                            val payload = body.toByteArray()
                            out.write(
                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: ${payload.size}\r\nConnection: close\r\n\r\n"
                                    .toByteArray()
                            )
                            out.write(payload)
                            out.flush()
                        }
                    }
                }
                .apply { start() }
        try {
            val provider =
                JSONObject()
                    .put("id", "test-provider")
                    .put("name", "Test")
                    .put("preset", "自定义")
                    .put("kind", "Compatible")
                    .put("base_url", "http://127.0.0.1:${socket.localPort}/v1")
                    .put("models", JSONArray().put("test-model"))
            val models =
                JSONArray(
                    NativeBridge.feature(
                        JSONObject()
                            .put("type", "models")
                            .put("provider", provider)
                            .put("key", "test-token")
                            .toString()
                    )
                )
            assertEquals("test-model", models.getString(0))
            val config =
                JSONObject()
                    .put("providers", JSONArray().put(provider))
                    .put("provider", "test-provider")
                    .put("model", "test-model")
                    .put("target", "简体中文")
                    .put("reasoning", "none")
            val result =
                NativeBridge.feature(
                    JSONObject()
                        .put("type", "translate")
                        .put("config", config)
                        .put("key", "test-token")
                        .put("text", "Hello")
                        .toString()
                )
            assertEquals("你好", org.json.JSONTokener(result).nextValue())
            server.join(10000)
            assertEquals(2, requests.size)
            assertTrue(requests.all { it.contains("Bearer test-token") })
            assertEquals(
                "none",
                JSONObject(requests[1].substringAfter("\r\n\r\n")).getString("reasoning_effort"),
            )
        } finally {
            socket.close()
            server.join(1000)
        }
    }

    @Test
    fun mobilePackCompilerUsesUpstreamReadings() = runBlocking {
        val raw = File(context.cacheDir, "a3-pack.yaml")
        val binary = File(context.cacheDir, "a3-pack.bin")
        try {
            raw.writeText(
                "---\nname: test\n...\n鲁迅乔布斯\tlǔ xùn qiáo bù sī\t100000\n乔布斯\tqiáo bù sī\t90000\n"
            )
            NativeBridge.feature(
                JSONObject()
                    .put("type", "compilePack")
                    .put("source", raw.absolutePath)
                    .put("destination", binary.absolutePath)
                    .toString()
            )
            assertTrue(binary.setReadOnly())
            val dict = DictionaryAssets.prepare(context)
            val handle =
                NativeBridge.create(
                    dict.absolutePath,
                    File(context.cacheDir, "test-a3-learning.db").absolutePath,
                    false,
                    true,
                    false,
                    JSONArray().put(binary.absolutePath).toString(),
                )
            try {
                var result = JSONObject()
                for (c in "luxunqiaobusi") result =
                    JSONObject(
                        NativeBridge.dispatch(
                            handle,
                            JSONObject().put("type", "key").put("value", c.toString()).toString(),
                        )
                    )
                assertTrue(
                    (0 until result.getJSONArray("candidates").length()).any {
                        result.getJSONArray("candidates").getString(it) == "鲁迅乔布斯"
                    }
                )
            } finally {
                NativeBridge.destroy(handle)
            }
        } finally {
            raw.delete()
            binary.delete()
        }
    }
}
