#!/usr/bin/env bash
# Regenerate packaging/macos/AppIcon.icns from packaging/macos/icon.svg.
# Needs macOS 13+ (AppKit renders SVG) and the Xcode Command Line Tools (swift).
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
svg="$root/packaging/macos/icon.svg"
out="$root/packaging/macos/AppIcon.icns"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
iconset="$work/AppIcon.iconset"
mkdir -p "$iconset"

cat > "$work/render.swift" <<'SWIFT'
import AppKit

let args = CommandLine.arguments
let svg = URL(fileURLWithPath: args[1])
let dir = URL(fileURLWithPath: args[2])
guard let image = NSImage(contentsOf: svg) else { fatalError("cannot load \(svg.path)") }

// (pixel size, iconset file name)
let sizes: [(Int, String)] = [
    (16, "icon_16x16"), (32, "icon_16x16@2x"),
    (32, "icon_32x32"), (64, "icon_32x32@2x"),
    (128, "icon_128x128"), (256, "icon_128x128@2x"),
    (256, "icon_256x256"), (512, "icon_256x256@2x"),
    (512, "icon_512x512"), (1024, "icon_512x512@2x"),
]
for (px, name) in sizes {
    let rep = NSBitmapImageRep(
        bitmapDataPlanes: nil, pixelsWide: px, pixelsHigh: px, bitsPerSample: 8,
        samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
        colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
    NSGraphicsContext.current?.imageInterpolation = .high
    image.draw(in: NSRect(x: 0, y: 0, width: px, height: px))
    NSGraphicsContext.restoreGraphicsState()
    let png = rep.representation(using: .png, properties: [:])!
    try! png.write(to: dir.appendingPathComponent("\(name).png"))
}
SWIFT

swift "$work/render.swift" "$svg" "$iconset"
iconutil -c icns "$iconset" -o "$out"
echo "Wrote $out"
