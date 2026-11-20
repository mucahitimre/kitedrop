package com.mucahitimre.kitedrop.bridge

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.ClipData
import android.content.ClipboardManager
import android.content.ContentValues
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Environment
import android.os.Handler
import android.os.Looper
import android.provider.MediaStore
import java.io.File
import java.net.HttpURLConnection
import java.net.URL
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicReference
import uniffi.kitedrop_ffi.HostException
import uniffi.kitedrop_ffi.NodeEvent
import uniffi.kitedrop_ffi.NodeHost

/** Platform services for the core (design §8, Android): clipboard, Downloads/kitedrop, notifications. */
class AndroidHost(private val context: Context) : NodeHost {
  private val nm = context.getSystemService(NotificationManager::class.java)
  private val ids = AtomicInteger(100)

  init {
    nm.createNotificationChannel(NotificationChannel(RECEIVED, "Received items", NotificationManager.IMPORTANCE_DEFAULT))
    nm.createNotificationChannel(NotificationChannel(PROBLEMS, "Send problems", NotificationManager.IMPORTANCE_DEFAULT))
  }

  override fun deliverText(kind: String, text: String, fromName: String) {
    // Writing the clipboard from a background service is not documented either way (D13), so
    // try it and always offer a "Copy" action on the notification as the fallback (design §8).
    val copied = writeClipboard(text)
    val id = ids.incrementAndGet()
    val saved = File(textDir(context), "$id.txt").apply { writeText(text) }
    val copy = PendingIntent.getBroadcast(
      context, id,
      Intent(context, CopyReceiver::class.java).putExtra(CopyReceiver.FILE, saved.path),
      PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
    )
    val what = if (kind == "link") "Link" else "Text"
    notify(
      id, RECEIVED,
      Notification.Builder(context, RECEIVED)
        .setContentTitle("$what from $fromName" + if (copied) " — copied" else "")
        .setContentText(text.take(200))
        .setStyle(Notification.BigTextStyle().bigText(text.take(1000)))
        .setSmallIcon(android.R.drawable.stat_sys_download_done)
        .setAutoCancel(true)
        .addAction(Notification.Action.Builder(null, "Copy", copy).build()),
    )
  }

  private fun writeClipboard(text: String): Boolean {
    val ok = AtomicReference(false)
    val done = CountDownLatch(1)
    Handler(Looper.getMainLooper()).post {
      try {
        val cm = context.getSystemService(ClipboardManager::class.java)
        cm.setPrimaryClip(ClipData.newPlainText("kitedrop", text))
        ok.set(true)
      } catch (_: Exception) {
      } finally {
        done.countDown()
      }
    }
    done.await(5, TimeUnit.SECONDS)
    return ok.get()
  }

  override fun deliverFile(path: String, name: String, mime: String, fromName: String): String {
    val src = File(path)
    val uri: Uri = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
      val values = ContentValues().apply {
        put(MediaStore.Downloads.DISPLAY_NAME, src.name)
        put(MediaStore.Downloads.MIME_TYPE, mime)
        put(MediaStore.Downloads.RELATIVE_PATH, "${Environment.DIRECTORY_DOWNLOADS}/kitedrop")
        put(MediaStore.Downloads.IS_PENDING, 1)
      }
      val resolver = context.contentResolver
      val target = resolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values)
        ?: throw HostException.Failed("could not create the file in Downloads")
      try {
        resolver.openOutputStream(target)!!.use { out -> src.inputStream().use { it.copyTo(out) } }
        resolver.update(target, ContentValues().apply { put(MediaStore.Downloads.IS_PENDING, 0) }, null, null)
      } catch (e: Exception) {
        resolver.delete(target, null, null)
        throw HostException.Failed("could not write to Downloads: ${e.message}")
      }
      src.delete()
      target
    } else {
      // Before Android 10 there is no MediaStore.Downloads; keep it in the app's own folder.
      val dir = File(context.getExternalFilesDir(Environment.DIRECTORY_DOWNLOADS), "kitedrop").apply { mkdirs() }
      val dest = File(dir, src.name)
      if (!src.renameTo(dest)) throw HostException.Failed("could not move the file")
      Uri.fromFile(dest)
    }
    val id = ids.incrementAndGet()
    val open = PendingIntent.getActivity(
      context, id,
      Intent(Intent.ACTION_VIEW).setDataAndType(uri, mime)
        .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_TASK),
      PendingIntent.FLAG_IMMUTABLE,
    )
    notify(
      id, RECEIVED,
      Notification.Builder(context, RECEIVED)
        .setContentTitle("File from $fromName")
        .setContentText("$name — saved to Downloads/kitedrop")
        .setSmallIcon(android.R.drawable.stat_sys_download_done)
        .setContentIntent(open)
        .setAutoCancel(true),
    )
    return uri.toString()
  }

  override fun httpsPost(url: String, jsonBody: String): UShort {
    val conn = URL(url).openConnection() as HttpURLConnection
    try {
      conn.requestMethod = "POST"
      conn.connectTimeout = 10_000
      conn.readTimeout = 10_000
      conn.doOutput = true
      conn.setRequestProperty("content-type", "application/json")
      conn.setRequestProperty("accept", "application/json")
      conn.outputStream.use { it.write(jsonBody.toByteArray()) }
      return conn.responseCode.toUShort()
    } catch (e: Exception) {
      throw HostException.Failed(e.message ?: "push failed")
    } finally {
      conn.disconnect()
    }
  }

  override fun onEvent(event: NodeEvent) {
    if (event is NodeEvent.TransferChanged && event.transfer.state == "failed") {
      val t = event.transfer
      val target = try {
        KitedropRuntime.node(context).devices().firstOrNull { it.id == t.targetId }?.name
      } catch (_: Exception) {
        null
      } ?: "a device"
      val what = t.name.ifEmpty { t.kind }
      notify(
        ids.incrementAndGet(), PROBLEMS,
        Notification.Builder(context, PROBLEMS)
          .setContentTitle("Could not send $what to $target")
          .setContentText(reasonText(t.reason))
          .setSmallIcon(android.R.drawable.stat_notify_error)
          .setAutoCancel(true),
      )
    }
    KitedropRuntime.dispatch(event)
  }

  override fun platformIssues(): List<String> = emptyList()

  private fun notify(id: Int, channel: String, b: Notification.Builder) {
    try {
      nm.notify(id, b.build())
    } catch (_: SecurityException) {
      // POST_NOTIFICATIONS not granted: the item is still stored and listed in the app.
    }
  }

  companion object {
    const val RECEIVED = "received"
    const val PROBLEMS = "problems"

    fun textDir(context: Context) = File(context.cacheDir, "texts").apply { mkdirs() }

    fun reasonText(reason: String?): String = when {
      reason == null -> ""
      reason == "expired" -> "It waited too long without reaching the device."
      reason == "cancelled" -> "You cancelled it."
      reason == "removed_from_group" -> "The device is no longer in your group."
      reason == "file_unreadable" -> "The file could not be read."
      reason.startsWith("rejected:") -> "The device refused it (${reason.removePrefix("rejected:")})."
      else -> reason
    }
  }
}

/** "Copy" action on a received-text notification (fallback for background clipboard writes). */
class CopyReceiver : BroadcastReceiver() {
  override fun onReceive(context: Context, intent: Intent) {
    val file = intent.getStringExtra(FILE)?.let(::File) ?: return
    if (!file.path.startsWith(AndroidHost.textDir(context).path) || !file.exists()) return
    context.getSystemService(ClipboardManager::class.java)
      .setPrimaryClip(ClipData.newPlainText("kitedrop", file.readText()))
  }

  companion object {
    const val FILE = "file"
  }
}
