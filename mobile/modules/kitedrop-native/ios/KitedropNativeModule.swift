import ExpoModulesCore
import Foundation
import KitedropCore
import Photos

/// JS bridge for the Expo UI (design §4: UI only). Same function names as the Android module.
public class KitedropNativeModule: Module {
  private var listener: UUID?

  public func definition() -> ModuleDefinition {
    Name("KitedropNative")
    Events("onChange")

    OnStartObserving("onChange") {
      self.listener = KitedropRuntime.shared.addListener { [weak self] e in
        self?.sendEvent("onChange", ["type": String(describing: e).components(separatedBy: "(").first ?? ""])
      }
    }
    OnStopObserving("onChange") {
      if let id = self.listener { KitedropRuntime.shared.removeListener(id) }
      self.listener = nil
    }

    Function("start") { () throws -> [String: Any?] in
      let n = try wrap { try KitedropRuntime.shared.node() }
      KitedropRuntime.shared.adoptPending()
      KitedropRuntime.shared.publishDevices()
      return ["deviceId": n.deviceId(), "name": UIDevice.current.name, "platform": "ios",
              "version": coreVersion(), "port": nil]
    }

    Function("devices") { () throws -> [[String: Any?]] in
      try wrap { try KitedropRuntime.shared.node().devices().map(device) }
    }
    Function("transfers") { (limit: Int) throws -> [[String: Any?]] in
      try wrap { try KitedropRuntime.shared.node().transfers(limit: UInt32(limit)).map(transfer) }
    }
    Function("received") { (limit: Int) throws -> [[String: Any?]] in
      try wrap { try KitedropRuntime.shared.node().received(limit: UInt32(limit)).map(received) }
    }
    Function("diagnostics") { () throws -> [String] in
      try wrap { try KitedropRuntime.shared.node().diagnostics() }
    }
    Function("refresh") { () throws in
      try wrap { try KitedropRuntime.shared.node().refresh() }
    }
    Function("retry") { (id: String) throws in
      try wrap { try KitedropRuntime.shared.node().retry(id: id) }
    }
    Function("cancel") { (id: String) throws in
      try wrap { try KitedropRuntime.shared.node().cancel(id: id) }
    }
    Function("createInvite") { (reverse: Bool) throws -> [String: Any] in
      try wrap {
        let uri = try KitedropRuntime.shared.node().createInvite(reverse: reverse)
        let qr = try qrMatrix(text: uri)
        return ["uri": uri, "size": Int(qr.size), "modules": qr.modules.map { Int($0) }]
      }
    }
    Function("setPushToken") { (token: String?) throws in
      try wrap { try KitedropRuntime.shared.node().setPushToken(token: token) }
    }

    /// Received photos and videos go to Photos (add-only access); asked once.
    AsyncFunction("requestPhotosAccess") { () async -> Bool in
      await PHPhotoLibrary.requestAuthorization(for: .addOnly) == .authorized
    }

    AsyncFunction("pair") { (uri: String) async throws -> String in
      try await pairDevice(uri)
    }
    AsyncFunction("pollOutboxes") { () async throws -> Int in
      try await pollOutboxes()
    }
    AsyncFunction("sendText") { (targets: [String], text: String) async throws -> [String] in
      try await sendText(targets, text)
    }
    AsyncFunction("sendLink") { (targets: [String], url: String) async throws -> [String] in
      try await sendLink(targets, url)
    }
    AsyncFunction("sendFile") { (targets: [String], path: String, name: String?, mime: String?) async throws -> [String] in
      try await sendFile(targets, path, name, mime)
    }
    AsyncFunction("removeDevice") { (id: String) async throws in
      try await removeDevice(id)
    }
    AsyncFunction("renameDevice") { (id: String, name: String) async throws in
      try await renameDevice(id, name)
    }
  }
}

// File-scope async helpers: Expo 58 async closures must not capture `self` (Sendable).

private func wrap<T>(_ f: () throws -> T) throws -> T {
  do {
    return try f()
  } catch let FfiError.Failed(code, detail) {
    throw KitedropError(code: code, detail: detail)
  }
}

private func wrapAsync<T>(_ f: () async throws -> T) async throws -> T {
  do {
    return try await f()
  } catch let FfiError.Failed(code, detail) {
    throw KitedropError(code: code, detail: detail)
  }
}

private func pairDevice(_ uri: String) async throws -> String {
  try await wrapAsync { try await KitedropRuntime.shared.node().pair(uri: uri) }
}

private func pollOutboxes() async throws -> Int {
  KitedropRuntime.shared.adoptPending()
  return try await wrapAsync { Int(try await KitedropRuntime.shared.node().pollOutboxes()) }
}

private func sendText(_ targets: [String], _ text: String) async throws -> [String] {
  try await wrapAsync { try await KitedropRuntime.shared.node().sendText(targets: targets, text: text) }
}

private func sendLink(_ targets: [String], _ url: String) async throws -> [String] {
  try await wrapAsync { try await KitedropRuntime.shared.node().sendLink(targets: targets, url: url) }
}

private func sendFile(_ targets: [String], _ path: String, _ name: String?, _ mime: String?) async throws -> [String] {
  let p = path.hasPrefix("file://") ? (URL(string: path)?.path ?? path) : path
  return try await wrapAsync {
    try await KitedropRuntime.shared.node().sendFile(targets: targets, path: p, name: name, mime: mime)
  }
}

private func removeDevice(_ id: String) async throws {
  try await wrapAsync { try await KitedropRuntime.shared.node().removeDevice(id: id) }
}

private func renameDevice(_ id: String, _ name: String) async throws {
  try await wrapAsync { try await KitedropRuntime.shared.node().renameDevice(id: id, name: name) }
}

private func device(_ d: DeviceItem) -> [String: Any?] {
  ["id": d.id, "name": d.name, "platform": d.platform, "isSelf": d.isSelf, "online": d.online,
   "address": d.address, "lastSeen": d.lastSeen.map { Double($0) }]
}

private func transfer(_ t: TransferItem) -> [String: Any?] {
  ["id": t.id, "targetId": t.targetId, "kind": t.kind, "name": t.name, "size": Double(t.size),
   "state": t.state, "reason": t.reason, "viaOutbox": t.viaOutbox,
   "createdAt": Double(t.createdAt), "updatedAt": Double(t.updatedAt),
   "progress": t.progress.map { ["phase": $0.phase, "done": Double($0.done), "total": Double($0.total)] }]
}

private func received(_ r: ReceivedItem) -> [String: Any?] {
  ["id": r.id, "senderId": r.senderId, "kind": r.kind, "name": r.name, "size": Double(r.size),
   "location": r.location, "at": Double(r.at)]
}
