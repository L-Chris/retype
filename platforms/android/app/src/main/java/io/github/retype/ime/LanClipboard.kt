package io.github.retype.ime

import android.content.ClipData
import android.content.ClipDescription
import android.content.ClipboardManager
import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import java.io.ByteArrayOutputStream
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.SocketTimeoutException
import java.security.MessageDigest
import java.security.SecureRandom
import java.security.cert.X509Certificate
import java.util.UUID
import java.util.concurrent.atomic.AtomicReference
import javax.net.ssl.*
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import org.json.JSONObject

data class NearbyClipboard(
    val name: String,
    val address: String,
    val mode: String = "",
    val id: String = "",
)

data class LanState(
    val enabled: Boolean = false,
    val message: String = "已关闭",
    val name: String = "",
    val code: String? = null,
    val expires: Long = 0,
    val nearby: List<NearbyClipboard> = emptyList(),
    val text: String? = null,
)

/** One process-wide connection. The default IME owns clipboard access; no accessibility service. */
class LanClipboard private constructor(private val context: Context) {
  companion object {
    private const val TTL = 600_000L
    private const val MAX_TEXT = 65_536
    private const val MAX_FRAME = 524_288
    @Volatile private var instance: LanClipboard? = null

    fun get(context: Context): LanClipboard =
        synchronized(this) {
          instance ?: LanClipboard(context.applicationContext).also { instance = it }
        }

    internal fun digest(bytes: ByteArray): String =
        MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }

    internal fun code(cert: ByteArray, client: String, server: String): String =
        ((digest(cert + client.toByteArray() + server.toByteArray()).take(8).toLong(16) % 1_000_000)
            .toString()
            .padStart(6, '0'))
  }

  private val prefs = context.getSharedPreferences("lanClipboard", Context.MODE_PRIVATE)
  private val vault = SecretVault(context)
  private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
  private val main = Handler(Looper.getMainLooper())
  private val clipboard = context.getSystemService(ClipboardManager::class.java)
  private val mutable =
      MutableStateFlow(
          LanState(
              enabled = prefs.getBoolean("enabled", false),
              name = prefs.getString("name", "") ?: "",
          )
      )
  val state = mutable.asStateFlow()
  private var job: Job? = null
  @Volatile private var socket: SSLSocket? = null
  @Volatile private var pairingPin = ""
  @Volatile private var pairingMode = ""
  @Volatile private var pairingExpires = 0L
  private val tried = mutableSetOf<String>()
  private var pairingJob: Job? = null
  private val outgoing = AtomicReference<JSONObject?>(null)
  private val gate = Any()
  private var clock = 0L
  private var sequence = maxOf(prefs.getLong("seq", 0), System.currentTimeMillis())
  private var latest: JSONObject? = null
  private val seen = mutableMapOf<String, Long>()
  private var appliedText: String? = null
  private var discoveryStop: (() -> Unit)? = null
  private val id =
      prefs.getString("id", null)
          ?: UUID.randomUUID().toString().replace("-", "").also {
            prefs.edit().putString("id", it).commit()
          }

  init {
    clipboard.addPrimaryClipChangedListener { capture() }
    main.post { resume() }
  }

  private fun update(change: (LanState) -> LanState) =
      synchronized(mutable) { mutable.value = change(mutable.value) }

  fun resume() {
    if (prefs.getBoolean("enabled", false) && discoveryStop == null) discoveryStop = discover()
    if (pairingMode.isEmpty() && prefs.getBoolean("enabled", false) && job?.isActive != true) {
      val address = prefs.getString("address", "") ?: ""
      if (address.isNotEmpty()) connect(address, false)
      else update { it.copy(enabled = true, message = "请关联电脑") }
    }
  }

  fun enable(value: Boolean) {
    prefs.edit().putBoolean("enabled", value).commit()
    update {
      it.copy(enabled = value, message = if (value) "连接中…" else "已关闭", text = null, code = null)
    }
    if (value) resume()
    else {
      cancelPair(false)
      update { it.copy(message = "已关闭") }
      job?.cancel()
      socket?.close()
      job = null
      discoveryStop?.invoke()
      discoveryStop = null
      synchronized(gate) {
        latest = null
        outgoing.set(null)
      }
    }
  }

  fun unlink() {
    enable(false)
    vault.save("lan-token", "")
    prefs.edit().remove("address").remove("pin").remove("peer").remove("name").commit()
    update { it.copy(name = "") }
  }

  fun showCode() {
    startPairing("show", SecureRandom().nextInt(1_000_000).toString().padStart(6, '0'))
  }

  fun join(code: String) {
    require(code.matches(Regex("[0-9]{6}")))
    startPairing("join", code)
  }

  private fun startPairing(mode: String, code: String) {
    cancelPair(false)
    prefs.edit().putBoolean("enabled", true).commit()
    pairingMode = mode
    pairingPin = code
    pairingExpires = System.currentTimeMillis() + 120_000
    synchronized(tried) { tried.clear() }
    update {
      it.copy(
          enabled = true,
          code = if (mode == "show") code else null,
          expires = pairingExpires,
          message = if (mode == "show") "请在另一台设备输入此匹配码" else "正在查找并关联设备…",
      )
    }
    pairingJob = scope.launch {
      var lastDiscovery = 0L
      while (isActive && pairingMode.isNotEmpty()) {
        if (System.currentTimeMillis() >= pairingExpires) {
          cancelPair(false)
          update { it.copy(message = "匹配码已过期，请重试") }
          break
        }
        if (job?.isActive != true) {
          if (synchronized(tried) { tried.size >= 5 }) {
            cancelPair(false)
            update { it.copy(message = "尝试次数过多，请重新查看或输入匹配码") }
            break
          }
          val wanted = if (pairingMode == "show") "join" else "show"
          val peer =
              mutable.value.nearby.firstOrNull {
                it.mode == wanted && synchronized(tried) { it.address !in tried }
              }
          if (peer != null) {
            synchronized(tried) { tried.add(peer.address) }
            connect(peer.address, true)
          }
        }
        if (System.currentTimeMillis() - lastDiscovery > 4000) {
          discoveryStop?.invoke()
          discoveryStop = discover()
          lastDiscovery = System.currentTimeMillis()
        }
        delay(500)
      }
    }
    if (discoveryStop == null) discoveryStop = discover()
  }

  internal fun pairForTest(address: String, code: String, showing: Boolean = false) {
    startPairing(if (showing) "show" else "join", code)
    connect(address, true)
  }

  fun cancelPair(resumeConnection: Boolean = true) {
    pairingPin = ""
    pairingMode = ""
    pairingExpires = 0
    pairingJob?.cancel()
    pairingJob = null
    job?.cancel()
    socket?.close()
    job = null
    update { it.copy(code = null, expires = 0, message = "关联已取消") }
    if (resumeConnection) resume()
  }

  private fun capture() {
    if (!mutable.value.enabled) return
    val ime =
        Settings.Secure.getString(context.contentResolver, Settings.Secure.DEFAULT_INPUT_METHOD)
    if (ime?.startsWith("${context.packageName}/") != true) return
    try {
      val clip = clipboard.primaryClip ?: return
      if (clip.description.extras?.getBoolean("android.content.extra.IS_SENSITIVE", false) == true)
          return
      if (!clip.description.hasMimeType(ClipDescription.MIMETYPE_TEXT_PLAIN) || clip.itemCount == 0)
          return
      val text = clip.getItemAt(0).text?.toString() ?: return
      if (text.isEmpty() || text.contains('\u0000') || text.toByteArray().size > MAX_TEXT) return
      synchronized(gate) {
        if (appliedText == text && clip.description.label?.toString() == "retype") return
        sequence++
        clock = maxOf(clock, System.currentTimeMillis()) + 1
        val event =
            JSONObject()
                .put("origin", id)
                .put("seq", sequence)
                .put("clock", clock)
                .put("created", System.currentTimeMillis())
                .put("text", text)
        seen[id] = sequence
        latest = event
        outgoing.set(event)
        prefs.edit().putLong("seq", sequence).apply()
      }
      update { it.copy(text = null) }
    } catch (_: SecurityException) {
      /* Another IME is active. */
    }
  }

  private fun accept(event: JSONObject) {
    val now = System.currentTimeMillis()
    val origin = event.getString("origin")
    val seq = event.getLong("seq")
    val remoteClock = event.getLong("clock")
    val created = event.getLong("created")
    val text = event.getString("text")
    require(origin.matches(Regex("[a-fA-F0-9]{32}")) && seq > 0 && remoteClock > 0)
    require(text.isNotEmpty() && !text.contains('\u0000') && text.toByteArray().size <= MAX_TEXT)
    if (created > now + 60_000 || created + TTL < now) return
    synchronized(gate) {
      if (seq <= (seen[origin] ?: 0)) return
      if (!seen.containsKey(origin) && seen.size >= 32) return
      seen[origin] = seq
      clock = maxOf(clock, remoteClock)
      val old = latest
      if (
          old != null &&
              compareValuesBy(
                  event,
                  old,
                  { it.getLong("clock") },
                  { it.getString("origin") },
                  { it.getLong("seq") },
              ) <= 0
      )
          return
      latest = event
    }
    main.post {
      synchronized(gate) {
        if (!mutable.value.enabled || latest !== event) return@post
        appliedText = text
        try {
          clipboard.setPrimaryClip(ClipData.newPlainText("retype", text))
        } catch (_: SecurityException) {}
      }
      update { it.copy(text = text) }
    }
    scope.launch {
      delay(maxOf(0, created + TTL - System.currentTimeMillis()))
      synchronized(gate) { if (latest === event) update { it.copy(text = null) } }
    }
  }

  fun pasteText(): String? =
      synchronized(gate) {
        val event = latest ?: return null
        if (!mutable.value.enabled || event.getLong("created") + TTL < System.currentTimeMillis())
            null
        else event.getString("text")
      }

  private fun connect(address: String, pairing: Boolean) {
    job?.cancel()
    socket?.close()
    job = scope.launch {
      var initialPair = pairing
      var delayMs = 1000L
      while (isActive && prefs.getBoolean("enabled", false)) {
        try {
          session(
              if (initialPair) address else prefs.getString("address", address) ?: address,
              initialPair,
          )
          initialPair = false
          delayMs = 1000
        } catch (e: CancellationException) {
          throw e
        } catch (error: Exception) {
          currentCoroutineContext().ensureActive()
          if (
              initialPair &&
                  prefs.getString("address", "") == address &&
                  prefs.getString("pin", "").orEmpty().isNotEmpty()
          )
              initialPair = false
          update {
            it.copy(
                message = if (initialPair) "尚未找到匹配设备，请检查匹配码及两端连接" else "设备离线，正在重连…",
            )
          }
          if (initialPair) break
        }
        delay(delayMs)
        delayMs = minOf(30_000, delayMs * 2)
      }
    }
  }

  private suspend fun session(address: String, pairing: Boolean) {
    val separator = address.lastIndexOf(':')
    require(separator > 0)
    val host = address.substring(0, separator)
    val port = address.substring(separator + 1).toInt()
    require(port in 1..65535)
    val resolved = InetAddress.getByName(host)
    require(
        resolved.isSiteLocalAddress || resolved.isLinkLocalAddress || resolved.isLoopbackAddress
    ) {
      "仅支持局域网地址"
    }
    val expectedPin = if (pairing) "" else prefs.getString("pin", "") ?: ""
    require(pairing || expectedPin.isNotEmpty())
    val trust =
        object : X509TrustManager {
          override fun getAcceptedIssuers(): Array<X509Certificate> = emptyArray()

          override fun checkClientTrusted(chain: Array<X509Certificate>, auth: String) =
              throw java.security.cert.CertificateException()

          override fun checkServerTrusted(chain: Array<X509Certificate>, auth: String) {
            require(chain.isNotEmpty())
            if (!pairing) require(digest(chain[0].encoded) == expectedPin) { "设备身份已变化，请重新关联" }
            // Initial certificate trust is established by certificate-bound SPAKE2 proofs.
          }
        }
    val ssl = SSLContext.getInstance("TLS").apply { init(null, arrayOf(trust), SecureRandom()) }
    val connection = ssl.socketFactory.createSocket() as SSLSocket
    socket = connection
    try {
      connection.connect(InetSocketAddress(resolved, port), 5000)
      connection.soTimeout = 5000
      connection.startHandshake()
      connection.soTimeout = 200
      val cert = connection.session.peerCertificates[0].encoded
      val sessionCode = pairingPin
      val sessionMode = pairingMode
      val input = connection.inputStream
      val output = connection.outputStream
      fun send(message: JSONObject) {
        output.write((message.toString() + "\n").toByteArray())
        output.flush()
      }
      send(
          JSONObject()
              .put("type", "hello")
              .put("version", 2)
              .put("id", id)
              .put("name", android.os.Build.MODEL)
              .put("mode", if (pairing) sessionMode else "")
              .put("token", if (pairing) "" else vault.get("lan-token"))
      )
      var ready = false
      var expectedProof = ""
      var pairedToken = ""
      val frame = ByteArrayOutputStream()

      var heartbeat = System.currentTimeMillis()
      var lastRead = heartbeat
      val started = heartbeat
      while (currentCoroutineContext().isActive && prefs.getBoolean("enabled", false)) {
        if (!ready && System.currentTimeMillis() - started > if (pairing) 120_000 else 10_000)
            error("连接超时")
        if (System.currentTimeMillis() - lastRead > 45_000) error("连接已断开")
        if (ready) {
          outgoing.getAndSet(null)?.let { event ->
            if (event.getLong("created") + TTL >= System.currentTimeMillis())
                send(JSONObject().put("type", "clip").put("clip", event))
          }
          if (System.currentTimeMillis() - heartbeat >= 15_000) {
            send(JSONObject().put("type", "ping"))
            heartbeat = System.currentTimeMillis()
          }
        }
        val byte =
            try {
              input.read()
            } catch (_: SocketTimeoutException) {
              continue
            }
        if (byte < 0) break
        lastRead = System.currentTimeMillis()
        if (byte != 10) {
          require(frame.size() < MAX_FRAME)
          frame.write(byte)
          continue
        }
        val message = JSONObject(frame.toString("UTF-8"))
        frame.reset()
        when (message.getString("type")) {
          "pair" -> {
            require(pairing && sessionCode.matches(Regex("[0-9]{6}")))
            val begin =
                JSONObject(
                    NativeBridge.feature(
                        JSONObject()
                            .put("type", "pairBegin")
                            .put("code", sessionCode)
                            .put("client", id)
                            .put("server", message.getString("id"))
                            .put("certificate", digest(cert))
                            .toString()
                    )
                )
            val finish =
                JSONObject(
                    NativeBridge.feature(
                        JSONObject()
                            .put("type", "pairFinish")
                            .put("handle", begin.getString("handle"))
                            .put("message", message.getString("message"))
                            .toString()
                    )
                )
            expectedProof = finish.getString("server")
            send(
                JSONObject()
                    .put("type", "proof")
                    .put("message", begin.getString("message"))
                    .put("proof", finish.getString("client"))
            )
          }
          "error" -> error(message.optString("message", "关联失败"))
          "paired" -> {
            require(pairing && expectedProof.isNotEmpty())
            require(pairingPin == sessionCode && System.currentTimeMillis() < pairingExpires) { "匹配码已失效" }
            require(
                MessageDigest.isEqual(
                    expectedProof.toByteArray(),
                    message.getString("proof").toByteArray(),
                )
            ) {
              "匹配码不正确"
            }
            pairedToken = message.getString("token")
            require(pairedToken.matches(Regex("[a-f0-9]{64}")))
            require(message.getString("id").matches(Regex("[a-fA-F0-9]{32}")))
            vault.save("lan-token", pairedToken)
            prefs
                .edit()
                .putString("address", address)
                .putString("pin", digest(cert))
                .putString("peer", message.getString("id"))
                .putString("name", message.getString("name"))
                .commit()
          }
          "ready" -> {
            if (pairing) {
              require(pairedToken.isNotEmpty())
              prefs
                  .edit()
                  .putString("address", address)
                  .putString("pin", digest(cert))
                  .putString("peer", message.getString("id"))
                  .putString("name", message.getString("name"))
                  .commit()
            } else require(message.getString("id") == prefs.getString("peer", ""))
            ready = true
            if (pairing) {
              pairingPin = ""
              pairingMode = ""
              pairingExpires = 0
            }
            update {
              it.copy(code = null, expires = 0, name = message.getString("name"), message = "已连接")
            }
            synchronized(gate) {
              latest?.let { event -> if (event.getString("origin") == id) outgoing.set(event) }
            }
          }
          "clip" -> {
            require(ready)
            accept(message.getJSONObject("clip"))
          }
          "ping" -> send(JSONObject().put("type", "pong"))
          "pong" -> {}
          else -> error("设备协议不兼容")
        }
      }
      if (!ready) error("关联未完成")
      update { it.copy(message = "设备离线，正在重连…") }
    } finally {
      connection.close()
      if (socket === connection) socket = null
    }
  }

  @Suppress("DEPRECATION")
  fun discover(): () -> Unit {
    val manager = context.getSystemService(NsdManager::class.java)
    val listener =
        object : NsdManager.DiscoveryListener {
          override fun onDiscoveryStarted(type: String) {}

          override fun onDiscoveryStopped(type: String) {}

          override fun onStartDiscoveryFailed(type: String, error: Int) {
            update { it.copy(message = "未能发现设备，请检查两端是否连接同一局域网") }
          }

          override fun onStopDiscoveryFailed(type: String, error: Int) {}

          override fun onServiceLost(service: NsdServiceInfo) {
            update { it.copy(nearby = it.nearby.filterNot { p -> p.name == service.serviceName }) }
          }

          override fun onServiceFound(service: NsdServiceInfo) {
            manager.resolveService(
                service,
                object : NsdManager.ResolveListener {
                  override fun onResolveFailed(info: NsdServiceInfo, error: Int) {}

                  override fun onServiceResolved(info: NsdServiceInfo) {
                    val host = info.host ?: return
                    if (!host.isSiteLocalAddress && !host.isLinkLocalAddress) return
                    val address = "${host.hostAddress}:${info.port}"
                    val peerId = info.attributes["id"]?.toString(Charsets.UTF_8)
                    val mode = info.attributes["mode"]?.toString(Charsets.UTF_8) ?: ""
                    if (peerId != null && peerId == prefs.getString("peer", ""))
                        prefs.edit().putString("address", address).apply()
                    update {
                      it.copy(
                          nearby =
                              (it.nearby.filterNot { p -> p.name == info.serviceName } +
                                      NearbyClipboard(
                                          info.serviceName,
                                          address,
                                          mode,
                                          peerId ?: "",
                                      ))
                                  .take(16)
                      )
                    }
                  }
                },
            )
          }
        }
    manager.discoverServices("_retype-clip._tcp.", NsdManager.PROTOCOL_DNS_SD, listener)
    return {
      try {
        manager.stopServiceDiscovery(listener)
      } catch (_: IllegalArgumentException) {}
    }
  }
}
