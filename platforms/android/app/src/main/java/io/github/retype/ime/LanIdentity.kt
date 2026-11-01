package io.github.retype.ime

import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import java.net.Socket
import java.security.KeyPairGenerator
import java.security.KeyStore
import java.security.Principal
import java.security.PrivateKey
import java.security.SecureRandom
import java.security.cert.X509Certificate
import java.security.spec.ECGenParameterSpec
import java.util.Date
import javax.net.ssl.*
import javax.security.auth.x500.X500Principal

/** Stable per-install server identity. The private key never leaves Android Keystore. */
internal class LanIdentity private constructor(val certificate: ByteArray, val context: SSLContext) {
  companion object {
    private const val ALIAS = "retype.clipboard.tls.v1"

    @Synchronized fun load(): LanIdentity {
      val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
      if (!store.containsAlias(ALIAS)) {
        KeyPairGenerator.getInstance(KeyProperties.KEY_ALGORITHM_EC, "AndroidKeyStore").apply {
          initialize(KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_SIGN or KeyProperties.PURPOSE_VERIFY)
              .setAlgorithmParameterSpec(ECGenParameterSpec("secp256r1"))
              // TLS providers sign a pre-hashed ECDSA transcript with NONEwithECDSA.
              .setDigests(KeyProperties.DIGEST_NONE, KeyProperties.DIGEST_SHA256, KeyProperties.DIGEST_SHA384, KeyProperties.DIGEST_SHA512)
              .setCertificateSubject(X500Principal("CN=retype.local"))
              .setCertificateNotBefore(Date(0))
              .setCertificateNotAfter(Date(4102444800000L))
              .build())
        }.generateKeyPair()
      }
      val key = store.getKey(ALIAS, null) as PrivateKey
      val certificate = store.getCertificate(ALIAS) as X509Certificate
      val manager = object : X509ExtendedKeyManager() {
        override fun getPrivateKey(alias: String?) = if (alias == ALIAS) key else null
        override fun getCertificateChain(alias: String?) = if (alias == ALIAS) arrayOf(certificate) else null
        override fun getServerAliases(type: String?, issuers: Array<out Principal>?) =
            if (type == "EC") arrayOf(ALIAS) else null
        override fun chooseServerAlias(type: String?, issuers: Array<out Principal>?, socket: Socket?) =
            getServerAliases(type, issuers)?.firstOrNull()
        override fun chooseEngineServerAlias(type: String?, issuers: Array<out Principal>?, engine: SSLEngine?) =
            getServerAliases(type, issuers)?.firstOrNull()
        override fun getClientAliases(type: String?, issuers: Array<out Principal>?): Array<String>? = null
        override fun chooseClientAlias(types: Array<out String>?, issuers: Array<out Principal>?, socket: Socket?): String? = null
      }
      val tls = SSLContext.getInstance("TLS").apply { init(arrayOf(manager), null, SecureRandom()) }
      return LanIdentity(certificate.encoded, tls)
    }
  }
}
