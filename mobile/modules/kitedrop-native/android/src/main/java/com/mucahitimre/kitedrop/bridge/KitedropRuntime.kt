package com.mucahitimre.kitedrop.bridge

import android.content.Context
import android.os.Build
import android.provider.Settings
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import java.io.File
import java.security.KeyStore
import java.util.concurrent.CopyOnWriteArrayList
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import uniffi.kitedrop_ffi.KitedropNode
import uniffi.kitedrop_ffi.NodeConfig
import uniffi.kitedrop_ffi.NodeEvent
import uniffi.kitedrop_ffi.generateIdentity

/**
 * The one kitedrop node of this process. The foreground service starts it at boot; the UI and
 * the share screen use the same instance.
 */
object KitedropRuntime {
  @Volatile private var node: KitedropNode? = null
  private val listeners = CopyOnWriteArrayList<(NodeEvent) -> Unit>()

  fun node(context: Context): KitedropNode =
    node ?: synchronized(this) {
      node ?: start(context.applicationContext).also { node = it }
    }

  fun isRunning(): Boolean = node != null

  fun addListener(l: (NodeEvent) -> Unit) = listeners.add(l)

  fun removeListener(l: (NodeEvent) -> Unit) = listeners.remove(l)

  internal fun dispatch(e: NodeEvent) {
    for (l in listeners) {
      try {
        l(e)
      } catch (_: Exception) {
      }
    }
  }

  private fun start(context: Context): KitedropNode {
    val config = NodeConfig(
      dataDir = File(context.filesDir, "kitedrop").path,
      // Received files land here first; the host moves them to Downloads/kitedrop.
      downloadDir = File(context.filesDir, "incoming").path,
      deviceName = deviceName(context),
      platform = "android",
      listen = true,
      presenceIntervalSecs = 300uL,
      mdns = true,
      pushToken = null,
    )
    return KitedropNode.start(config, Identity.load(context), AndroidHost(context))
  }

  fun deviceName(context: Context): String =
    Settings.Global.getString(context.contentResolver, Settings.Global.DEVICE_NAME)
      ?.takeIf { it.isNotBlank() }
      ?: "${Build.MANUFACTURER.replaceFirstChar { it.uppercase() }} ${Build.MODEL}"
}

/**
 * The 64-byte device identity, encrypted with an AES-GCM key that lives in the Android Keystore
 * and never leaves it (design §5: private keys in the platform secure store).
 */
internal object Identity {
  private const val ALIAS = "kitedrop-identity-wrap"
  private const val FILE = "identity.bin"

  fun load(context: Context): ByteArray {
    val file = File(context.filesDir, FILE)
    // Phase 0b spike kept a plain key file; it was only ever paired with a test server.
    File(context.filesDir, "identity.key").delete()
    if (file.exists()) {
      val data = file.readBytes()
      val cipher = Cipher.getInstance("AES/GCM/NoPadding")
      cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, data.copyOfRange(0, 12)))
      return cipher.doFinal(data, 12, data.size - 12)
    }
    val identity = generateIdentity()
    val cipher = Cipher.getInstance("AES/GCM/NoPadding")
    cipher.init(Cipher.ENCRYPT_MODE, key())
    val tmp = File(context.filesDir, "$FILE.tmp")
    tmp.writeBytes(cipher.iv + cipher.doFinal(identity))
    tmp.renameTo(file)
    return identity
  }

  private fun key(): SecretKey {
    val ks = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
    (ks.getEntry(ALIAS, null) as? KeyStore.SecretKeyEntry)?.let { return it.secretKey }
    val gen = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
    gen.init(
      KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
        .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
        .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
        .setKeySize(256)
        .build()
    )
    return gen.generateKey()
  }
}
