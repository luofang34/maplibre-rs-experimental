import CompositorServices
import Metal
import simd

final class FlightSymbolRenderer {
    private struct Vertex { let position: SIMD4<Float>; let color: SIMD4<Float>; let capsule: SIMD4<Float> }
    private let pipeline: MTLRenderPipelineState
    private var buffers: [[MTLBuffer]] = []
    private var vertices: [Vertex] = []
    private var slot = 0
    private let capacity = 1024 * 6
    private var strokes: [AngularStroke] = []

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
        pipeline = try device.makeRenderPipelineState(descriptor: descriptor)
        buffers = try (0..<3).map { _ in try (0..<2).map { _ in
            guard let buffer = device.makeBuffer(length: capacity * MemoryLayout<Vertex>.stride, options: .storageModeShared) else { throw SetupError.resource }
            return buffer
        } }
        vertices.reserveCapacity(capacity)
    }

    func prepare(telemetry: ReplayTelemetry) {
        var scene = indicate_svs_directions(telemetry)
        let count = min(Int(scene.length), 1024)
        strokes.removeAll(keepingCapacity: true)
        withUnsafeBytes(of: &scene.strokes) {
            strokes.append(contentsOf: $0.bindMemory(to: AngularStroke.self).prefix(count))
        }
        slot = (slot + 1) % buffers.count
    }

    func encode(placement: MapPlacement, head: simd_float4x4,
                drawable: LayerRenderer.Drawable, index: Int, encoder: MTLRenderCommandEncoder) {
        guard index < 2 else { return }
        let eye = drawable.views[index]
        let projection = drawable.computeProjection(viewIndex: index)
        let rotation = simd_float4x4(simd_quatf(vector: SIMD4<Float>(placement.current.rotation.vector)))
        let transform = projection * simd_inverse(head * eye.transform) * rotation
        let texture = drawable.colorTextures[index]
        let size = SIMD2<Float>(Float(texture.width), Float(texture.height))
        vertices.removeAll(keepingCapacity: true)
        for stroke in strokes {
            append(stroke, transform: transform, size: size)
        }
        guard !vertices.isEmpty, vertices.count <= capacity else { return }
        let buffer = buffers[slot][index]
        vertices.withUnsafeBytes { if let base = $0.baseAddress { buffer.contents().copyMemory(from: base, byteCount: $0.count) } }
        encoder.setRenderPipelineState(pipeline)
        encoder.setVertexBuffer(buffer, offset: 0, index: 0)
        // All halos precede all cores, so a later segment cannot darken a joined stroke.
        for value: UInt32 in [0, 1] {
            var pass = value
            encoder.setFragmentBytes(&pass, length: MemoryLayout<UInt32>.size, index: 0)
            encoder.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: vertices.count)
        }
    }

    private func append(_ stroke: AngularStroke, transform: simd_float4x4, size: SIMD2<Float>) {
        // Directions have no translation or binocular parallax: these cues are collimated.
        var a = transform * SIMD4<Float>(SIMD3<Float>(stroke.a.0, stroke.a.1, stroke.a.2), 0)
        var b = transform * SIMD4<Float>(SIMD3<Float>(stroke.b.0, stroke.b.1, stroke.b.2), 0)
        a.z = 0; b.z = 0
        FlightStroke.forEachCorner(a, b, width: 3, viewport: size) { corner in
            vertices.append(.init(position: corner.position, color: [0.25, 1, 0.4, 1], capsule: corner.capsule))
        }
    }
}
