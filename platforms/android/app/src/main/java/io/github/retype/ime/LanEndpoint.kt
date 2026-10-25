package io.github.retype.ime

import java.net.InetAddress
import org.json.JSONObject

/** Routing policy is shared with desktop Rust; addresses never replace peer identity. */
internal object LanEndpoint {
    val PORT = JSONObject(NativeBridge.feature(JSONObject().put("type", "featurePolicy").toString())).getJSONObject("clipboard").getInt("port")
    private fun request(operation: String, value: String): JSONObject = JSONObject(NativeBridge.feature(
        JSONObject().put("type", "lanEndpoint").put("operation", operation).put("value", value).toString()))
    fun allowed(address: InetAddress): Boolean = request("allowed", address.hostAddress!!.substringBefore('%')).getBoolean("allowed")
    fun normalize(value: String): String = request("normalize", value).getString("address")
    fun list(value: String): List<String> = request("list", value).getJSONArray("addresses").let { a -> List(a.length()) { a.getString(it) } }
    fun priority(value: String): Int = request("priority", value).getInt("priority")
    fun overlay(address: InetAddress): Boolean = request("overlay", address.hostAddress!!.substringBefore('%')).getBoolean("overlay")
}
