// Draws the kitedrop mark (desktop/icon.svg) into the Expo icon files under mobile/assets/.
//   swift scripts/make-mobile-icons.swift
// The shapes are the same as icon.svg, in its 1024 × 1024 coordinate space.
import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

let blue = CGColor(srgbRed: 0x25 / 255.0, green: 0x63 / 255.0, blue: 0xeb / 255.0, alpha: 1)
let white = CGColor(srgbRed: 1, green: 1, blue: 1, alpha: 1)

enum Background { case none, square, rounded }

/// `scale`: size of the mark relative to the canvas (1 = as in icon.svg).
func render(_ path: String, size: Int, background: Background, scale: CGFloat = 1, opaque: Bool = false) {
    let space = CGColorSpace(name: CGColorSpace.sRGB)!
    let info = opaque ? CGImageAlphaInfo.noneSkipLast.rawValue : CGImageAlphaInfo.premultipliedLast.rawValue
    let ctx = CGContext(data: nil, width: size, height: size, bitsPerComponent: 8, bytesPerRow: 0,
                        space: space, bitmapInfo: info)!
    let s = CGFloat(size) / 1024
    // SVG has y down.
    ctx.translateBy(x: 0, y: CGFloat(size))
    ctx.scaleBy(x: s, y: -s)

    switch background {
    case .none: break
    case .square:
        ctx.setFillColor(blue)
        ctx.fill(CGRect(x: 0, y: 0, width: 1024, height: 1024))
    case .rounded:
        ctx.setFillColor(blue)
        ctx.addPath(CGPath(roundedRect: CGRect(x: 0, y: 0, width: 1024, height: 1024),
                           cornerWidth: 224, cornerHeight: 224, transform: nil))
        ctx.fillPath()
    }

    // Mark, scaled around its own centre (y 150…940 → centre 545).
    ctx.translateBy(x: 512, y: 512)
    ctx.scaleBy(x: scale, y: scale)
    ctx.translateBy(x: -512, y: -545)

    let kite = CGMutablePath()
    kite.move(to: CGPoint(x: 512, y: 150))
    kite.addLine(to: CGPoint(x: 770, y: 450))
    kite.addLine(to: CGPoint(x: 512, y: 790))
    kite.addLine(to: CGPoint(x: 254, y: 450))
    kite.closeSubpath()
    ctx.setFillColor(white)
    ctx.addPath(kite)
    ctx.fillPath()

    // Cross: background colour on a coloured background, cut out on a transparent one.
    let cross = CGMutablePath()
    cross.move(to: CGPoint(x: 512, y: 150))
    cross.addLine(to: CGPoint(x: 512, y: 790))
    cross.move(to: CGPoint(x: 254, y: 450))
    cross.addLine(to: CGPoint(x: 770, y: 450))
    ctx.saveGState()
    ctx.addPath(kite)
    ctx.clip()
    ctx.setLineWidth(28)
    if background == .none {
        ctx.setBlendMode(.clear)
    } else {
        ctx.setStrokeColor(blue)
    }
    ctx.addPath(cross)
    ctx.strokePath()
    ctx.restoreGState()

    let tail = CGMutablePath()
    tail.move(to: CGPoint(x: 512, y: 790))
    tail.addCurve(to: CGPoint(x: 520, y: 940), control1: CGPoint(x: 470, y: 850), control2: CGPoint(x: 560, y: 880))
    ctx.setStrokeColor(white)
    ctx.setLineWidth(30)
    ctx.setLineCap(.round)
    ctx.addPath(tail)
    ctx.strokePath()

    let url = URL(fileURLWithPath: path)
    let dest = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil)!
    CGImageDestinationAddImage(dest, ctx.makeImage()!, nil)
    CGImageDestinationFinalize(dest)
    print("wrote \(path)")
}

func solid(_ path: String, size: Int) {
    let space = CGColorSpace(name: CGColorSpace.sRGB)!
    let ctx = CGContext(data: nil, width: size, height: size, bitsPerComponent: 8, bytesPerRow: 0,
                        space: space, bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue)!
    ctx.setFillColor(blue)
    ctx.fill(CGRect(x: 0, y: 0, width: size, height: size))
    let dest = CGImageDestinationCreateWithURL(URL(fileURLWithPath: path) as CFURL, UTType.png.identifier as CFString, 1, nil)!
    CGImageDestinationAddImage(dest, ctx.makeImage()!, nil)
    CGImageDestinationFinalize(dest)
    print("wrote \(path)")
}

let dir = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "mobile/assets"
// iOS masks the corners itself: full-bleed and opaque (App Store rejects alpha in the icon).
render("\(dir)/icon.png", size: 1024, background: .square, opaque: true)
// Android adaptive icon: the mark must stay inside the 66 % safe zone of the 108 dp foreground.
render("\(dir)/android-icon-foreground.png", size: 512, background: .none, scale: 0.55)
solid("\(dir)/android-icon-background.png", size: 512)
render("\(dir)/android-icon-monochrome.png", size: 432, background: .none, scale: 0.55)
render("\(dir)/splash-icon.png", size: 1024, background: .rounded, scale: 1)
render("\(dir)/favicon.png", size: 48, background: .rounded)
