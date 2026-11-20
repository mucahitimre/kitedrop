import ExpoModulesCore
import KitedropCore
import UIKit

/// iOS relaunches the app when a background upload started by the share extension finishes (the
/// extension is gone by then). Reattach to the session, then adopt the extension's upload records
/// so the receipts are fetched (protocol.md §8: until then the send stays `sending`).
public class KitedropAppDelegate: ExpoAppDelegateSubscriber {
  private var sessions: [String: URLSession] = [:]

  public func application(
    _ application: UIApplication,
    handleEventsForBackgroundURLSession identifier: String,
    completionHandler: @escaping () -> Void
  ) {
    UploadObserver.shared.finishedEvents = completionHandler
    UploadObserver.shared.onTaskDone = { KitedropRuntime.shared.adoptPending() }
    sessions[identifier] = UploadObserver.shared.session(identifier: identifier)
  }
}
