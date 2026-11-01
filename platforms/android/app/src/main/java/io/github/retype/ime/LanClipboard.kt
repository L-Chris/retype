package io.github.retype.ime

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.net.Uri
import android.util.Base64
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import java.io.ByteArrayOutputStream
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.Inet4Address
import java.net.SocketTimeoutException
import java.net.NetworkInterface
import java.security.MessageDigest
import java.security.SecureRandom
import java.security.cert.X509Certificate
import java.util.UUID
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
    val peers: List<LanPeerState> = emptyList(),
    val localAddresses: List<String> = emptyList(),
    val code: String? = null,
    val expires: Long = 0,
    val nearby: List<NearbyClipboard> = emptyList(),
    val text: String? = null,
    val image: Uri? = null,
)

/** One process-wide journal and independent authenticated connections to associated devices. */
class LanClipboard private constructor(private val context: Context) {
  companion object {
    private val policy = JSONObject(NativeBridge.feature(JSONObject().put("type", "featurePolicy").toString())).getJSONObject("clipboard")
    private val MAX_TEXT = policy.getInt("maxText")
    private val MAX_IMAGE = policy.getInt("maxImage")
    private val IMAGE_FORMATS = policy.getJSONArray("imageFormats")
    private val MAX_FRAME = policy.getInt("maxFrame")
    @Volatile private var instance: LanClipboard? = null

    fun get(context: Context): LanClipboard =
        synchronized(this) {
          instance ?: LanClipboard(context.applicationContext).also { instance = it }
        }

    internal fun imageInfo(encoded: String): JSONObject? = runCatching {
      JSONObject(NativeBridge.feature(JSONObject().put("type", "clipboardImageInfo").put("image", encoded).toString())).optJSONObject("value")
    }.getOrNull()

    internal fun digest(bytes: ByteArray): String =
        MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }

    internal fun code(cert: ByteArray, client: String, server: String): String =
        ((digest(cert + client.toByteArray() + server.toByteArray()).take(8).toLong(16) % 1_000_000)
            .toString()
            .padStart(6, '0'))
  }

  private val prefs = context.getSharedPreferences("lanClipboard", Context.MODE_PRIVATE)
  private val vault = SecretVault(context)
  private val peers = LanPeerStore(prefs, vault)
  private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
  private val main = Handler(Looper.getMainLooper())
  private val clipboard = context.getSystemService(ClipboardManager::class.java)
  private val mutable =
      MutableStateFlow(
          LanState(
              enabled = prefs.getBoolean("enabled", false),
              peers = peers.all().map { LanPeerState(it.id, it.name, it.manualAddresses.split(',').filter(LanEndpoint::visible).joinToString(", "), it.address) },
          )
      )
  val state = mutable.asStateFlow()
  private val control = Any()
  private val connections = mutableMapOf<String, LanConnection>()
  private var pairingConnection: LanConnection? = null
  @Volatile private var pairingPin = ""
  @Volatile private var pairingMode = ""
  @Volatile private var pairingExpires = 0L
  private val tried = mutableSetOf<String>()
  private var pairingJob: Job? = null
  private val gate = Any()
  private var sequence = maxOf(prefs.getLong("seq", 0), System.currentTimeMillis())
  private var latest: JSONObject? = null
  private var appliedText: String? = null
  private var appliedImage: Uri? = null
  private var captureGeneration = 0L
  private val discoveredAddresses = mutableMapOf<String, List<String>>()
  private var discoveryStop: (() -> Unit)? = null
  private val id =
      prefs.getString("id", null)
          ?: UUID.randomUUID().toString().replace("-", "").also {
            prefs.edit().putString("id", it).commit()
          }

  init {
    clipboard.addPrimaryClipChangedListener { capture() }
    scope.launch {
      runCatching { peers.migrate() }
          .onSuccess { resume() }
          .onFailure { error -> update { it.copy(message = error.message ?: "无法恢复关联设备") } }
    }
  }

  private fun update(change: (LanState) -> LanState) =
      synchronized(mutable) { mutable.value = change(mutable.value) }

  private fun current(link: LanConnection): Boolean = synchronized(control) {
    link.active.get() && prefs.getBoolean("enabled", false) &&
        (pairingConnection === link || connections[link.peerId] === link)
  }

  private fun refreshPeers(reset: Boolean = false) {
    val saved = peers.all()
    update { state ->
      val rows = saved.map { peer ->
        val old = state.peers.firstOrNull { it.id == peer.id }
        val row = if (reset || connections[peer.id]?.active?.get() != true) null else old
        (row ?: LanPeerState(peer.id, peer.name)).copy(name = peer.name,
            addresses = peer.manualAddresses.split(',').filter(LanEndpoint::visible).joinToString(", "), target = row?.target?.ifEmpty { peer.address } ?: peer.address)
      }
      val online = rows.count { it.connection.isNotEmpty() }
      state.copy(peers = rows, message = when {
        !state.enabled -> "已关闭"
        pairingMode.isNotEmpty() -> state.message
        online > 0 -> "已连接 $online 台设备"
        saved.isEmpty() -> "请关联设备"
        else -> "等待设备连接"
      })
    }
  }

  private fun updatePeer(link: LanConnection, target: String = "", connection: String = "", local: String = "", message: String) {
    synchronized(control) {
      if (!current(link)) return
      refreshPeers()
      update { state -> state.copy(peers = state.peers.map { peer ->
        if (peer.id != link.peerId) peer else peer.copy(target = target.ifEmpty { peer.target },
            connection = connection, localAddress = local, message = message)
      }) }
      refreshPeers()
    }
  }

  private fun broadcast(event: JSONObject?) = synchronized(control) {
    (connections.values.toList() + listOfNotNull(pairingConnection)).forEach { link ->
      if (link.active.get()) link.outgoing.set(event)
    }
  }

  private fun refreshLocalAddresses() {
    scope.launch {
      val addresses = runCatching {
        NetworkInterface.getNetworkInterfaces().toList().filter { it.isUp && !it.isLoopback }
            .flatMap { it.inetAddresses.toList() }.filter { it is Inet4Address && LanEndpoint.allowed(it) }
            .mapNotNull { it.hostAddress?.substringBefore('%') }.distinct().sorted()
      }.getOrDefault(emptyList())
      update { it.copy(localAddresses = addresses) }
    }
  }

  fun resume() {
    refreshLocalAddresses()
    synchronized(control) {
      if (!prefs.getBoolean("enabled", false)) return
      if (discoveryStop == null) discoveryStop = discover()
      for (peer in peers.all()) {
        if (connections[peer.id]?.job?.isActive != true) connect(peer.address, false, peer.id)
      }
      refreshPeers()
    }
  }

  fun enable(value: Boolean) {
    prefs.edit().putBoolean("enabled", value).commit()
    update {
      it.copy(enabled = value, message = if (value) "连接中…" else "已关闭", text = null, image = null, code = null)
    }
    if (value) resume()
    else {
      cancelPair(false)
      synchronized(control) {
        connections.values.forEach { it.close(scope) }
        connections.clear()
        discoveryStop?.invoke()
        discoveryStop = null
        refreshPeers(reset = true)
      }
      synchronized(gate) {
        captureGeneration++
        latest = null
        protocol("clear")
        broadcast(null)
      }
    }
  }

  fun unlink(peerId: String) {
    synchronized(control) {
      connections.remove(peerId)?.close(scope)
      peers.remove(peerId)
      synchronized(discoveredAddresses) { discoveredAddresses.remove(peerId) }
      refreshPeers()
    }
  }

  internal fun unlinkAllForTest() {
    enable(false)
    peers.all().forEach { unlink(it.id) }
  }

  fun showCode() {
    startPairing("show", SecureRandom().nextInt(1_000_000).toString().padStart(6, '0'))
  }

  fun join(code: String, address: String = ""): Boolean {
    val target=runCatching { if(address.isBlank()) "" else LanEndpoint.normalize(address) }.getOrElse { error -> update {it.copy(message=error.message?:"设备地址无效")};return false }
    require(code.matches(Regex("[0-9]{6}")))
    startPairing("join", code, target)
    return true
  }

  fun setAddresses(peerId: String, value: String): Boolean {
    val values=runCatching {LanEndpoint.list(value)}.getOrElse { error -> update {it.copy(message=error.message?:"设备地址无效")};return false }
    val text=values.joinToString(", ")
    synchronized(control) {
      val peer = peers.get(peerId) ?: return false
      peers.upsert(peer.copy(manualAddresses = text))
      if(prefs.getBoolean("enabled",false)) connect(peer.address, false, peerId)
      refreshPeers()
    }
    return true
  }

  private fun candidates(peerId: String): List<String> {
    val peer = peers.get(peerId) ?: return emptyList()
    val discovered=synchronized(discoveredAddresses){discoveredAddresses[peerId].orEmpty()}
    return (discovered + LanEndpoint.list(peer.manualAddresses) + peer.addresses + peer.address).filter(String::isNotBlank).distinct().sortedBy(LanEndpoint::priority).take(8)
  }

  private fun remember(peerId: String, name: String, address: String, advertised: org.json.JSONArray?) {
    val old = peers.get(peerId) ?: return
    val values=listOf(address)+(0 until (advertised?.length()?:0)).take(8).map {advertised!!.optString(it)}+old.addresses
    val valid=values.mapNotNull {runCatching {LanEndpoint.normalize(it)}.getOrNull()}.distinct().take(8)
    peers.upsert(old.copy(name = name, addresses = valid, address = address))
  }

  private fun startPairing(mode: String, code: String, target: String = "") = synchronized(control) {
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
        if (synchronized(control) { pairingConnection?.job?.isActive != true }) {
          if (synchronized(tried) { tried.size >= 5 }) {
            cancelPair(false)
            update { it.copy(message = "尝试次数过多，请重新查看或输入匹配码") }
            break
          }
          val wanted = if (pairingMode == "show") "join" else "show"
          val peer =
              if(target.isNotEmpty()) NearbyClipboard("",target,wanted) else mutable.value.nearby.firstOrNull {
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
    resume()
  }

  internal fun pairForTest(address: String, code: String, showing: Boolean = false) {
    startPairing(if (showing) "show" else "join", code)
    connect(address, true)
  }

  fun cancelPair(resumeConnection: Boolean = true) = synchronized(control) {
    pairingPin = ""
    pairingMode = ""
    pairingExpires = 0
    pairingJob?.cancel()
    pairingJob = null
    synchronized(control) {
      pairingConnection?.close(scope)
      pairingConnection = null
      update { it.copy(code = null, expires = 0) }
      refreshPeers()
    }
    if (resumeConnection) resume()
  }

  private fun protocol(operation: String, now: Long = System.currentTimeMillis(), configure: (JSONObject) -> Unit = {}): JSONObject {
    val request = JSONObject().put("type", "clipboard").put("operation", operation).put("now", now)
    configure(request)
    return JSONObject(NativeBridge.feature(request.toString()))
  }

  private fun capture() {
    if (!mutable.value.enabled) return
    val ime =
        Settings.Secure.getString(context.contentResolver, Settings.Secure.DEFAULT_INPUT_METHOD)
    if (ime?.startsWith("${context.packageName}/") != true) return
    try {
      val clip = clipboard.primaryClip ?: return
      val sensitive = clip.description.extras?.getBoolean("android.content.extra.IS_SENSITIVE", false) == true
      if (clip.itemCount == 0) return
      val item = clip.getItemAt(0)
      val uri = item.uri
      val image = uri != null && (clip.description.hasMimeType("image/*") || context.contentResolver.getType(uri)?.startsWith("image/") == true)
      val text = if (image) "" else item.text?.toString().orEmpty()
      val generation = synchronized(gate) {
        if (clip.description.label?.toString() == "retype" &&
            (if (image) appliedImage == uri else appliedText == text)) return
        captureGeneration++
        latest = null
        broadcast(null)
        protocol("clear")
        captureGeneration
      }
      update { it.copy(text = null, image = null) }
      if (sensitive) return
      scope.launch {
        val bytes = if (image) readImage(uri!!) ?: return@launch else null
        if (!image && (text.isEmpty() || text.contains('\u0000') || text.toByteArray().size > MAX_TEXT)) return@launch
        synchronized(gate) {
          if (!mutable.value.enabled || captureGeneration != generation) return@synchronized
          sequence++
          val event = protocol("local") {
            it.put("origin", id).put("seq", sequence).put("text", text)
            if (bytes != null) it.put("image", Base64.encodeToString(bytes, Base64.NO_WRAP))
          }.optJSONObject("event") ?: return@synchronized
          latest = event
          broadcast(event)
          prefs.edit().putLong("seq", sequence).apply()
        }
      }
    } catch (_: SecurityException) {
      /* Another IME is active. */
    }
  }

  private fun readImage(uri: Uri): ByteArray? = runCatching {
    if (uri.scheme != "content") return null
    val bytes = context.contentResolver.openInputStream(uri)?.use { input ->
      val out = ByteArrayOutputStream()
      val buffer = ByteArray(8192)
      while (out.size() <= MAX_IMAGE) {
        val count = input.read(buffer, 0, minOf(buffer.size, MAX_IMAGE + 1 - out.size()))
        if (count < 0) break
        out.write(buffer, 0, count)
      }
      out.toByteArray()
    } ?: return null
    if (bytes.size > MAX_IMAGE) return null
    // The shared protocol validates the encoded bytes and pixel bounds without re-encoding.
    bytes
  }.getOrNull()

  private fun accept(event: JSONObject, source: LanConnection) {
    val now = System.currentTimeMillis()
    val lifetime = protocol("valid", now) { it.put("event", event) }
    if (!lifetime.optBoolean("valid")) return
    val bytes = if (!event.isNull("image")) Base64.decode(event.getString("image"), Base64.NO_WRAP) else null
    val text = if (bytes == null) event.getString("text") else null
    val mime = if (bytes != null) imageInfo(event.getString("image"))?.getString("mime") ?: return else null
    synchronized(gate) {
      if (!current(source)) return
      if (!protocol("accept", now) { it.put("event", event) }.getBoolean("accepted")) return
      captureGeneration++ // A pending local image conversion must not overtake this event.
      latest = event
      // Desktop listeners require a client's clip origin to match its authenticated ID.
      // Remote clips update this phone only, and supersede pending local clipboard sends.
      broadcast(null)
    }
    main.post {
      synchronized(gate) {
        if (!mutable.value.enabled || latest !== event) return@post
        try {
          if (bytes != null) {
            val uri = ClipboardImageProvider.publish(context, bytes, mime!!)
            appliedImage = uri
            appliedText = null
            clipboard.setPrimaryClip(ClipData.newUri(context.contentResolver, "retype", uri))
          } else {
            appliedImage = null
            appliedText = text
            clipboard.setPrimaryClip(ClipData.newPlainText("retype", text))
          }
        } catch (_: SecurityException) {}
      }
      update { it.copy(text = text, image = if (bytes != null) appliedImage else null) }
    }
    scope.launch {
      delay(lifetime.getLong("remaining"))
      synchronized(gate) { if (latest === event) update { it.copy(text = null, image = null) } }
    }
  }

  fun pasteText(): String? = synchronized(gate) {
    val event = latest ?: return null
    if (!mutable.value.enabled || !event.isNull("image") || !protocol("valid") { it.put("event", event) }.getBoolean("valid")) null
    else event.getString("text")
  }

  fun pasteImage(): Uri? = synchronized(gate) {
    val event = latest ?: return null
    if (!mutable.value.enabled || event.isNull("image") || !protocol("valid") { it.put("event", event) }.getBoolean("valid")) null
    else appliedImage
  }

  private fun connect(address: String, pairing: Boolean, peerId: String? = null) {
    synchronized(control) {
      val link = LanConnection(peerId, if (pairing) pairingPin else "",
          if (pairing) pairingMode else "", if (pairing) pairingExpires else 0)
      if (pairing) {
        pairingConnection?.close(scope)
        pairingConnection = link
      } else {
        require(peerId != null)
        connections.put(peerId, link)?.close(scope)
      }
      link.job = scope.launch(start = CoroutineStart.LAZY) {
        var delayMs = 1000L
        while (isActive && current(link)) {
          val initialPair = link.peerId == null
          try {
            val endpoints = if (initialPair) listOf(address) else candidates(link.peerId!!)
            var failure: Exception? = null
            var completed = false
            for (endpoint in endpoints) {
              currentCoroutineContext().ensureActive()
              if (!current(link)) throw CancellationException()
              try {
                updatePeer(link, target = endpoint, message = "连接中…")
                session(endpoint, initialPair, link)
                completed = true
                break
              } catch (error: CancellationException) { throw error }
              catch (error: Exception) {
                failure = error
                android.util.Log.i("retype-lan", "Endpoint failed: $endpoint (${error.javaClass.simpleName})")
                // A validated pairing can persist its credentials before ready arrives.
                if (link.peerId != null && initialPair) break
              }
            }
            if (!completed) throw failure ?: IllegalStateException("缺少设备连接地址")
            delayMs = 1000
          } catch (error: CancellationException) { throw error }
          catch (error: Exception) {
            currentCoroutineContext().ensureActive()
            if (!current(link)) break
            if (link.peerId == null) {
              update { it.copy(message = "尚未找到匹配设备，请检查匹配码及两端连接") }
              break // The pairing coordinator tries another discovered device.
            }
            updatePeer(link, message = if (error is java.io.IOException) "设备离线，正在重连…" else error.message ?: "设备离线，正在重连…")
          }
          delay(delayMs)
          delayMs = minOf(30_000, delayMs * 2)
        }
      }
      updatePeer(link, target = address, message = "连接中…")
      link.job!!.start()
    }
  }

  private fun promote(link: LanConnection, peer: LanPeer, token: String) {
    synchronized(control) {
      if (!current(link) || pairingConnection !== link || pairingPin != link.code ||
          System.currentTimeMillis() >= link.expires) throw CancellationException("匹配码已失效")
      peers.paired(peer, token)
      link.peerId = peer.id
      pairingConnection = null
      connections.put(peer.id, link)?.takeIf { it !== link }?.close(scope)
      pairingJob?.cancel()
      pairingJob = null
      pairingPin = ""
      pairingMode = ""
      pairingExpires = 0
      update { it.copy(code = null, expires = 0) }
      refreshPeers()
    }
  }

  private suspend fun session(address: String, pairing: Boolean, link: LanConnection) {
    val separator = address.lastIndexOf(':')
    require(separator > 0)
    val host = address.substring(0, separator)
    val port = address.substring(separator + 1).toInt()
    require(port in 1..65535)
    val resolved = InetAddress.getAllByName(host).filter(LanEndpoint::allowed).sortedBy(LanEndpoint::overlay)
    require(resolved.isNotEmpty()) { "设备地址不属于局域网或 Tailscale" }
    val expectedPin = if (pairing) "" else peers.get(link.peerId!!)?.pin.orEmpty()
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
    var connected: SSLSocket?=null
    for(ip in resolved.take(16)) {
      currentCoroutineContext().ensureActive()
      val candidate=ssl.socketFactory.createSocket() as SSLSocket
      synchronized(control) {
        if (!current(link)) { candidate.close(); throw CancellationException() }
        link.socket.set(candidate)
      }
      try {candidate.connect(InetSocketAddress(ip,port),3000);connected=candidate;break}
      catch(error: Exception){candidate.close();android.util.Log.i("retype-lan","TCP connection failed: $address (${error.javaClass.simpleName})")}
    }
    val connection=connected?:throw java.io.IOException("设备连接失败，请检查路由、防火墙和同步端口")
    try {
      connection.soTimeout = 5000
      connection.startHandshake()
      connection.soTimeout = 200
      val cert = connection.session.peerCertificates[0].encoded
      val sessionCode = link.code
      val sessionMode = link.mode
      val input = connection.inputStream.buffered(8192)
      val output = connection.outputStream
      fun send(message: JSONObject) {
        output.write((message.toString() + "\n").toByteArray())
        output.flush()
      }
      send(
          JSONObject()
              .put("type", "hello")
              .put("version", 2)
              .put("images", true)
              .put("imageFormats", IMAGE_FORMATS)
              .put("id", id)
              .put("name", android.os.Build.MODEL)
              .put("mode", if (pairing) sessionMode else "")
              .put("token", if (pairing) "" else peers.token(link.peerId!!))
      )
      var ready = false
      var imageFormats = emptySet<String>()
      var expectedProof = ""
      var pairingPeer = ""
      var pairedToken = ""
      val frame = ByteArrayOutputStream()

      var heartbeat = System.currentTimeMillis()
      var lastRead = heartbeat
      val started = heartbeat
      while (currentCoroutineContext().isActive && current(link)) {
        if (!ready && System.currentTimeMillis() - started > if (pairing) 120_000 else 10_000)
            error("连接超时")
        if (System.currentTimeMillis() - lastRead > 45_000) error("连接已断开")
        if (ready) {
          link.outgoing.getAndSet(null)?.let { event ->
            if ((event.isNull("image") || imageInfo(event.getString("image"))?.optString("mime") in imageFormats) && protocol("valid") { it.put("event", event) }.getBoolean("valid"))
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
            require(pairing && expectedProof.isEmpty() && sessionCode.matches(Regex("[0-9]{6}")))
            pairingPeer=message.getString("id")
            require(pairingPeer.matches(Regex("[a-fA-F0-9]{32}")))
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
            require(pairing && expectedProof.isNotEmpty() && message.getString("id")==pairingPeer)
            require(current(link) && pairingPin == sessionCode && System.currentTimeMillis() < link.expires) { "匹配码已失效" }
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
            promote(link, LanPeer(pairingPeer, message.getString("name"), digest(cert), address), pairedToken)
          }
          "ready" -> {
            imageFormats = if (!message.optBoolean("images", false)) emptySet() else {
              val formats = message.optJSONArray("imageFormats")
              if (formats == null) setOf("image/png") else (0 until formats.length()).map { formats.optString(it) }.toSet()
            }
            if (pairing) {
              require(pairedToken.isNotEmpty() && message.getString("id")==pairingPeer)
            } else require(message.getString("id") == link.peerId)
            synchronized(control) {
              if (!current(link)) throw CancellationException()
              remember(link.peerId!!, message.getString("name"), address, message.optJSONArray("addresses"))
            }
            android.util.Log.i("retype-lan","Authenticated connection ready: $address")
            ready = true
            val local = connection.localSocketAddress as InetSocketAddress
            val localHost = local.address.hostAddress!!.substringBefore('%')
            updatePeer(link, target = address, connection = address,
                local = "${if (local.address.address.size == 16) "[$localHost]" else localHost}:${local.port}", message = "已连接")
            refreshLocalAddresses()
            synchronized(gate) {
              if (current(link)) latest?.takeIf { it.getString("origin") == id }?.let { link.outgoing.set(it) }
            }
          }
          "clip" -> {
            require(ready)
            if (current(link)) accept(message.getJSONObject("clip"), link)
          }
          "ping" -> send(JSONObject().put("type", "pong"))
          "pong" -> {}
          else -> error("设备协议不兼容")
        }
      }
      if (!ready) error("关联未完成")
    } finally {
      updatePeer(link, message = "设备离线，正在重连…")
      link.socket.compareAndSet(connection, null)
      runCatching { connection.close() }
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
            update { if(it.peers.none { p -> p.connection.isNotEmpty() }) it.copy(message = "自动发现不可用，可填写设备地址连接") else it }
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
                    if (!LanEndpoint.allowed(host)) return
                    val address = "${if(host.address.size==16) "[${host.hostAddress}]" else host.hostAddress}:${info.port}"
                    val peerId = info.attributes["id"]?.toString(Charsets.UTF_8)
                    val mode = info.attributes["mode"]?.toString(Charsets.UTF_8) ?: ""
                    if (peerId != null && peers.get(peerId) != null)
                        synchronized(discoveredAddresses) {discoveredAddresses[peerId]=(discoveredAddresses[peerId].orEmpty()+address).distinct().take(8)}
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
