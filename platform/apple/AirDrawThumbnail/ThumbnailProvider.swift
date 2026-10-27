// Quick Look thumbnail extension for .airdraw files (iPadOS + macOS).
//
// Reads only the preview AirDraw embeds at the start of every sketch (format 2+):
//   "ADRW" | u16 version | u16 flags | u32 PNG length | PNG bytes | ...rest of the sketch
// The sketch itself is never parsed or rendered here. Files without a preview make the
// system fall back to the document icon.

import QuickLookThumbnailing

#if canImport(UIKit)
import UIKit
typealias PlatformImage = UIImage
#else
import AppKit
typealias PlatformImage = NSImage
#endif

enum EmbeddedPreview {
    static let maxBytes = 8 << 20

    static func png(at url: URL) -> Data? {
        guard let file = try? FileHandle(forReadingFrom: url) else { return nil }
        defer { try? file.close() }
        guard let head = try? file.read(upToCount: 12), head.count == 12,
              head.prefix(4) == Data("ADRW".utf8) else { return nil }
        let bytes = [UInt8](head)
        let version = UInt16(bytes[4]) | UInt16(bytes[5]) << 8
        guard version >= 2 else { return nil }
        let length = Int(UInt32(bytes[8]) | UInt32(bytes[9]) << 8 | UInt32(bytes[10]) << 16 | UInt32(bytes[11]) << 24)
        guard length > 0, length <= maxBytes,
              let png = try? file.read(upToCount: length), png.count == length else { return nil }
        return png
    }
}

@objc(AirDrawThumbnailProvider)
final class ThumbnailProvider: QLThumbnailProvider {
    override func provideThumbnail(
        for request: QLFileThumbnailRequest,
        _ handler: @escaping (QLThumbnailReply?, Error?) -> Void
    ) {
        guard let data = EmbeddedPreview.png(at: request.fileURL), let image = PlatformImage(data: data) else {
            handler(nil, CocoaError(.fileReadCorruptFile))
            return
        }
        let size = Self.fit(image.size, in: request.maximumSize)
        handler(QLThumbnailReply(contextSize: size, currentContextDrawing: {
            image.draw(in: CGRect(origin: .zero, size: size))
            return true
        }), nil)
    }

    /// Largest size with the image's aspect ratio that fits `bounds`.
    static func fit(_ image: CGSize, in bounds: CGSize) -> CGSize {
        guard image.width > 0, image.height > 0 else { return bounds }
        let scale = min(bounds.width / image.width, bounds.height / image.height)
        return CGSize(width: (image.width * scale).rounded(), height: (image.height * scale).rounded())
    }
}
