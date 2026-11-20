import KitedropCore
import UIKit
import UniformTypeIdentifiers

/// Share extension (design §8): pick a device, stream-encrypt each shared item into an envelope
/// in the app group container, hand it to a background URLSession upload and exit. The app later
/// adopts the uploads and fetches the signed receipts (protocol.md §8, D25).
/// Looks like the app (same palette and device cards, D40).
final class ShareViewController: UIViewController {
  private let summary = UILabel()
  private let status = UILabel()
  private let list = UIStackView()
  private var rows: [DeviceRow] = []
  private var busy = false

  override func viewDidLoad() {
    super.viewDidLoad()
    view.backgroundColor = Palette.bg

    let header = UIStackView(arrangedSubviews: [LogoView(), titleBlock()])
    header.axis = .horizontal
    header.spacing = 12
    header.alignment = .center

    list.axis = .vertical
    list.spacing = 10

    status.font = .preferredFont(forTextStyle: .subheadline)
    status.textColor = Palette.muted
    status.numberOfLines = 0
    status.textAlignment = .center
    status.isHidden = true

    var cancelConfig = UIButton.Configuration.plain()
    cancelConfig.title = "Cancel"
    cancelConfig.baseForegroundColor = Palette.accent
    let cancel = UIButton(configuration: cancelConfig, primaryAction: UIAction { [weak self] _ in
      self?.extensionContext?.cancelRequest(withError: NSError(domain: "kitedrop", code: 0))
    })

    let stack = UIStackView(arrangedSubviews: [header, sectionLabel("SEND TO"), list, status, cancel])
    stack.axis = .vertical
    stack.spacing = 14
    stack.setCustomSpacing(22, after: header)
    stack.setCustomSpacing(8, after: stack.arrangedSubviews[1])
    stack.translatesAutoresizingMaskIntoConstraints = false
    let scroll = UIScrollView()
    scroll.translatesAutoresizingMaskIntoConstraints = false
    scroll.alwaysBounceVertical = true
    scroll.addSubview(stack)
    view.addSubview(scroll)
    NSLayoutConstraint.activate([
      scroll.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor),
      scroll.bottomAnchor.constraint(equalTo: view.bottomAnchor),
      scroll.leadingAnchor.constraint(equalTo: view.leadingAnchor),
      scroll.trailingAnchor.constraint(equalTo: view.trailingAnchor),
      stack.topAnchor.constraint(equalTo: scroll.contentLayoutGuide.topAnchor, constant: 24),
      stack.bottomAnchor.constraint(equalTo: scroll.contentLayoutGuide.bottomAnchor, constant: -24),
      stack.leadingAnchor.constraint(equalTo: scroll.frameLayoutGuide.leadingAnchor, constant: 16),
      stack.trailingAnchor.constraint(equalTo: scroll.frameLayoutGuide.trailingAnchor, constant: -16),
    ])

    summary.text = describe(providers())
    // iPhones do not listen, so they cannot be targets of a direct upload.
    let targets = KitedropShared.loadDevices().filter { $0.platform != "ios" }
    if targets.isEmpty {
      show("No device to send to. Open kitedrop and add your computer or Android phone first.", ok: false)
    }
    for d in targets {
      let row = DeviceRow(device: d)
      row.addAction(UIAction { [weak self, weak row] _ in
        guard let self, let row else { return }
        self.choose(row)
      }, for: .touchUpInside)
      rows.append(row)
      list.addArrangedSubview(row)
    }
  }

  private func titleBlock() -> UIView {
    let title = UILabel()
    title.text = "Send with kitedrop"
    title.font = .systemFont(ofSize: 20, weight: .bold)
    title.textColor = Palette.text
    summary.font = .preferredFont(forTextStyle: .subheadline)
    summary.textColor = Palette.muted
    summary.numberOfLines = 2
    let v = UIStackView(arrangedSubviews: [title, summary])
    v.axis = .vertical
    v.spacing = 2
    return v
  }

  private func sectionLabel(_ text: String) -> UILabel {
    let l = UILabel()
    l.text = text
    l.font = .systemFont(ofSize: 12.5, weight: .semibold)
    l.textColor = Palette.muted
    return l
  }

  private func providers() -> [NSItemProvider] {
    (extensionContext?.inputItems as? [NSExtensionItem] ?? []).flatMap { $0.attachments ?? [] }
  }

  /// "IMG_0412.HEIC", "3 photos", "Link", "2 items".
  private func describe(_ ps: [NSItemProvider]) -> String {
    guard let first = ps.first else { return "Nothing to send" }
    func kind(_ p: NSItemProvider) -> String {
      if p.hasItemConformingToTypeIdentifier(UTType.image.identifier) { return "photo" }
      if p.hasItemConformingToTypeIdentifier(UTType.movie.identifier) { return "video" }
      if p.hasItemConformingToTypeIdentifier(UTType.url.identifier)
        && !p.hasItemConformingToTypeIdentifier(UTType.fileURL.identifier) { return "link" }
      if p.hasItemConformingToTypeIdentifier(UTType.plainText.identifier) { return "text" }
      return "file"
    }
    if ps.count == 1 {
      switch kind(first) {
      case "link": return "Link"
      case "text": return "Text"
      default: return first.suggestedName ?? kind(first).capitalized
      }
    }
    let kinds = Set(ps.map(kind))
    if kinds.count == 1, let k = kinds.first { return "\(ps.count) \(k)s" }
    return "\(ps.count) items"
  }

  private func choose(_ row: DeviceRow) {
    guard !busy else { return }
    busy = true
    for r in rows where r !== row { r.setDimmed(true) }
    row.setChosen("Preparing…")
    start(target: row.device, row: row)
  }

  private func start(target: KitedropShared.SharedDevice, row: DeviceRow) {
    guard let address = target.addresses.first else {
      return finish("\(target.name) has not been seen on the network yet. Open kitedrop once while both are on the same Wi-Fi.", ok: false, delay: 3)
    }
    let items = providers()
    guard !items.isEmpty else { return finish("Nothing to send.", ok: false) }
    // One envelope and upload per item, one after the other (each file is streamed from disk).
    var sent = 0
    var failure: String?
    func next(_ i: Int) {
      guard i < items.count else {
        if let failure, sent == 0 { return finish(failure, ok: false, delay: 2.5) }
        let what = items.count == 1 ? "" : " \(sent) of \(items.count)"
        return finish("Sending\(what) to \(target.name). kitedrop shows when it is delivered.", ok: failure == nil)
      }
      DispatchQueue.main.async { row.setChosen(items.count > 1 ? "Preparing \(i + 1) of \(items.count)…" : "Preparing…") }
      item(items[i], to: target, at: address) { error in
        if let error { failure = error } else { sent += 1 }
        next(i + 1)
      }
    }
    next(0)
  }

  private func item(
    _ provider: NSItemProvider, to target: KitedropShared.SharedDevice, at address: String,
    done: @escaping (String?) -> Void
  ) {
    if provider.hasItemConformingToTypeIdentifier(UTType.url.identifier),
       !provider.hasItemConformingToTypeIdentifier(UTType.fileURL.identifier)
    {
      provider.loadItem(forTypeIdentifier: UTType.url.identifier) { item, _ in
        let text = (item as? URL)?.absoluteString ?? ""
        done(self.send(kind: "link", file: nil, text: text, name: "", mime: "text/uri-list", to: target, at: address))
      }
    } else if provider.hasItemConformingToTypeIdentifier(UTType.plainText.identifier) {
      provider.loadItem(forTypeIdentifier: UTType.plainText.identifier) { item, _ in
        let text = item as? String ?? ""
        done(self.send(kind: "text", file: nil, text: text, name: "", mime: "text/plain; charset=utf-8", to: target, at: address))
      }
    } else {
      let type = [UTType.movie, .image, .data].first { provider.hasItemConformingToTypeIdentifier($0.identifier) } ?? .data
      // The URL is valid only inside the handler, so seal synchronously here (one chunk of memory).
      provider.loadFileRepresentation(forTypeIdentifier: type.identifier) { url, error in
        guard let url else { return done("Could not read the item: \(error?.localizedDescription ?? "?")") }
        let mime = UTType(filenameExtension: url.pathExtension)?.preferredMIMEType ?? "application/octet-stream"
        done(self.send(kind: "file", file: url, text: nil, name: url.lastPathComponent, mime: mime, to: target, at: address))
      }
    }
  }

  /// Seals one item and starts its background upload. Returns an error message, or nil.
  private func send(
    kind: String, file: URL?, text: String?, name: String, mime: String,
    to target: KitedropShared.SharedDevice, at address: String
  ) -> String? {
    do {
      let envelope = KitedropShared.outbox.appendingPathComponent(UUID().uuidString + ".ktdp")
      let up = try prepareUpload(
        identity: try KitedropShared.identity(), targetSigPk: target.sigPk, targetKemPk: target.kemPk,
        address: address, kind: kind, sourcePath: file?.path, text: text, name: name, mime: mime,
        envelopePath: envelope.path)
      // Record first, so the app knows about the upload even if the session never reports back.
      try Data(up.json.utf8).write(to: KitedropShared.pending.appendingPathComponent(up.transferId + ".json"), options: .atomic)

      var req = URLRequest(url: URL(string: up.url)!)
      req.httpMethod = "POST"
      req.setValue(up.authorization, forHTTPHeaderField: "Authorization")
      req.setValue("application/octet-stream", forHTTPHeaderField: "Content-Type")
      let session = UploadObserver.shared.session(identifier: KitedropShared.extensionSessionId)
      let task = session.uploadTask(with: req, fromFile: envelope)
      task.taskDescription = up.transferId
      task.resume()
      KitedropShared.log("ext_handoff,\(up.transferId),bytes=\(up.contentLength)")
      return nil
    } catch {
      return "Could not send: \(error)"
    }
  }

  private func show(_ message: String, ok: Bool) {
    let symbol = ok ? "checkmark.circle.fill" : "exclamationmark.triangle.fill"
    let text = NSMutableAttributedString(
      attachment: NSTextAttachment(image: UIImage(systemName: symbol)!.withTintColor(ok ? Palette.ok : Palette.err, renderingMode: .alwaysOriginal)))
    text.append(NSAttributedString(string: "  " + message, attributes: [.foregroundColor: Palette.text]))
    status.attributedText = text
    status.isHidden = false
  }

  private func finish(_ message: String, ok: Bool = true, delay: Double = 1.2) {
    DispatchQueue.main.async {
      self.show(message, ok: ok)
      DispatchQueue.main.asyncAfter(deadline: .now() + delay) {
        self.extensionContext?.completeRequest(returningItems: nil)
      }
    }
  }
}

/// Same colors as the app (mobile/src/lib/theme.ts), light and dark.
private enum Palette {
  static func dyn(_ light: UInt32, _ dark: UInt32, darkAlpha: CGFloat = 1) -> UIColor {
    UIColor { $0.userInterfaceStyle == .dark ? UIColor(hex: dark, alpha: darkAlpha) : UIColor(hex: light) }
  }
  static let bg = dyn(0xf6f7fb, 0x0b1020)
  static let card = dyn(0xffffff, 0x131a2c)
  static let text = dyn(0x0f172a, 0xe2e8f0)
  static let muted = dyn(0x64748b, 0x94a3b8)
  static let line = dyn(0xe2e8f0, 0x233049)
  static let accent = dyn(0x2563eb, 0x3b82f6)
  static let accentSoft = dyn(0xdbeafe, 0x1e3a8a, darkAlpha: 0.4)
  static let ok = dyn(0x16a34a, 0x22c55e)
  static let err = dyn(0xdc2626, 0xf87171)
}

private extension UIColor {
  convenience init(hex: UInt32, alpha: CGFloat = 1) {
    self.init(
      red: CGFloat((hex >> 16) & 0xff) / 255, green: CGFloat((hex >> 8) & 0xff) / 255,
      blue: CGFloat(hex & 0xff) / 255, alpha: alpha)
  }
}

/// The app mark: blue rounded square with the white kite.
private final class LogoView: UIView {
  override init(frame: CGRect) {
    super.init(frame: frame)
    backgroundColor = Palette.accent
    layer.cornerRadius = 11
    layer.cornerCurve = .continuous
    translatesAutoresizingMaskIntoConstraints = false
    NSLayoutConstraint.activate([widthAnchor.constraint(equalToConstant: 42), heightAnchor.constraint(equalToConstant: 42)])
    let kite = CAShapeLayer()
    let p = UIBezierPath()
    p.move(to: CGPoint(x: 21, y: 9)); p.addLine(to: CGPoint(x: 31.5, y: 21))
    p.addLine(to: CGPoint(x: 21, y: 34)); p.addLine(to: CGPoint(x: 10.5, y: 21)); p.close()
    kite.path = p.cgPath
    kite.fillColor = UIColor.white.cgColor
    layer.addSublayer(kite)
  }
  required init?(coder: NSCoder) { fatalError() }
}

/// One device card: platform icon with online dot, name, status line, chevron.
private final class DeviceRow: UIControl {
  let device: KitedropShared.SharedDevice
  private let sub = UILabel()
  private let chevron = UIImageView(image: UIImage(systemName: "chevron.right"))
  private let spinner = UIActivityIndicatorView(style: .medium)

  init(device: KitedropShared.SharedDevice) {
    self.device = device
    super.init(frame: .zero)
    backgroundColor = Palette.card
    layer.cornerRadius = 16
    layer.cornerCurve = .continuous
    layer.borderWidth = 1
    layer.borderColor = Palette.line.cgColor

    let icon = UIImageView(image: UIImage(systemName: Self.symbol(device.platform)))
    icon.tintColor = Palette.accent
    icon.contentMode = .scaleAspectFit
    icon.translatesAutoresizingMaskIntoConstraints = false
    let avatar = UIView()
    avatar.backgroundColor = Palette.accentSoft
    avatar.layer.cornerRadius = 11
    avatar.layer.cornerCurve = .continuous
    avatar.translatesAutoresizingMaskIntoConstraints = false
    avatar.addSubview(icon)
    let dot = UIView()
    dot.backgroundColor = device.online ? Palette.ok : Palette.muted
    dot.layer.cornerRadius = 6.5
    dot.layer.borderWidth = 2
    dot.layer.borderColor = Palette.card.cgColor
    dot.translatesAutoresizingMaskIntoConstraints = false
    avatar.addSubview(dot)

    let name = UILabel()
    name.text = device.name
    name.font = .systemFont(ofSize: 17, weight: .semibold)
    name.textColor = Palette.text
    sub.text = Self.status(device)
    sub.font = .systemFont(ofSize: 13)
    sub.textColor = Palette.muted
    let texts = UIStackView(arrangedSubviews: [name, sub])
    texts.axis = .vertical
    texts.spacing = 2

    chevron.tintColor = Palette.muted
    chevron.preferredSymbolConfiguration = .init(pointSize: 13, weight: .semibold)
    spinner.hidesWhenStopped = true
    spinner.color = Palette.accent

    let row = UIStackView(arrangedSubviews: [avatar, texts, UIView(), spinner, chevron])
    row.axis = .horizontal
    row.spacing = 12
    row.alignment = .center
    row.isUserInteractionEnabled = false
    row.translatesAutoresizingMaskIntoConstraints = false
    addSubview(row)
    NSLayoutConstraint.activate([
      avatar.widthAnchor.constraint(equalToConstant: 42), avatar.heightAnchor.constraint(equalToConstant: 42),
      icon.centerXAnchor.constraint(equalTo: avatar.centerXAnchor), icon.centerYAnchor.constraint(equalTo: avatar.centerYAnchor),
      icon.widthAnchor.constraint(equalToConstant: 22), icon.heightAnchor.constraint(equalToConstant: 22),
      dot.widthAnchor.constraint(equalToConstant: 13), dot.heightAnchor.constraint(equalToConstant: 13),
      dot.trailingAnchor.constraint(equalTo: avatar.trailingAnchor, constant: 2),
      dot.bottomAnchor.constraint(equalTo: avatar.bottomAnchor, constant: 2),
      row.topAnchor.constraint(equalTo: topAnchor, constant: 12),
      row.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -12),
      row.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 14),
      row.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -14),
    ])
  }

  required init?(coder: NSCoder) { fatalError() }

  override var isHighlighted: Bool {
    didSet { alpha = isHighlighted ? 0.7 : 1 }
  }

  override func traitCollectionDidChange(_ previous: UITraitCollection?) {
    super.traitCollectionDidChange(previous)
    if layer.borderColor != Palette.accent.cgColor { layer.borderColor = Palette.line.cgColor }
  }

  func setDimmed(_ dimmed: Bool) {
    isEnabled = !dimmed
    alpha = dimmed ? 0.4 : 1
  }

  func setChosen(_ text: String) {
    isEnabled = false
    layer.borderColor = Palette.accent.cgColor
    layer.borderWidth = 2
    sub.text = text
    sub.textColor = Palette.accent
    chevron.isHidden = true
    spinner.startAnimating()
  }

  static func symbol(_ platform: String) -> String {
    switch platform {
    case "macos": return "laptopcomputer"
    case "windows": return "desktopcomputer"
    case "ios": return "iphone"
    default: return "smartphone"
    }
  }

  static func status(_ d: KitedropShared.SharedDevice) -> String {
    let platform = ["macos": "Mac", "windows": "Windows", "android": "Android"][d.platform] ?? d.platform
    if d.addresses.isEmpty { return "\(platform) · not seen on the network yet" }
    return "\(platform) · \(d.online ? "online" : "offline")"
  }
}
