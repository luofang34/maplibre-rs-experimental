import Metal
import simd
import XCTest
@testable import MapInteraction

final class FlightSymbolAATests: XCTestCase {
    private struct Vertex { let position: SIMD4<Float>; let color: SIMD4<Float>; let capsule: SIMD4<Float> }

    func testHaloHasSmoothCapsAndJoinsKeepTheirGreenCore() throws {
        let halo = try render(passes: [0], crossing: false)
        let full = try render(passes: [0, 1], crossing: true)
        // This pixel is before the endpoint; a hard butt cap leaves it white.
        let cap = (32 * 64 + 14) * 4
        XCTAssertLessThan(halo[cap], 255)
        XCTAssertGreaterThan(halo[cap], 60)
        let edge = (33 * 64 + 32) * 4
        XCTAssertGreaterThan(halo[edge], 60)
        XCTAssertLessThan(halo[edge], 240)
        let outside = (38 * 64 + 32) * 4
        XCTAssertEqual(halo[outside], 255)
        let joint = (32 * 64 + 32) * 4
        XCTAssertGreaterThan(full[joint + 1], 240)
        XCTAssertLessThan(full[joint], 120)
    }

    private func render(passes: [UInt32], crossing: Bool) throws -> [UInt8] {
        guard let device = MTLCreateSystemDefaultDevice() else { throw XCTSkip("Metal device unavailable") }
        let shader = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("MapLibreVision/FlightTrack.metal")
        let library = try device.makeLibrary(source: String(contentsOf: shader, encoding: .utf8), options: nil)
        let descriptor = MTLRenderPipelineDescriptor()
        descriptor.vertexFunction = library.makeFunction(name: "flightSymbolVertex")
        descriptor.fragmentFunction = library.makeFunction(name: "flightSymbolFragment")
        let color = try XCTUnwrap(descriptor.colorAttachments[0])
        color.pixelFormat = .rgba8Unorm
        color.isBlendingEnabled = true
        color.sourceRGBBlendFactor = .one
        color.destinationRGBBlendFactor = .oneMinusSourceAlpha
        color.sourceAlphaBlendFactor = .one
        color.destinationAlphaBlendFactor = .oneMinusSourceAlpha
        let pipeline = try device.makeRenderPipelineState(descriptor: descriptor)
        let target = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .rgba8Unorm, width: 64, height: 64, mipmapped: false)
        target.storageMode = .shared
        target.usage = .renderTarget
        let texture = try XCTUnwrap(device.makeTexture(descriptor: target))
        let command = try XCTUnwrap(device.makeCommandQueue()?.makeCommandBuffer())
        let pass = MTLRenderPassDescriptor()
        pass.colorAttachments[0].texture = texture
        pass.colorAttachments[0].loadAction = .clear
        pass.colorAttachments[0].clearColor = MTLClearColorMake(1, 1, 1, 1)
        pass.colorAttachments[0].storeAction = .store
        let encoder = try XCTUnwrap(command.makeRenderCommandEncoder(descriptor: pass))
        encoder.setRenderPipelineState(pipeline)
        let vertices = geometry(crossing: crossing)
        vertices.withUnsafeBytes { encoder.setVertexBytes($0.baseAddress!, length: $0.count, index: 0) }
        for value in passes {
            var mode = value
            encoder.setFragmentBytes(&mode, length: MemoryLayout<UInt32>.size, index: 0)
            encoder.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: vertices.count)
        }
        encoder.endEncoding()
        command.commit()
        command.waitUntilCompleted()
        XCTAssertEqual(command.status, .completed)
        var pixels = [UInt8](repeating: 0, count: 64 * 64 * 4)
        texture.getBytes(&pixels, bytesPerRow: 64 * 4, from: MTLRegionMake2D(0, 0, 64, 64), mipmapLevel: 0)
        return pixels
    }

    private func geometry(crossing: Bool) -> [Vertex] {
        func clip(_ x: Float, _ y: Float) -> SIMD4<Float> { [x / 32 - 1, 1 - y / 32, 0, 1] }
        var vertices: [Vertex] = []
        let segments = crossing ? [(clip(16.4, 32.2), clip(48.4, 32.2)), (clip(32.2, 16.4), clip(32.2, 48.4))]
                                : [(clip(16.4, 32.2), clip(48.4, 32.2))]
        for (a, b) in segments {
            FlightStroke.forEachCorner(a, b, width: 3, viewport: [64, 64]) {
                vertices.append(Vertex(position: $0.position, color: [0.25, 1, 0.4, 1], capsule: $0.capsule))
            }
        }
        return vertices
    }
}
