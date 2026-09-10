import CompositorServices
import Metal
import simd

final class FlightSymbolRenderer {
    private struct Vertex { let position: SIMD4<Float>; let color: SIMD4<Float>; let edge: Float }
    private let pipeline: MTLRenderPipelineState
    private let depth: MTLDepthStencilState
    private var buffers: [[MTLBuffer]] = []
    private var vertices: [Vertex] = []
    private var slot = 0
    private let capacity = 12_000

    init(device: MTLDevice) throws {
        enum SetupError: Error { case resource }
        let descriptor = MTLRenderPipelineDescriptor()
        let library = device.makeDefaultLibrary()
        descriptor.vertexFunction = library?.makeFunction(name: "flightSymbolVertex")
        descriptor.fragmentFunction = library?.makeFunction(name: "flightSymbolFragment")
        descriptor.colorAttachments[0].pixelFormat = .bgra8Unorm_srgb
        descriptor.colorAttachments[0].isBlendingEnabled = true
        descriptor.colorAttachments[0].sourceRGBBlendFactor = .one
        descriptor.colorAttachments[0].destinationRGBBlendFactor = .oneMinusSourceAlpha
        descriptor.colorAttachments[0].sourceAlphaBlendFactor = .one
        descriptor.colorAttachments[0].destinationAlphaBlendFactor = .oneMinusSourceAlpha
        descriptor.depthAttachmentPixelFormat = .depth32Float
        pipeline = try device.makeRenderPipelineState(descriptor: descriptor)
        let state = MTLDepthStencilDescriptor()
        state.depthCompareFunction = .always
        state.isDepthWriteEnabled = true
        guard let depth = device.makeDepthStencilState(descriptor: state) else { throw SetupError.resource }
        self.depth = depth
        buffers = try (0..<3).map { _ in try (0..<2).map { _ in
            guard let buffer = device.makeBuffer(length: capacity * MemoryLayout<Vertex>.stride, options: .storageModeShared) else { throw SetupError.resource }
            return buffer
        } }
        vertices.reserveCapacity(capacity)
    }

    func draw(frame: FlightReplay.Frame, placement: MapPlacement, head: simd_float4x4,
              drawable: LayerRenderer.Drawable, command: MTLCommandBuffer) {
        guard frame.view == .fpv, let point = frame.observation else { return }
        let geometry = FlightHUDGeometry(point: point, verticalSpeed: frame.track?.verticalSpeed(at: frame.elapsed))
        let strokes = geometry.references() + geometry.marker(.prograde) + geometry.marker(.retrograde)
        slot = (slot + 1) % buffers.count
        for (index, eye) in drawable.views.enumerated() where index < 2 {
            let projection = drawable.computeProjection(viewIndex: index)
            let rotation = simd_float4x4(simd_quatf(vector: SIMD4<Float>(placement.current.rotation.vector)))
            let transform = projection * simd_inverse(head * eye.transform) * rotation
            let texture = drawable.colorTextures[index]
            let size = SIMD2<Float>(Float(texture.width), Float(texture.height))
            vertices.removeAll(keepingCapacity: true)
            for (width, color): (Float, SIMD4<Float>) in [(3.0, [0, 0.04, 0, 0.8]), (1.6, [0.25, 1, 0.4, 1])] {
                for stroke in strokes {
                    append(stroke, transform: transform, size: size, width: width, color: color)
                }
            }
            guard !vertices.isEmpty, vertices.count <= capacity else { continue }
            let buffer = buffers[slot][index]
            vertices.withUnsafeBytes { if let base = $0.baseAddress { buffer.contents().copyMemory(from: base, byteCount: $0.count) } }
            let pass = MTLRenderPassDescriptor()
            pass.colorAttachments[0].texture = texture
            pass.colorAttachments[0].loadAction = .load
            pass.colorAttachments[0].storeAction = .store
            pass.depthAttachment.texture = drawable.depthTextures[index]
            pass.depthAttachment.loadAction = .load
            pass.depthAttachment.storeAction = .store
            guard let encoder = command.makeRenderCommandEncoder(descriptor: pass) else { continue }
            encoder.setRenderPipelineState(pipeline)
            encoder.setDepthStencilState(depth)
            encoder.setVertexBuffer(buffer, offset: 0, index: 0)
            encoder.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: vertices.count)
            encoder.endEncoding()
        }
    }

    private func append(_ stroke: FlightHUDGeometry.Stroke, transform: simd_float4x4,
                        size: SIMD2<Float>, width: Float, color: SIMD4<Float>) {
        // Directions have no translation or binocular parallax: these cues are collimated.
        var a = transform * SIMD4<Float>(SIMD3<Float>(stroke.a), 0)
        var b = transform * SIMD4<Float>(SIMD3<Float>(stroke.b), 0)
        guard a.w > 0.05, b.w > 0.05 else { return }
        a.z = 0; b.z = 0
        let delta = (SIMD2(b.x, b.y) / b.w - SIMD2(a.x, a.y) / a.w) * size
        guard simd_length_squared(delta) > 0.001 else { return }
        let normal = simd_normalize(SIMD2(-delta.y, delta.x)) * (width + 1) / size
        let da = SIMD4<Float>(normal.x * a.w, normal.y * a.w, 0, 0)
        let db = SIMD4<Float>(normal.x * b.w, normal.y * b.w, 0, 0)
        for (p, edge): (SIMD4<Float>, Float) in [(a-da,-1), (b-db,-1), (a+da,1), (a+da,1), (b-db,-1), (b+db,1)] {
            vertices.append(.init(position: p, color: color, edge: edge))
        }
    }
}
