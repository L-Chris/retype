package io.github.retype.ime

import android.content.Context
import androidx.test.platform.app.InstrumentationRegistry
import java.util.UUID
import org.json.JSONArray
import org.junit.Assert.*
import org.junit.Test

class LanPeerStoreTest {
  @Test fun legacyPairingMigratesAndDevicesKeepIndependentCredentialsAndAddresses() {
    val context = InstrumentationRegistry.getInstrumentation().targetContext
    val prefs = context.getSharedPreferences("lan-migration-${UUID.randomUUID()}", Context.MODE_PRIVATE)
    val vault = SecretVault(context)
    val first = UUID.randomUUID().toString().replace("-", "")
    val second = UUID.randomUUID().toString().replace("-", "")
    try {
      prefs.edit().putString("peer", first).putString("name", "Mac").putString("pin", "old-pin")
          .putString("address", "192.168.1.2:50206")
          .putString("addresses", JSONArray(listOf("192.168.1.2:50206")).toString())
          .putString("manualAddresses", "100.64.0.1:50206").commit()
      vault.save("lan-token", "first-secret")
      val store = LanPeerStore(prefs, vault)
      store.migrate()
      store.migrate()
      assertEquals("first-secret", store.token(first))
      assertEquals("", vault.get("lan-token"))
      assertFalse(prefs.contains("peer"))
      store.paired(LanPeer(second, "Windows", "second-pin", "192.168.1.3:50206"), "second-secret")
      store.upsert(store.get(second)!!.copy(manualAddresses = "100.64.0.2:50206"))
      val reopened = LanPeerStore(prefs, vault)
      assertEquals(listOf(first, second), reopened.all().map { it.id })
      assertEquals("old-pin", reopened.get(first)!!.pin)
      assertEquals("100.64.0.1:50206", reopened.get(first)!!.manualAddresses)
      assertEquals("first-secret", reopened.token(first))
      assertEquals("second-secret", reopened.token(second))
      assertFalse(prefs.getString("peers", "")!!.contains("secret"))
      reopened.remove(second)
      assertEquals(listOf(first), reopened.all().map { it.id })
      assertEquals("first-secret", reopened.token(first))
      assertEquals("", reopened.token(second))
      // Re-pairing the same device updates only that device, retaining its route preferences.
      reopened.paired(LanPeer(first, "Renamed Mac", "new-pin", "192.168.1.4:50206"), "rotated-secret")
      assertEquals(1, reopened.all().size)
      assertEquals("100.64.0.1:50206", reopened.get(first)!!.manualAddresses)
      assertEquals("rotated-secret", reopened.token(first))
    } finally {
      prefs.edit().clear().commit()
      for (key in listOf("lan-token", LanPeerStore.tokenKey(first), LanPeerStore.tokenKey(second))) vault.save(key, "")
    }
  }
}
