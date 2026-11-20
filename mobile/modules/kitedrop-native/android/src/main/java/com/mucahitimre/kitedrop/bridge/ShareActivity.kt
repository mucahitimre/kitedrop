package com.mucahitimre.kitedrop.bridge

import android.app.Activity
import android.app.AlertDialog
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.provider.OpenableColumns
import android.widget.Toast
import java.io.File
import kotlinx.coroutines.runBlocking

/**
 * Android share target (design §8): "Share → kitedrop" from any app. A small native dialog picks
 * the device; no React Native is involved, so it opens fast.
 */
class ShareActivity : Activity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    super.onCreate(savedInstanceState)
    ListenerService.start(this)
    val node = try {
      KitedropRuntime.node(this)
    } catch (e: Exception) {
      toast("kitedrop could not start: ${e.message}")
      finish()
      return
    }
    val targets = node.devices().filter { !it.isSelf }
    if (targets.isEmpty()) {
      toast("No other devices yet. Open kitedrop to add one.")
      finish()
      return
    }
    val labels = targets.map { d ->
      val state = when {
        d.platform == "ios" -> "gets it when the app opens"
        d.online -> "online"
        else -> "offline — sent when it is back"
      }
      "${d.name}  ·  $state"
    } + if (targets.size > 1) listOf("All devices") else emptyList()
    AlertDialog.Builder(this)
      .setTitle("Send with kitedrop")
      .setItems(labels.toTypedArray()) { _, which ->
        val ids = if (which == targets.size) targets.map { it.id } else listOf(targets[which].id)
        send(ids)
      }
      .setOnCancelListener { finish() }
      .show()
  }

  private fun send(ids: List<String>) {
    val intent = intent
    Thread {
      val result = try {
        runBlocking { dispatch(intent, ids) }
      } catch (e: Exception) {
        "Could not send: ${e.message}"
      }
      runOnUiThread {
        toast(result)
        finish()
      }
    }.start()
  }

  private suspend fun dispatch(intent: Intent, ids: List<String>): String {
    val node = KitedropRuntime.node(this)
    val uris = streams(intent)
    if (uris.isNotEmpty()) {
      for (uri in uris) {
        val (name, file) = copyToOutgoing(uri)
        node.sendFile(ids, file.path, name, contentResolver.getType(uri))
      }
      return if (uris.size == 1) "Sending…" else "Sending ${uris.size} files…"
    }
    val text = intent.getStringExtra(Intent.EXTRA_TEXT)?.takeIf { it.isNotEmpty() }
      ?: return "Nothing to send"
    val trimmed = text.trim()
    if (!trimmed.contains(Regex("\\s")) && (trimmed.startsWith("https://") || trimmed.startsWith("http://"))) {
      node.sendLink(ids, trimmed)
    } else {
      node.sendText(ids, text)
    }
    return "Sending…"
  }

  @Suppress("DEPRECATION")
  private fun streams(intent: Intent): List<Uri> = when (intent.action) {
    Intent.ACTION_SEND -> listOfNotNull(intent.getParcelableExtra(Intent.EXTRA_STREAM))
    Intent.ACTION_SEND_MULTIPLE -> intent.getParcelableArrayListExtra<Uri>(Intent.EXTRA_STREAM) ?: emptyList()
    else -> emptyList()
  }

  /** The core reads files by path, so content URIs are copied into the app cache first. */
  private fun copyToOutgoing(uri: Uri): Pair<String, File> {
    val name = contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c ->
      if (c.moveToFirst()) c.getString(0) else null
    } ?: uri.lastPathSegment ?: "file"
    val dir = File(ListenerService.outgoingDir(this), System.nanoTime().toString()).apply { mkdirs() }
    val file = File(dir, name.replace('/', '_'))
    contentResolver.openInputStream(uri)!!.use { input -> file.outputStream().use { input.copyTo(it) } }
    return name to file
  }

  private fun toast(text: String) = Toast.makeText(this, text, Toast.LENGTH_SHORT).show()
}
