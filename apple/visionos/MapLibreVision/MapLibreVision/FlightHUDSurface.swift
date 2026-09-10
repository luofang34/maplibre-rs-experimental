import CoreGraphics
import IOSurface
import Metal

/// Core Graphics and Metal share these pixels; a slot is reused only beyond the in-flight frame bound.
final class FlightHUDSurface {
    enum Failure: Error { case allocation }
    let surface: IOSurfaceRef
    let texture: MTLTexture
    private(set) var context: CGContext
    static let width = 1200, height = 600, rowBytes = 4800

    init(device: MTLDevice) throws {
        let properties: [CFString: Any] = [
            kIOSurfaceWidth: Self.width, kIOSurfaceHeight: Self.height,
            kIOSurfaceBytesPerElement: 4, kIOSurfaceBytesPerRow: Self.rowBytes,
            kIOSurfaceAllocSize: Self.rowBytes * Self.height,
            kIOSurfacePixelFormat: UInt32(0x52474241)]
        guard let surface = IOSurfaceCreate(properties as CFDictionary),
              let context = Self.makeContext(surface) else { throw Failure.allocation }
        let descriptor = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .rgba8Unorm_srgb,
            width: Self.width, height: Self.height, mipmapped: false)
        descriptor.storageMode = .shared
        descriptor.usage = .shaderRead
        guard let texture = device.makeTexture(descriptor: descriptor, iosurface: surface, plane: 0) else { throw Failure.allocation }
        self.surface = surface
        self.context = context
        self.texture = texture
    }

    func begin() { IOSurfaceLock(surface, [], nil) }
    func end() { IOSurfaceUnlock(surface, [], nil) }

    func resetContext() -> Bool {
        guard let context = Self.makeContext(surface) else { return false }
        self.context = context
        return true
    }

    func clearPixels() {
        memset(IOSurfaceGetBaseAddress(surface), 0, Self.rowBytes * Self.height)
    }

    private static func makeContext(_ surface: IOSurfaceRef) -> CGContext? {
        CGContext(data: IOSurfaceGetBaseAddress(surface), width: width, height: height,
            bitsPerComponent: 8, bytesPerRow: rowBytes,
            space: CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue | CGBitmapInfo.byteOrder32Big.rawValue)
    }
}
