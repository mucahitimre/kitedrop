package com.mucahitimre.kitedrop.bridge

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/** Starts the listener after boot (and after an app update) without opening the app. */
class BootReceiver : BroadcastReceiver() {
  override fun onReceive(context: Context, intent: Intent) {
    when (intent.action) {
      Intent.ACTION_BOOT_COMPLETED, Intent.ACTION_MY_PACKAGE_REPLACED -> ListenerService.start(context)
    }
  }
}
