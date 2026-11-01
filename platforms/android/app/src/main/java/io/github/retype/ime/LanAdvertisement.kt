package io.github.retype.ime

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.net.wifi.WifiManager
import android.os.Handler
import android.os.Looper

/** Serialize asynchronous registration/withdrawal, including pairing-mode changes. */
internal class LanAdvertisement(context: Context, private val id: String, private val failed: () -> Unit) {
  private val main = Handler(Looper.getMainLooper())
  private val manager = context.getSystemService(NsdManager::class.java)
  private val multicast = context.applicationContext.getSystemService(WifiManager::class.java)
      .createMulticastLock("retype-clipboard").apply { setReferenceCounted(false) }
  private var desired: Pair<Int, String>? = null
  private var active: Registration? = null

  fun update(port: Int, mode: String) { main.post { desired = if (port > 0) port to mode else null; reconcile() } }

  private fun reconcile() {
    val old = active
    if (old != null) {
      if (old.spec != desired && old.registered && !old.stopping) {
        old.stopping = true
        runCatching { manager.unregisterService(old) }.onFailure { active = null; reconcile() }
      }
      return
    }
    val spec = desired
    if (spec == null) {
      if (multicast.isHeld) multicast.release()
      return
    }
    runCatching {
      if (!multicast.isHeld) multicast.acquire()
      val registration = Registration(spec)
      active = registration
      manager.registerService(NsdServiceInfo().apply {
        serviceName = "retype-${id.take(12)}"
        serviceType = "_retype-clip._tcp."
        port = spec.first
        setAttribute("id", id)
        setAttribute("version", "2")
        setAttribute("platform", "android")
        setAttribute("name", android.os.Build.MODEL.take(40))
        setAttribute("dial", "1")
        setAttribute("mode", spec.second)
      }, NsdManager.PROTOCOL_DNS_SD, registration)
    }.onFailure { active = null; desired = null; if (multicast.isHeld) multicast.release(); failed() }
  }

  private inner class Registration(val spec: Pair<Int, String>) : NsdManager.RegistrationListener {
    var registered = false
    var stopping = false
    override fun onServiceRegistered(info: NsdServiceInfo) { main.post {
      if (active === this) { registered = true; reconcile() }
    } }
    override fun onServiceUnregistered(info: NsdServiceInfo) { main.post {
      if (active === this) { active = null; reconcile() }
    } }
    override fun onRegistrationFailed(info: NsdServiceInfo, error: Int) { main.post {
      if (active === this) {
        active = null
        if (desired != spec) reconcile()
        else { desired = null; if (multicast.isHeld) multicast.release(); failed() }
      }
    } }
    override fun onUnregistrationFailed(info: NsdServiceInfo, error: Int) { main.post {
      if (active === this) { stopping = false; failed() }
    } }
  }
}
