package io.github.retype.ime

import android.content.SharedPreferences
import java.net.Socket
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicReference
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch
import org.json.JSONArray
import org.json.JSONObject

internal data class LanPeer(
    val id: String,
    val name: String,
    val pin: String,
    val address: String,
    val addresses: List<String> = emptyList(),
    val manualAddresses: String = "",
)

data class LanPeerState(
    val id: String,
    val name: String,
    val addresses: String = "",
    val target: String = "",
    val connection: String = "",
    val localAddress: String = "",
    val message: String = "离线",
)

/** Metadata and certificate pins are separate from per-device encrypted credentials. */
internal class LanPeerStore(private val prefs: SharedPreferences, private val vault: SecretVault) {
  @Synchronized fun all(): List<LanPeer> {
    if (!prefs.contains("peers")) {
      val id = prefs.getString("peer", "").orEmpty()
      if (id.isEmpty()) return emptyList()
      return listOf(LanPeer(id, prefs.getString("name", "").orEmpty(), prefs.getString("pin", "").orEmpty(),
          prefs.getString("address", "").orEmpty(), strings(JSONArray(prefs.getString("addresses", "[]"))),
          prefs.getString("manualAddresses", "").orEmpty()))
    }
    val values = JSONArray(prefs.getString("peers", "[]"))
    return List(values.length()) { index ->
      val value = values.getJSONObject(index)
      LanPeer(value.getString("id"), value.getString("name"), value.getString("pin"), value.getString("address"),
          strings(value.optJSONArray("addresses")), value.optString("manualAddresses"))
    }
  }

  @Synchronized fun migrate() {
    if (prefs.contains("peers")) {
      vault.save("lan-token", "")
      return
    }
    val legacy = all()
    // Publish the new list only after the credential copy succeeds. A retry is safe.
    legacy.firstOrNull()?.let { vault.save(tokenKey(it.id), vault.get("lan-token")) }
    save(legacy, removeLegacy = true)
    vault.save("lan-token", "")
  }

  @Synchronized fun get(id: String): LanPeer? = all().firstOrNull { it.id == id }
  fun token(id: String): String = vault.get(tokenKey(id))

  @Synchronized fun paired(peer: LanPeer, token: String) {
    migrate()
    vault.save(tokenKey(peer.id), token)
    val old = get(peer.id)
    upsert(peer.copy(addresses = (peer.addresses + old?.addresses.orEmpty()).distinct().take(8),
        manualAddresses = old?.manualAddresses.orEmpty()))
  }

  @Synchronized fun upsert(peer: LanPeer) {
    val old = all()
    save(if (old.any { it.id == peer.id }) old.map { if (it.id == peer.id) peer else it } else old + peer)
  }
  @Synchronized fun remove(id: String) {
    save(all().filterNot { it.id == id })
    vault.save(tokenKey(id), "")
  }

  private fun save(peers: List<LanPeer>, removeLegacy: Boolean = false) {
    val values = JSONArray()
    for (peer in peers) values.put(JSONObject().put("id", peer.id).put("name", peer.name).put("pin", peer.pin)
        .put("address", peer.address).put("addresses", JSONArray(peer.addresses)).put("manualAddresses", peer.manualAddresses))
    val edit = prefs.edit().putString("peers", values.toString())
    if (removeLegacy) for (key in listOf("peer", "name", "pin", "address", "addresses", "manualAddresses")) edit.remove(key)
    check(edit.commit()) { "无法保存关联设备" }
  }

  companion object {
    internal fun tokenKey(id: String) = "lan-token-$id"
    private fun strings(values: JSONArray?): List<String> =
        if (values == null) emptyList() else List(values.length()) { values.getString(it) }
  }
}

/** Each connection owns its socket and latest outgoing clip; closing one cannot detach another. */
internal class LanConnection(@Volatile var peerId: String? = null, val code: String = "", val mode: String = "", val expires: Long = 0) {
  var job: Job? = null
  val active = AtomicBoolean(true)
  val socket = AtomicReference<Socket?>(null)
  val outgoing = AtomicReference<JSONObject?>(null)

  fun close(scope: CoroutineScope) {
    active.set(false)
    job?.cancel()
    outgoing.set(null)
    val old = socket.getAndSet(null) ?: return
    scope.launch { runCatching { old.close() } }
  }
}
