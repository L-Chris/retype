package io.github.retype.ime

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.io.File
import java.net.InetAddress
import java.net.ServerSocket
import kotlinx.coroutines.runBlocking
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class CloudRoundtripTest {
    @Test
    fun sharedWebdavProtocolFirstMergeAndConcurrentSettingConflict() = runBlocking {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val root = File(context.cacheDir, "sync-test-${System.nanoTime()}").apply { mkdirs() }
        val socket = ServerSocket(0, 4, InetAddress.getByName("127.0.0.1"))
        val objects = mutableMapOf<String, ByteArray>()
        val paths = mutableListOf<String>()
        var running = true
        val server =
            Thread {
                    while (running) try {
                        socket.accept().use { s ->
                            s.soTimeout = 5000
                            val input = s.getInputStream()
                            val h = StringBuilder()
                            while (!h.endsWith("\r\n\r\n")) {
                                val c = input.read()
                                if (c < 0) error("Disconnected")
                                h.append(c.toChar())
                                check(h.length < 16384)
                            }
                            val parts = h.toString().lineSequence().first().split(' ')
                            val method = parts[0]
                            val path = parts[1]
                            paths.add(path)
                            val length =
                                Regex("(?i)content-length: (\\d+)")
                                    .find(h)
                                    ?.groupValues
                                    ?.get(1)
                                    ?.toInt() ?: 0
                            val payload = ByteArray(length)
                            var at = 0
                            while (at < length) {
                                val n = input.read(payload, at, length - at)
                                check(n > 0)
                                at += n
                            }
                            var code = 200
                            var body = byteArrayOf()
                            when (method) {
                                "MKCOL" -> code = 201
                                "GET" -> {
                                    body = objects[path] ?: byteArrayOf()
                                    if (!objects.containsKey(path)) code = 404
                                }
                                "PUT" -> {
                                    objects[path] = payload
                                    code = 201
                                }
                                "DELETE" -> {
                                    objects.remove(path)
                                    code = 204
                                }
                                "PROPFIND" -> {
                                    code = 207
                                    body =
                                        ("<d:multistatus xmlns:d=\"DAV:\">" +
                                                objects.keys
                                                    .filter { it.contains("/devices/") }
                                                    .joinToString("") {
                                                        "<d:response><d:href>$it</d:href></d:response>"
                                                    } +
                                                "</d:multistatus>")
                                            .toByteArray()
                                }
                                else -> code = 405
                            }
                            val out = s.getOutputStream()
                            out.write(
                                "HTTP/1.1 $code Status\r\nContent-Length: ${body.size}\r\nConnection: close\r\n\r\n"
                                    .toByteArray()
                            )
                            out.write(body)
                            out.flush()
                        }
                    } catch (e: Exception) {
                        if (running) throw e
                    }
                }
                .apply { start() }
        try {
            val dict = DictionaryAssets.prepare(context)
            val a = "a".repeat(32)
            val b = "b".repeat(32)
            fun operation(id: String, scheme: Int, confirm: Boolean = false): JSONObject {
                val cfg =
                    JSONObject()
                        .put("version", 1)
                        .put("provider", "cstcloud")
                        .put("enabled", true)
                        .put("url", "http://127.0.0.1:${socket.localPort}/dav/")
                        .put("username", "test")
                        .put("device_id", id)
                        .put("device_name", id.take(1))
                val values = AppStore(context).values().put("input.scheme", scheme)
                return JSONObject()
                    .put("type", "sync")
                    .put("config", cfg)
                    .put("password", "fixture-password")
                    .put("root", File(root, id).absolutePath)
                    .put("dictionary", dict.absolutePath)
                    .put("database", File(root, "$id.db").absolutePath)
                    .put("values", values)
                    .put("statistics", JSONArray())
                    .put("confirm", confirm)
            }
            fun send(o: JSONObject) = JSONObject(NativeBridge.feature(o.toString()))
            assertFalse(send(operation(a, 0)).getBoolean("awaitingMerge"))
            assertTrue(send(operation(b, 1)).getBoolean("awaitingMerge"))
            assertEquals(
                0,
                send(operation(b, 1, true)).getJSONObject("values").getInt("input.scheme"),
            )
            send(operation(a, 1))
            // B makes a different local edit to the same field, concurrent with A.
            val edited = operation(b, 0)
            edited.getJSONObject("values").put("translation.target", "日本語")
            send(edited)
            val changedA = operation(a, 1)
            changedA.getJSONObject("values").put("translation.target", "Deutsch")
            val conflict = send(changedA).getJSONArray("conflicts")
            assertTrue(conflict.length() > 0)
            val c =
                (0 until conflict.length())
                    .map { conflict.getJSONObject(it) }
                    .first { it.getString("key") == "translation.target" }
            changedA
                .put("resolveKey", c.getString("key"))
                .put("resolveStamp", c.getJSONObject("remote").getJSONObject("stamp"))
                .put("useRemote", true)
            val resolved = send(changedA)
            assertEquals("日本語", resolved.getJSONObject("values").getString("translation.target"))
            assertEquals(0, resolved.getJSONArray("conflicts").length())
            assertTrue(
                paths
                    .filter {
                        (it.contains("/objects/") || it.contains("/devices/")) && !it.endsWith('/')
                    }
                    .all { it.endsWith(".json.prop") }
            )
            assertFalse(
                objects.values.any { it.toString(Charsets.UTF_8).contains("fixture-password") }
            )
        } finally {
            running = false
            socket.close()
            server.join(1000)
            root.deleteRecursively()
        }
    }
}
