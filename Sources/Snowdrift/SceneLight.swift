import AppKit
import ImageIO
import simd

/// The light the snow is seen in: its shadows pick up the colour of the desktop picture along the bottom
/// of the screen, and the whole scene turns warmer at dawn and dusk, cooler and dimmer at night.
final class SceneLight {
    /// Multiplies every colour of the snow.
    private(set) var light = SIMD3<Float>(repeating: 1)
    /// The colour shadows lean toward, and how far.
    private(set) var shadowTint = SIMD4<Float>(repeating: 0)

    private var wallpaper: (url: URL, modified: Date?)?
    private var updatedAt: CFTimeInterval = -.infinity

    func invalidate() {
        updatedAt = -.infinity
    }

    func update(screen: NSScreen?, now: CFTimeInterval) {
        guard now - updatedAt > 60 else { return }
        updatedAt = now
        light = Self.daylight(at: Date())

        guard let screen, let url = NSWorkspace.shared.desktopImageURL(for: screen) else {
            wallpaper = nil
            shadowTint = .zero
            return
        }
        let modified = (try? url.resourceValues(forKeys: [.contentModificationDateKey]))?.contentModificationDate
        guard wallpaper?.url != url || wallpaper?.modified != modified else { return }
        wallpaper = (url, modified)
        shadowTint = Self.tint(fromPictureAt: url) ?? .zero
    }

    /// Clock-based rather than solar: close enough for mood, and needs no location.
    private static func daylight(at date: Date) -> SIMD3<Float> {
        let parts = Calendar.current.dateComponents([.hour, .minute], from: date)
        var hour = Float(parts.hour ?? 12) + Float(parts.minute ?? 0) / 60
        // Lets the look at another time of day be previewed: defaults write com.kirDima-787.Snowdrift previewHour -float 19
        if UserDefaults.standard.object(forKey: "previewHour") != nil {
            hour = UserDefaults.standard.float(forKey: "previewHour")
        }
        func smoothstep(_ a: Float, _ b: Float, _ x: Float) -> Float {
            let t = min(max((x - a) / (b - a), 0), 1)
            return t * t * (3 - 2 * t)
        }
        let day = smoothstep(6, 8.5, hour) * (1 - smoothstep(17.5, 20.5, hour))
        let golden = min(1, exp(-pow((hour - 7.6) / 1.1, 2)) + exp(-pow((hour - 18.6) / 1.2, 2)))
        let night = SIMD3<Float>(0.86, 0.89, 0.97)
        let warm = SIMD3<Float>(1, 0.965, 0.915)
        let base = night + (SIMD3<Float>(repeating: 1) - night) * day
        return base + (warm - base) * golden * 0.55
    }

    /// The average colour of the picture's bottom quarter, brought to the brightness of snow in shade.
    private static func tint(fromPictureAt url: URL) -> SIMD4<Float>? {
        guard let source = CGImageSourceCreateWithURL(url as CFURL, nil),
              let image = CGImageSourceCreateThumbnailAtIndex(source, 0, [
                  kCGImageSourceCreateThumbnailFromImageAlways: true,
                  kCGImageSourceCreateThumbnailWithTransform: true,
                  kCGImageSourceThumbnailMaxPixelSize: 64,
              ] as CFDictionary)
        else { return nil }
        let width = image.width, height = image.height
        var pixels = [UInt8](repeating: 0, count: width * height * 4)
        guard let context = CGContext(
            data: &pixels, width: width, height: height, bitsPerComponent: 8, bytesPerRow: width * 4,
            space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
        ) else { return nil }
        context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))

        // Rows run top-down in memory, so the bottom of the picture is at the end.
        var sum = SIMD3<Float>(repeating: 0)
        var count: Float = 0
        for row in (height * 3 / 4)..<height {
            for column in 0..<width {
                let i = (row * width + column) * 4
                sum += SIMD3(Float(pixels[i]), Float(pixels[i + 1]), Float(pixels[i + 2])) / 255
                count += 1
            }
        }
        guard count > 0 else { return nil }
        let average = sum / count
        let luminance = simd_dot(average, SIMD3(0.2126, 0.7152, 0.0722))
        let shade: Float = 0.775
        let color = simd_min(average * (shade / max(luminance, 0.04)), SIMD3(repeating: 1))
        return SIMD4(color, 0.35)
    }
}
