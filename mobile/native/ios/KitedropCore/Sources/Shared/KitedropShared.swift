import Foundation
import Security

/// Code shared by the iOS app and the share extension. No React Native / Expo dependency.
///
/// - The device identity lives in the Keychain, in the app group's access group, so both
///   processes use the same keys (design §5).
/// - The app writes the device list (`devices.json`) into the app group container; the extension
///   reads it to pick a target and seal to its keys without a running node.
/// - The extension leaves one JSON record per started upload in `pending/`; the app adopts them
///   (`KitedropNode.adoptUpload`) and fetches the receipts.
public enum KitedropShared {
  public static let appGroup = "group.com.mucahitimre.kitedrop"
  /// One background session identifier per process (Apple: each process needs its own).
  public static let extensionSessionId = "com.mucahitimre.kitedrop.upload.share"

  public static var container: URL {
    guard let url = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: appGroup) else {
      fatalError("app group \(appGroup) missing from entitlements")
    }
    return url
  }

  static func dir(_ name: String) -> URL {
    let url = container.appendingPathComponent(name, isDirectory: true)
    try? FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    return url
  }

  /// Sealed envelopes handed to the background session.
  public static var outbox: URL { dir("outbox") }
  /// Prepared-upload records waiting for the app.
  public static var pending: URL { dir("pending") }

  // MARK: Identity (Keychain, shared access group)

  private static let service = "com.mucahitimre.kitedrop"
  private static let account = "device-identity"

  private static func query() -> [String: Any] {
    [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: service,
      kSecAttrAccount as String: account,
      kSecAttrAccessGroup as String: appGroup,
    ]
  }

  public struct KeychainError: Error, CustomStringConvertible {
    public let status: OSStatus
    public var description: String { "Keychain error \(status)" }
  }

  /// The 64-byte identity, created on first use. Readable after the first unlock so the app can
  /// finish a background upload while the phone is locked.
  public static func identity() throws -> Data {
    var q = query()
    q[kSecReturnData as String] = true
    q[kSecMatchLimit as String] = kSecMatchLimitOne
    var item: CFTypeRef?
    let status = SecItemCopyMatching(q as CFDictionary, &item)
    if status == errSecSuccess, let data = item as? Data, data.count == 64 {
      return data
    }
    guard status == errSecItemNotFound else { throw KeychainError(status: status) }
    let fresh = generateIdentity()
    var add = query()
    add[kSecValueData as String] = fresh
    add[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
    let added = SecItemAdd(add as CFDictionary, nil)
    guard added == errSecSuccess else { throw KeychainError(status: added) }
    return fresh
  }

  // MARK: Device list for the extension

  public struct SharedDevice: Codable {
    public var id: String
    public var name: String
    public var platform: String
    public var online: Bool
    public var sigPk: Data
    public var kemPk: Data
    public var addresses: [String]

    public init(_ d: DeviceItem) {
      id = d.id
      name = d.name
      platform = d.platform
      online = d.online
      sigPk = d.sigPk
      kemPk = d.kemPk
      addresses = d.addresses
    }
  }

  private static var devicesFile: URL { container.appendingPathComponent("devices.json") }

  public static func saveDevices(_ devices: [DeviceItem]) {
    let list = devices.filter { !$0.isSelf }.map(SharedDevice.init)
    if let data = try? JSONEncoder().encode(list) {
      try? data.write(to: devicesFile, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
    }
  }

  public static func loadDevices() -> [SharedDevice] {
    guard let data = try? Data(contentsOf: devicesFile) else { return [] }
    return (try? JSONDecoder().decode([SharedDevice].self, from: data)) ?? []
  }

  // MARK: Diagnostics log (`unix_ms,line`, shared by app and extension, last 200 KB)

  private static let logQueue = DispatchQueue(label: "kitedrop.log")
  private static var logFile: URL { container.appendingPathComponent("kitedrop.log") }

  public static func log(_ line: String) {
    let row = "\(Int64(Date().timeIntervalSince1970 * 1000)),\(line)\n"
    logQueue.sync {
      if let attrs = try? FileManager.default.attributesOfItem(atPath: logFile.path),
         (attrs[.size] as? UInt64 ?? 0) > 200_000
      {
        try? FileManager.default.removeItem(at: logFile)
      }
      if let handle = try? FileHandle(forWritingTo: logFile) {
        handle.seekToEndOfFile()
        handle.write(Data(row.utf8))
        try? handle.close()
      } else {
        try? Data(row.utf8).write(to: logFile)
      }
    }
  }
}

/// Delegate of the background upload session. While the extension lives it logs; when iOS
/// relaunches the app for the session's events, the app reattaches and then adopts the pending
/// records to fetch receipts (decision D8: the response body is not relied on).
public final class UploadObserver: NSObject, URLSessionDataDelegate {
  public static let shared = UploadObserver()
  public var finishedEvents: (() -> Void)?
  public var onTaskDone: (() -> Void)?

  public func session(identifier: String) -> URLSession {
    let config = URLSessionConfiguration.background(withIdentifier: identifier)
    config.sharedContainerIdentifier = KitedropShared.appGroup
    config.isDiscretionary = false
    config.sessionSendsLaunchEvents = true
    return URLSession(configuration: config, delegate: self, delegateQueue: nil)
  }

  public func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
    let status = (task.response as? HTTPURLResponse)?.statusCode ?? 0
    KitedropShared.log("upload_done,\(task.taskDescription ?? "?"),status=\(status),error=\((error as NSError?)?.code ?? 0)")
    onTaskDone?()
  }

  public func urlSessionDidFinishEvents(forBackgroundURLSession session: URLSession) {
    DispatchQueue.main.async {
      self.finishedEvents?()
      self.finishedEvents = nil
    }
  }
}
