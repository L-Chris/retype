package io.github.retype.ime

import android.content.Context
import android.content.Intent
import android.app.ActivityManager
import androidx.test.uiautomator.UiDevice
import androidx.test.platform.app.InstrumentationRegistry
import java.io.BufferedReader
import java.net.ServerSocket
import java.net.Socket
import java.security.SecureRandom
import java.security.cert.X509Certificate
import java.util.UUID
import javax.net.ssl.*
import kotlinx.coroutines.*
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test

/** Real TLS sockets against the phone listener, with isolated synthetic identities. */
class LanInboundTest {
  private val context = InstrumentationRegistry.getInstrumentation().targetContext
  private val lan get() = LanClipboard.get(context)
  private val store get() = LanPeerStore(context.getSharedPreferences("lanClipboard", Context.MODE_PRIVATE), SecretVault(context))
  private suspend fun waitFor(check: () -> Boolean) { withTimeout(15_000) { while (!check()) delay(25) } }
  private fun feature(value: JSONObject) = JSONObject(NativeBridge.feature(value.toString()))
  @Suppress("DEPRECATION")
  private fun foreground() = context.getSystemService(ActivityManager::class.java).getRunningServices(100)
      .any { it.service.className == LanSyncService::class.java.name && it.foreground }

  private class Client(val socket: SSLSocket) : AutoCloseable {
    val input: BufferedReader = socket.inputStream.bufferedReader()
    fun send(value: JSONObject) { socket.outputStream.write((value.toString() + "\n").toByteArray()); socket.outputStream.flush() }
    fun read(): JSONObject = JSONObject(input.readLine() ?: error("Connection closed"))
    override fun close() { socket.close() }
  }

  private fun client(protocol: String? = null): Client {
    val trust = object : X509TrustManager {
      override fun getAcceptedIssuers(): Array<X509Certificate> = emptyArray()
      override fun checkClientTrusted(chain: Array<X509Certificate>, auth: String) {}
      override fun checkServerTrusted(chain: Array<X509Certificate>, auth: String) {}
    }
    val tls = SSLContext.getInstance("TLS").apply { init(null, arrayOf(trust), SecureRandom()) }
    val socket = tls.socketFactory.createSocket("127.0.0.1", lan.state.value.port) as SSLSocket
    socket.soTimeout = 5000
    if (protocol != null) socket.enabledProtocols = arrayOf(protocol)
    socket.startHandshake()
    return Client(socket)
  }

  private fun hello(id: String, token: String = "", nonce: String? = null) =
      JSONObject().put("type", "hello").put("version", 2).put("id", id).put("name", "Synthetic desktop")
          .put("mode", "join").put("token", token).put("resume", nonce ?: JSONObject.NULL)
          .put("images", true).put("imageFormats", JSONArray().put("image/png").put("image/jpeg"))

  private fun pair(client: Client, id: String, code: String): String {
    client.send(hello(id))
    val request = client.read()
    assertEquals("pair", request.getString("type"))
    val begin = feature(JSONObject().put("type", "pairBegin").put("code", code).put("client", id)
        .put("server", request.getString("id")).put("certificate", LanClipboard.digest(client.socket.session.peerCertificates[0].encoded)))
    val proof = feature(JSONObject().put("type", "pairFinish").put("handle", begin.getString("handle"))
        .put("message", request.getString("message")))
    client.send(JSONObject().put("type", "proof").put("message", begin.getString("message")).put("proof", proof.getString("client")))
    val paired = client.read()
    assertEquals("paired", paired.getString("type"))
    assertEquals(proof.getString("server"), paired.getString("proof"))
    assertEquals("ready", client.read().getString("type"))
    return paired.getString("token")
  }

  private fun clip(id: String, text: String, seq: Int = 1) = JSONObject().put("type", "clip").put("clip",
      JSONObject().put("origin", id).put("seq", seq).put("clock", System.currentTimeMillis())
          .put("created", System.currentTimeMillis()).put("text", text))

  @Test fun desktopInitiatedPairingResumeRelayAndUnlink() = runBlocking {
    val oldPort = lan.state.value.port
    try {
      lan.unlinkAllForTest()
      assertEquals(50206, LanEndpoint.PORT)
      lan.setPort(LanEndpoint.PORT.toString())
      lan.showCode()
      waitFor { lan.state.value.listening }
      context.startActivity(Intent(context, SettingsActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
      lan.resume()
      waitFor { foreground() }
      UiDevice.getInstance(InstrumentationRegistry.getInstrumentation()).pressHome()
      assertEquals(LanEndpoint.PORT, lan.state.value.port)
      val a = UUID.randomUUID().toString().replace("-", "")
      val b = UUID.randomUUID().toString().replace("-", "")
      val firstCode = lan.state.value.code!!
      var token = ""
      var fingerprint = ""
      client().use { first ->
        fingerprint = LanClipboard.digest(first.socket.session.peerCertificates[0].encoded)
        token = pair(first, a, firstCode)
        waitFor { lan.state.value.peers.any { it.id == a && it.connection.isNotEmpty() } }
        assertEquals("", store.get(a)!!.pin) // No client certificate was presented.
        assertEquals(token, store.token(a))
        lan.showCode()
        client().use { second ->
          pair(second, b, lan.state.value.code!!)
          first.send(clip(a, "inbound-relay"))
          waitFor { lan.pasteText() == "inbound-relay" }
          val relayed = second.read()
          assertEquals("clip", relayed.getString("type"))
          assertEquals("inbound-relay", relayed.getJSONObject("clip").getString("text"))
          assertEquals(2, lan.state.value.peers.count { it.connection.isNotEmpty() })
          lan.unlink(b)
          waitFor { lan.state.value.peers.none { it.id == b } }
          assertTrue(lan.state.value.peers.any { it.id == a && it.connection.isNotEmpty() })
        }
      }
      waitFor { lan.state.value.peers.single().connection.isEmpty() }
      client().use { resumed ->
        assertEquals(fingerprint, LanClipboard.digest(resumed.socket.session.peerCertificates[0].encoded))
        val nonce = UUID.randomUUID().toString().replace("-", "")
        resumed.send(hello(a, nonce = nonce))
        val challenge = resumed.read()
        assertEquals("resume", challenge.getString("type"))
        val proof = feature(JSONObject().put("type", "pairResume").put("token", token).put("client", a)
            .put("server", challenge.getString("id")).put("certificate", fingerprint)
            .put("nonce", nonce).put("challenge", challenge.getString("challenge")))
        assertEquals(proof.getString("server"), challenge.getString("proof"))
        resumed.send(JSONObject().put("type", "resumeProof").put("proof", proof.getString("client")))
        assertEquals("ready", resumed.read().getString("type"))
        waitFor { lan.state.value.peers.single().connection.isNotEmpty() }
        assertEquals(token, store.token(a))
        lan.enable(false)
        assertFalse(lan.state.value.listening)
        waitFor { !foreground() }
        waitFor { runCatching { Socket("127.0.0.1", LanEndpoint.PORT).use {} }.isFailure }
      }
      lan.enable(true)
      waitFor { lan.state.value.listening }
      assertEquals(fingerprint, LanClipboard.digest(client("TLSv1.2").use { it.socket.session.peerCertificates[0].encoded }))
    } finally { lan.unlinkAllForTest(); lan.setPort(oldPort.toString()) }
  }

  @Test fun invalidPairingUnauthenticatedClipsAndPortConflictsCannotCreatePeers() = runBlocking {
    val oldPort = lan.state.value.port
    try {
      lan.unlinkAllForTest()
      lan.setPort(LanEndpoint.PORT.toString())
      lan.showCode()
      waitFor { lan.state.value.listening }
      client().use { unauthenticated ->
        unauthenticated.send(clip("a".repeat(32), "must-not-appear"))
        assertTrue(runCatching { unauthenticated.read() }.isFailure)
      }
      assertNull(lan.pasteText())
      assertTrue(store.all().isEmpty())
      client().use { wrong ->
        val badCode = if (lan.state.value.code == "000000") "111111" else "000000"
        assertTrue(runCatching { pair(wrong, "b".repeat(32), badCode) }.isFailure)
      }
      assertTrue(store.all().isEmpty())
      client().use { cancelled ->
        cancelled.send(hello("c".repeat(32)))
        assertEquals("pair", cancelled.read().getString("type"))
        lan.cancelPair()
        assertTrue(runCatching { cancelled.read() }.isFailure)
      }
      assertTrue(store.all().isEmpty())
      lan.join("123456")
      client().use { stale ->
        lan.cancelPair()
        lan.join("123456")
        stale.send(hello("d".repeat(32)).put("mode", "show"))
        assertTrue("A transport from a cancelled offer must not join a replacement using the same code", runCatching { stale.read() }.isFailure)
      }
      assertTrue(store.all().isEmpty())
      ServerSocket(0).use { occupied ->
        lan.setPort(occupied.localPort.toString())
        waitFor { lan.state.value.listenerError.contains("无法监听") }
        assertFalse(lan.state.value.listening)
      }
      assertTrue(lan.setPort(LanEndpoint.PORT.toString()))
      waitFor { lan.state.value.listening }
      assertFalse(lan.setPort("0"))
      assertEquals(LanEndpoint.PORT, lan.state.value.port)
    } finally { lan.unlinkAllForTest(); lan.setPort(oldPort.toString()) }
  }

  @Test fun rapidToggleCannotCancelAnUnacknowledgedForegroundStart() = runBlocking {
    val instrumentation = InstrumentationRegistry.getInstrumentation()
    try {
      context.startActivity(Intent(context, SettingsActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
      lan.unlinkAllForTest()
      instrumentation.runOnMainSync {
        repeat(10) { lan.enable(true); lan.enable(false) }
        lan.enable(true)
      }
      waitFor { lan.state.value.listening && foreground() }
      client().use { assertTrue(it.socket.session.isValid) }
      // The platform reports a missed foreground deadline asynchronously.
      delay(6000)
      assertTrue(lan.state.value.listening && foreground())
      lan.enable(false)
      waitFor { !foreground() }
    } finally { lan.unlinkAllForTest() }
  }
}
