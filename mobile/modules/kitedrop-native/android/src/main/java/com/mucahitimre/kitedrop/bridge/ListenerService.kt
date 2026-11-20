package com.mucahitimre.kitedrop.bridge

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.IBinder
import java.io.File

/**
 * Foreground service (type specialUse) that keeps the kitedrop node listening at all times
 * (design §2). Started at boot, after an app update and when the app opens.
 */
class ListenerService : Service() {
  override fun onBind(intent: Intent?): IBinder? = null

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    startForeground(NOTIFICATION_ID, notification(), ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
    try {
      KitedropRuntime.node(this)
      cleanOutgoing(this)
    } catch (e: Exception) {
      android.util.Log.e("kitedrop", "node start failed", e)
    }
    return START_STICKY
  }

  private fun notification(): Notification {
    val nm = getSystemService(NotificationManager::class.java)
    nm.createNotificationChannel(NotificationChannel(CHANNEL_ID, "Ready to receive", NotificationManager.IMPORTANCE_MIN))
    val pending = packageManager.getLaunchIntentForPackage(packageName)?.let {
      PendingIntent.getActivity(this, 0, it, PendingIntent.FLAG_IMMUTABLE)
    }
    return Notification.Builder(this, CHANNEL_ID)
      .setContentTitle("kitedrop")
      .setContentText("Ready to receive from your devices")
      .setSmallIcon(android.R.drawable.stat_sys_download_done)
      .setOngoing(true)
      .setContentIntent(pending)
      .build()
  }

  companion object {
    private const val CHANNEL_ID = "listener"
    private const val NOTIFICATION_ID = 1

    fun start(context: Context) {
      context.startForegroundService(Intent(context, ListenerService::class.java))
    }

    /** Copies of shared items waiting to be sealed; the core seals them within seconds. */
    fun outgoingDir(context: Context) = File(context.cacheDir, "outgoing").apply { mkdirs() }

    private fun cleanOutgoing(context: Context) {
      val cutoff = System.currentTimeMillis() - 24 * 3600 * 1000L
      outgoingDir(context).walkBottomUp().filter { it.isFile && it.lastModified() < cutoff }.forEach { it.delete() }
    }
  }
}
