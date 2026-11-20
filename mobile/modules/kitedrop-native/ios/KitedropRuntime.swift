import Foundation
import KitedropCore
import Network
import Photos
import UIKit

/// The app's kitedrop node (iPhone: does not listen; pulls outboxes, design §2/§8). Started by
/// the UI or by a background-session relaunch, whichever comes first.
final class KitedropRuntime {
  static let shared = KitedropRuntime()

  private let lock = NSLock()
  private var nodeRef: KitedropNode?
  private var listeners: [UUID: (NodeEvent) -> Void] = [:]
  private var browser: NWBrowser?
  fileprivate var localNetworkDenied = false

  func node() throws -> KitedropNode {
    lock.lock()
    defer { lock.unlock() }
    if let n = nodeRef { return n }
    let fm = FileManager.default
    let support = fm.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0].appendingPathComponent("kitedrop")
    let config = NodeConfig(
      dataDir: support.path,
      downloadDir: Self.downloads.path,
      deviceName: UIDevice.current.name,
      platform: "ios",
      listen: false,
      presenceIntervalSecs: 300,
      mdns: false,
      pushToken: nil)
    let n = try KitedropNode.start(config: config, identity: try KitedropShared.identity(), host: IOSHost(self))
    nodeRef = n
    DispatchQueue.main.async { self.startBrowsing() }
    return n
  }

  /// `Documents/kitedrop`, visible in the Files app.
  static var downloads: URL {
    FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0].appendingPathComponent("kitedrop")
  }

  func addListener(_ f: @escaping (NodeEvent) -> Void) -> UUID {
    let id = UUID()
    lock.lock()
    listeners[id] = f
    lock.unlock()
    return id
  }

  func removeListener(_ id: UUID) {
    lock.lock()
    listeners[id] = nil
    lock.unlock()
  }

  fileprivate func dispatch(_ e: NodeEvent) {
    lock.lock()
    let all = Array(listeners.values)
    lock.unlock()
    all.forEach { $0(e) }
  }

  /// Uploads started by the share extension: register them so their receipts are fetched (D25).
  func adoptPending() {
    guard let node = try? node() else { return }
    let dir = KitedropShared.pending
    let files = (try? FileManager.default.contentsOfDirectory(at: dir, includingPropertiesForKeys: nil)) ?? []
    for f in files where f.pathExtension == "json" {
      if let json = try? String(contentsOf: f, encoding: .utf8) {
        do {
          try node.adoptUpload(preparedJson: json)
          try? FileManager.default.removeItem(at: f)
        } catch {
          KitedropShared.log("adopt_failed,\(f.lastPathComponent),\(error)")
        }
      }
    }
  }

  /// Keeps the device list in the app group for the share extension.
  func publishDevices() {
    guard let node = nodeRef else { return }
    KitedropShared.saveDevices(node.devices())
  }

  // MARK: Bonjour (Network framework; no multicast entitlement needed, protocol.md §12)

  private func startBrowsing() {
    guard browser == nil else { return }
    let b = NWBrowser(for: .bonjourWithTXTRecord(type: "_kitedrop._tcp", domain: nil), using: .tcp)
    b.stateUpdateHandler = { [weak self] state in
      // A denied Local Network permission shows up as a DNS policy error (TN3179).
      if case .waiting(let error) = state, case .dns(let code) = error, code == DNSServiceErrorType(kDNSServiceErr_PolicyDenied) {
        self?.localNetworkDenied = true
      } else if case .ready = state {
        self?.localNetworkDenied = false
      }
    }
    b.browseResultsChangedHandler = { [weak self] results, _ in
      for r in results {
        guard case .bonjour(let txt) = r.metadata, let id = txt["id"] else { continue }
        self?.resolve(r.endpoint, id: id)
      }
    }
    b.start(queue: .global(qos: .utility))
    browser = b
  }

  /// Bonjour gives a service endpoint; connect once to learn its IP and port.
  private func resolve(_ endpoint: NWEndpoint, id: String) {
    let conn = NWConnection(to: endpoint, using: .tcp)
    conn.stateUpdateHandler = { [weak self] state in
      switch state {
      case .ready:
        if case .hostPort(let host, let port) = conn.currentPath?.remoteEndpoint, case .ipv4(let ip) = host {
          let addr = "\(ip):\(port.rawValue)".replacingOccurrences(of: "%.*:", with: ":", options: .regularExpression)
          self?.nodeRef?.addDiscovered(idHex: id, address: addr)
        }
        conn.cancel()
      case .failed, .waiting:
        conn.cancel()
      default:
        break
      }
    }
    conn.start(queue: .global(qos: .utility))
  }
}

struct KitedropError: LocalizedError {
  let code: String
  let detail: String
  var errorDescription: String? { detail }
}

/// Platform services for the core (design §8, iPhone): clipboard, Photos / Files.
final class IOSHost: NodeHost {
  private weak var runtime: KitedropRuntime?

  init(_ runtime: KitedropRuntime) {
    self.runtime = runtime
  }

  func deliverText(kind: String, text: String, fromName: String) throws {
    DispatchQueue.main.sync { UIPasteboard.general.string = text }
  }

  func deliverFile(path: String, name: String, mime: String, fromName: String) throws -> String {
    let url = URL(fileURLWithPath: path)
    let isImage = mime.hasPrefix("image/")
    guard isImage || mime.hasPrefix("video/") else { return path }
    // Photos and videos go to Photos when allowed; otherwise they stay in Files.
    guard PHPhotoLibrary.authorizationStatus(for: .addOnly) == .authorized else { return path }
    var localId: String?
    do {
      try PHPhotoLibrary.shared().performChangesAndWait {
        let req = PHAssetCreationRequest.forAsset()
        req.addResource(with: isImage ? .photo : .video, fileURL: url, options: nil)
        localId = req.placeholderForCreatedAsset?.localIdentifier
      }
    } catch {
      return path
    }
    try? FileManager.default.removeItem(at: url)
    return "photos:\(localId ?? "")"
  }

  func httpsPost(url: String, jsonBody: String) throws -> UInt16 {
    guard let u = URL(string: url) else { throw HostError.Failed(detail: "bad url") }
    var req = URLRequest(url: u, timeoutInterval: 10)
    req.httpMethod = "POST"
    req.setValue("application/json", forHTTPHeaderField: "Content-Type")
    req.httpBody = Data(jsonBody.utf8)
    let done = DispatchSemaphore(value: 0)
    var status: UInt16 = 0
    var failure: Error?
    URLSession.shared.dataTask(with: req) { _, resp, err in
      status = UInt16((resp as? HTTPURLResponse)?.statusCode ?? 0)
      failure = err
      done.signal()
    }.resume()
    done.wait()
    if let failure { throw HostError.Failed(detail: failure.localizedDescription) }
    return status
  }

  func onEvent(event: NodeEvent) {
    switch event {
    case .devicesChanged, .presence, .paired, .removedFromGroup:
      runtime?.publishDevices()
    default:
      break
    }
    runtime?.dispatch(event)
  }

  func platformIssues() -> [String] {
    runtime?.localNetworkDenied == true ? ["local_network_denied"] : []
  }
}
