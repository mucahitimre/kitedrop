package com.mucahitimre.kitedrop.bridge

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.PowerManager
import android.provider.Settings
import expo.modules.kotlin.exception.CodedException
import expo.modules.kotlin.exception.Exceptions
import expo.modules.kotlin.functions.Coroutine
import expo.modules.kotlin.modules.Module
import expo.modules.kotlin.modules.ModuleDefinition
import uniffi.kitedrop_ffi.DeviceItem
import uniffi.kitedrop_ffi.FfiException
import uniffi.kitedrop_ffi.NodeEvent
import uniffi.kitedrop_ffi.ReceivedItem
import uniffi.kitedrop_ffi.TransferItem
import uniffi.kitedrop_ffi.coreVersion
import uniffi.kitedrop_ffi.qrMatrix

/** JS bridge for the Expo UI (design §4: the UI only; the service and share screen are native). */
class KitedropNativeModule : Module() {
  private val context: Context
    get() = appContext.reactContext ?: throw Exceptions.ReactContextLost()

  private val listener: (NodeEvent) -> Unit = { e ->
    sendEvent("onChange", mapOf("type" to e.javaClass.simpleName))
  }

  private fun <T> call(block: () -> T): T = try {
    block()
  } catch (e: FfiException.Failed) {
    throw CodedException(e.code, e.detail, null)
  }

  private suspend fun <T> callAsync(block: suspend () -> T): T = try {
    block()
  } catch (e: FfiException.Failed) {
    throw CodedException(e.code, e.detail, null)
  }

  private fun node() = KitedropRuntime.node(context)

  override fun definition() = ModuleDefinition {
    Name("KitedropNative")
    Events("onChange")

    OnStartObserving("onChange") { KitedropRuntime.addListener(listener) }
    OnStopObserving("onChange") { KitedropRuntime.removeListener(listener) }

    Function("start") {
      ListenerService.start(context)
      val n = node()
      mapOf(
        "deviceId" to n.deviceId(),
        "name" to KitedropRuntime.deviceName(context),
        "platform" to "android",
        "version" to coreVersion(),
        "port" to n.port()?.toInt(),
      )
    }

    Function("devices") { node().devices().map(::device) }
    Function("transfers") { limit: Int -> call { node().transfers(limit.toUInt()).map(::transfer) } }
    Function("received") { limit: Int -> call { node().received(limit.toUInt()).map(::received) } }
    Function("diagnostics") { node().diagnostics() }
    Function("refresh") { node().refresh() }
    Function("retry") { id: String -> call { node().retry(id) } }
    Function("cancel") { id: String -> call { node().cancel(id) } }

    Function("createInvite") { reverse: Boolean ->
      call {
        val uri = node().createInvite(reverse)
        val qr = qrMatrix(uri)
        mapOf("uri" to uri, "size" to qr.size.toInt(), "modules" to qr.modules.map { it.toInt() })
      }
    }

    AsyncFunction("pair") Coroutine { uri: String -> callAsync { node().pair(uri) } }
    AsyncFunction("sendText") Coroutine { targets: List<String>, text: String ->
      callAsync { node().sendText(targets, text) }
    }
    AsyncFunction("sendLink") Coroutine { targets: List<String>, url: String ->
      callAsync { node().sendLink(targets, url) }
    }
    /** `path`: a file path (document picker copies into the cache). */
    AsyncFunction("sendFile") Coroutine { targets: List<String>, path: String, name: String?, mime: String? ->
      callAsync { node().sendFile(targets, path.removePrefix("file://"), name, mime) }
    }
    AsyncFunction("removeDevice") Coroutine { id: String -> callAsync { node().removeDevice(id) } }
    AsyncFunction("renameDevice") Coroutine { id: String, name: String -> callAsync { node().renameDevice(id, name) } }

    Function("isIgnoringBatteryOptimizations") {
      val pm = context.getSystemService(Context.POWER_SERVICE) as PowerManager
      pm.isIgnoringBatteryOptimizations(context.packageName)
    }

    Function("requestIgnoreBatteryOptimizations") {
      val intent = Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS)
        .setData(Uri.parse("package:${context.packageName}"))
        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
      context.startActivity(intent)
    }
  }

  private fun device(d: DeviceItem) = mapOf(
    "id" to d.id, "name" to d.name, "platform" to d.platform, "isSelf" to d.isSelf,
    "online" to d.online, "address" to d.address, "lastSeen" to d.lastSeen?.toDouble(),
  )

  private fun transfer(t: TransferItem) = mapOf(
    "id" to t.id, "targetId" to t.targetId, "kind" to t.kind, "name" to t.name, "size" to t.size.toDouble(),
    "state" to t.state, "reason" to t.reason, "viaOutbox" to t.viaOutbox,
    "createdAt" to t.createdAt.toDouble(), "updatedAt" to t.updatedAt.toDouble(),
    "progress" to t.progress?.let {
      mapOf("phase" to it.phase, "done" to it.done.toDouble(), "total" to it.total.toDouble())
    },
  )

  private fun received(r: ReceivedItem) = mapOf(
    "id" to r.id, "senderId" to r.senderId, "kind" to r.kind, "name" to r.name, "size" to r.size.toDouble(),
    "location" to r.location, "at" to r.at.toDouble(),
  )
}
