import CoreGraphics
import XCTest
@testable import MapInteraction

final class FlightHUDContrastTests: XCTestCase {
    func testBrightStrokeHasLocalDarkHaloAndTransparentSurroundings() throws {
        let context = try XCTUnwrap(CGContext(data: nil, width: 32, height: 32,
            bitsPerComponent: 8, bytesPerRow: 128, space: CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue | CGBitmapInfo.byteOrder32Big.rawValue))
        FlightHUDContrast.apply(to: context)
        context.setFillColor(CGColor(red: 0.25, green: 1, blue: 0.4, alpha: 1))
        context.fill(CGRect(x: 15, y: 8, width: 2, height: 16))
        let pixels = try XCTUnwrap(context.data).assumingMemoryBound(to: UInt8.self)
        let center = 16 * 128 + 16 * 4, edge = 16 * 128 + 14 * 4
        XCTAssertGreaterThan(pixels[center + 1], 200)
        XCTAssertGreaterThan(pixels[edge + 3], 10)
        XCTAssertLessThan(pixels[edge + 1], 10)
        XCTAssertEqual(pixels[3], 0)
    }
}
