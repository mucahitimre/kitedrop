// macOS share extension: "Share → kitedrop" in Finder, Safari and other apps (decisions D39).
// It does not send anything itself. It hands what was shared to the kitedrop app through the
// `kitedrop://send?…` URL and closes; the app shows its usual "Send with kitedrop" window, where the
// user picks the device (the same flow as Windows "Send to").
//   file=<absolute path>   (repeated; files and folders — the app reads them directly)
//   link=<http(s) URL>
//   text=<plain text>
import Cocoa
import UniformTypeIdentifiers

@objc(ShareViewController)
final class ShareViewController: NSViewController {
    override func loadView() {
        // Nothing to show: the app's own window takes over.
        view = NSView(frame: NSRect(x: 0, y: 0, width: 1, height: 1))
    }

    override func viewDidAppear() {
        super.viewDidAppear()
        Task { @MainActor in await handOver() }
    }

    @MainActor
    private func handOver() async {
        let providers = (extensionContext?.inputItems as? [NSExtensionItem] ?? [])
            .flatMap { $0.attachments ?? [] }
        var files: [String] = []
        var link: String?
        var text: String?
        for p in providers {
            if p.hasItemConformingToTypeIdentifier(UTType.fileURL.identifier) {
                if let url = await loadURL(p, UTType.fileURL), url.isFileURL {
                    files.append(url.path)
                }
            } else if p.hasItemConformingToTypeIdentifier(UTType.url.identifier) {
                if let url = await loadURL(p, UTType.url), ["http", "https"].contains(url.scheme?.lowercased()) {
                    link = url.absoluteString
                }
            } else if p.hasItemConformingToTypeIdentifier(UTType.plainText.identifier) {
                text = try? await p.loadItem(forTypeIdentifier: UTType.plainText.identifier) as? String
            }
        }

        var c = URLComponents()
        c.scheme = "kitedrop"
        c.host = "send"
        if !files.isEmpty {
            c.queryItems = files.map { URLQueryItem(name: "file", value: $0) }
        } else if let link {
            c.queryItems = [URLQueryItem(name: "link", value: link)]
        } else if let text, !text.isEmpty {
            c.queryItems = [URLQueryItem(name: "text", value: text)]
        }
        if c.queryItems != nil, let url = c.url {
            NSWorkspace.shared.open(url)
        }
        extensionContext?.completeRequest(returningItems: nil, completionHandler: nil)
    }

    /// File and web URLs arrive as `URL` or as their data representation, depending on the source app.
    private func loadURL(_ p: NSItemProvider, _ type: UTType) async -> URL? {
        guard let item = try? await p.loadItem(forTypeIdentifier: type.identifier) else { return nil }
        if let url = item as? URL { return url }
        if let data = item as? Data { return URL(dataRepresentation: data, relativeTo: nil) }
        if let s = item as? String { return URL(string: s) }
        return nil
    }
}
