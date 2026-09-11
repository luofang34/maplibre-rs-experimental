import CompositorServices
import Metal
import simd

final class TrackOverlayRenderer {
    struct Vertex {
        var position: SIMD4<Float>
        var color: SIMD4<Float>
        var capsule: SIMD4<Float> = .zero
    }
    private let pipeline: MTLRenderPipelineState
    private let depth: MTLDepthStencilState
    private var route: FlightRoute?
    private var routeGeneration: UInt64?
    private var vertices: [Vertex] = []
    private var buffers: [[MTLBuffer]] = []
    private var slot = 0
    private let vertexLimit = 1500 * 6 + 180

    init(device: MTLDevice) throws {
        enum SetupError: Error { case shaders, depth }
        guard let library = device.makeDefaultLibrary(),
              let vertex = library.makeFunction(name: "flightTrackVertex"),
              let fragment = library.makeFunction(name: "flightTrackFragment") else { throw SetupError.shaders }
        let descriptor = MTLRenderPipelineDescriptor()
        descriptor.vertexFunction = vertex
        descriptor.fragmentFunction = fragment
        let color = descriptor.colorAttachments[0]
        color?.pixelFormat = .bgra8Unorm
        color?.isBlendingEnabled = true
        color?.sourceRGBBlendFactor = .one
        color?.destinationRGBBlendFactor = .oneMinusSourceAlpha
        color?.sourceAlphaBlendFactor = .one
        color?.destinationAlphaBlendFactor = .oneMinusSourceAlpha
        descriptor.depthAttachmentPixelFormat = .depth32Float
        pipeline = try device.makeRenderPipelineState(descriptor: descriptor)
        let state = MTLDepthStencilDescriptor()
        state.depthCompareFunction = .greaterEqual
        state.isDepthWriteEnabled = false
        guard let depth = device.makeDepthStencilState(descriptor: state) else { throw SetupError.depth }
        self.depth = depth
        vertices.reserveCapacity(vertexLimit)
        buffers = try (0..<3).map { _ in
            try (0..<2).map { _ in
                guard let buffer = device.makeBuffer(length: vertexLimit * MemoryLayout<Vertex>.stride, options: .storageModeShared) else { throw SetupError.depth }
                return buffer
            }
        }
    }

    func draw(track: FlightTrack, observation: FlightTrack.Observation?, placement: MapPlacement,
              drawable: LayerRenderer.Drawable, head: simd_float4x4,
              colors: [MTLTexture], depths: [MTLTexture?], command: MTLCommandBuffer, fpv: Bool = false, elapsed: Double = 0, generation: UInt64 = 0) {
        if routeGeneration != generation {
            route = FlightRoute(track: track)
            routeGeneration = generation
        }
        let points = (route?.segments ?? []).map { segment in
            (SIMD4<Float>(SIMD3<Float>(placement.roomPoint(for: segment.start)), 1),
             SIMD4<Float>(SIMD3<Float>(placement.roomPoint(for: segment.end)), 1), segment.time)
        }
        slot = (slot + 1) % buffers.count
        for (index, view) in drawable.views.enumerated() {
            let projection = drawable.computeProjection(viewIndex: index) * simd_inverse(head * view.transform)
            let size = SIMD2<Float>(Float(colors[index].width), Float(colors[index].height))
            vertices.removeAll(keepingCapacity: true)
            for segment in points {
                let a = projection * segment.0, b = projection * segment.1
                let flown = segment.2 <= elapsed
                appendSegment(a, b, width: flown ? 2.4 : 1.4, size: size,
                              color: flown ? [1, 0.55, 0.1, 1] : [0.2, 0.6, 0.7, 1], vertices: &vertices)
            }
            if let observation, !fpv { appendAircraft(observation, placement: placement, projection: projection, size: size, focalY: drawable.computeProjection(viewIndex: index).columns.1.y, vertices: &vertices) }
            guard !vertices.isEmpty, vertices.count <= vertexLimit, index < buffers[slot].count else { continue }
            let buffer = buffers[slot][index]
            vertices.withUnsafeBytes { bytes in
                if let base = bytes.baseAddress { buffer.contents().copyMemory(from: base, byteCount: bytes.count) }
            }
            let pass = MTLRenderPassDescriptor()
            pass.colorAttachments[0].texture = colors[index]
            pass.colorAttachments[0].loadAction = .load
            pass.colorAttachments[0].storeAction = .store
            pass.depthAttachment.texture = depths[index]
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

    private func appendSegment(_ a: SIMD4<Float>, _ b: SIMD4<Float>, width: Float,
                               size: SIMD2<Float>, color: SIMD4<Float>, vertices: inout [Vertex]) {
        FlightStroke.forEachCorner(a, b, width: width, viewport: size) { corner in
            vertices.append(Vertex(position: corner.position, color: color, capsule: corner.capsule))
        }
    }

    private func appendAircraft(_ observation: FlightTrack.Observation, placement: MapPlacement,
                                projection: simd_float4x4, size: SIMD2<Float>, focalY: Float, vertices: inout [Vertex]) {
        let origin = placement.roomPoint(for: observation.coordinate)
        let clip = projection * SIMD4<Float>(SIMD3<Float>(origin), 1)
        guard clip.w > 0.01, clip.z >= 0 else { return }
        let metersPerPixel = Double(2 * clip.w / max(abs(focalY), 0.001) / size.y)
        let points = FlightOwnshipGeometry.vertices(observation: observation, placement: placement,
                                                    radius: metersPerPixel * 12)
        for point in points {
            let projected = projection * SIMD4<Float>(SIMD3<Float>(point), 1)
            guard projected.w > 0.01 else { return }
            vertices.append(Vertex(position: projected, color: [1, 0.8, 0.2, 1]))
        }
    }
}
