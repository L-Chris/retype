package io.github.retype.ime

import java.net.InetAddress
import java.net.URI

/** Routing hints never replace the saved peer ID or certificate pin. */
internal object LanEndpoint {
    const val PORT = 50206
    fun allowed(address: InetAddress): Boolean {
        val bytes = address.address
        val overlay = bytes.size == 4 && bytes[0].toInt() and 255 == 100 && (bytes[1].toInt() and 255) in 64..127
        val ula = bytes.size == 16 && bytes[0].toInt() and 254 == 252
        return address.isSiteLocalAddress || address.isLinkLocalAddress || address.isLoopbackAddress || overlay || ula
    }
    fun normalize(value: String): String {
        require(value.trim().length in 1..300) { "请输入设备 IP 或域名，可附带端口" }
        val uri = URI("tcp://${value.trim()}")
        val host = requireNotNull(uri.host) { "设备地址格式无效" }.lowercase()
        require(uri.rawUserInfo == null && uri.rawQuery == null && uri.rawFragment == null && uri.rawPath.isNullOrEmpty()) { "只需填写 IP 或域名和端口" }
        val port = if (uri.port == -1) PORT else uri.port
        require(port in 1..65535) { "端口范围为 1–65535" }
        if (host.startsWith("[") || host.matches(Regex("[0-9.]+"))) {
            require(allowed(InetAddress.getByName(host))) { "仅支持局域网或 Tailscale 地址" }
        } else {
            require(host.length <= 253 && host.split('.').all { it.length in 1..63 && it.matches(Regex("[a-zA-Z0-9](?:[a-zA-Z0-9-]*[a-zA-Z0-9])?")) }) { "设备域名格式无效" }
        }
        return "$host:$port"
    }
    fun list(value: String): List<String> = value.split(',', '\n').map(String::trim).filter(String::isNotEmpty).map(::normalize).distinct().also {
        require(it.size <= 8) { "最多保存 8 个连接地址" }
    }
    fun priority(value: String): Int {
        val host = URI("tcp://$value").host ?: return 1
        if (!host.startsWith("[") && !host.matches(Regex("[0-9.]+"))) return 1
        val address = InetAddress.getByName(host)
        return if (overlay(address) || host.startsWith("[fd7a:115c:a1e0:")) 2 else 0
    }
    fun overlay(address: InetAddress): Boolean {
        val b = address.address
        return when(b.size) {
            4 -> b[0].toInt() and 255 == 100 && (b[1].toInt() and 255) in 64..127
            16 -> b.copyOfRange(0,6).contentEquals(byteArrayOf(0xfd.toByte(),0x7a,0x11,0x5c,0xa1.toByte(),0xe0.toByte()))
            else -> false
        }
    }
}
