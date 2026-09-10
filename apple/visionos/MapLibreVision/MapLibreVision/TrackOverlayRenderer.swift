import CompositorServices
import Metal
import simd

final class TrackOverlayRenderer {
    struct Vertex {
        var position: SIMD4<Float>
        var color: SIMD4<Float>
    }
    private let pipeline: MTLRenderPipelineState
    private let depth: MTLDepthStencilState
    private var route: FlightRoute?
    private var routeGeneration: UInt64?

    init(device: MTLDevice) throws {
        enum SetupError: Error { case shaders, depth }
        guard let library = device.makeDefaultLibrary(),
              let vertex = library.makeFunction(name: "flightTrackVertex"),
              let fragment = library.makeFunction(name: "flightTrackFragment") else { throw SetupError.shaders }
        let descriptor = MTLRenderPipelineDescriptor()
        descriptor.vertexFunction = vertex
        descriptor.fragmentFunction = fragment
        descriptor.colorAttachments[0].pixelFormat = .bgra8Unorm
        descriptor.depthAttachmentPixelFormat = .depth32Float
        pipeline = try device.makeRenderPipelineState(descriptor: descriptor)
        let state = MTLDepthStencilDescriptor()
        state.depthCompareFunction = .greaterEqual
        state.isDepthWriteEnabled = false
        guard let depth = device.makeDepthStencilState(descriptor: state) else { throw SetupError.depth }
        self.depth = depth
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
        for (index, view) in drawable.views.enumerated() {
            let projection = drawable.computeProjection(viewIndex: index) * simd_inverse(head * view.transform)
            let size = SIMD2<Float>(Float(colors[index].width), Float(colors[index].height))
            var vertices: [Vertex] = []
            vertices.reserveCapacity(points.count * 6 + 180)
            for segment in points {
                let a = projection * segment.0, b = projection * segment.1
                let flown = segment.2 <= elapsed
                appendSegment(a, b, width: flown ? 2.4 : 1.4, size: size,
                              color: flown ? [1, 0.55, 0.1, 1] : [0.2, 0.6, 0.7, 1], vertices: &vertices)
            }
            if fpv { appendFlightPath(track: track, time: elapsed, placement: placement, projection: projection, size: size, vertices: &vertices) }
            if let observation, !fpv { appendAircraft(observation, placement: placement, projection: projection, size: size, vertices: &vertices) }
            guard !vertices.isEmpty, let buffer = vertices.withUnsafeBytes({ bytes in
                bytes.baseAddress.flatMap { command.device.makeBuffer(bytes: $0, length: bytes.count) }
            }) else { continue }
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

    private func appendFlightPath(track: FlightTrack, time: Double, placement: MapPlacement,
                                  projection: simd_float4x4, size: SIMD2<Float>, vertices: inout [Vertex]) {
        guard let point = track.sample(at: time), let vertical = track.verticalSpeed(at: time),
              point.hasVelocity, point.groundSpeed > 1 else { return }
        let distance = 2000.0, angle = point.track * .pi / 180
        let ahead = MapAnchor(latitude: point.latitude + cos(angle) * distance / MapPlacement.earthRadiusMeters * 180 / .pi,
            longitude: point.longitude + sin(angle) * distance / (MapPlacement.earthRadiusMeters * cos(point.latitude * .pi / 180)) * 180 / .pi,
            altitudeMeters: point.altitudeMSL + vertical * distance / point.groundSpeed)
        let center = projection * SIMD4<Float>(SIMD3<Float>(placement.roomPoint(for: ahead)), 1)
        guard center.w > 0.01, abs(center.x / center.w) < 0.9, abs(center.y / center.w) < 0.85 else { return }
        let radius: Float = 11
        let offset = { (x: Float, y: Float) in center + SIMD4<Float>(x * 2 / size.x * center.w, y * 2 / size.y * center.w, 0, 0) }
        for i in 0..<24 {
            let a = Float(i) * 2 * .pi / 24, b = Float(i + 1) * 2 * .pi / 24
            appendSegment(offset(cos(a) * radius, sin(a) * radius), offset(cos(b) * radius, sin(b) * radius),
                          width: 1.5, size: size, color: [0.3, 1, 0.45, 1], vertices: &vertices)
        }
        let wings: [(SIMD2<Float>, SIMD2<Float>)] = [([-23, 0], [-11, 0]), ([11, 0], [23, 0]), ([0, 11], [0, 19])]
        for (a, b) in wings {
            appendSegment(offset(a.x, a.y), offset(b.x, b.y), width: 1.5, size: size, color: [0.3, 1, 0.45, 1], vertices: &vertices)
        }
    }

    private func appendSegment(_ a: SIMD4<Float>, _ b: SIMD4<Float>, width: Float,
                               size: SIMD2<Float>, color: SIMD4<Float>, vertices: inout [Vertex]) {
        // Segments through the near plane cannot be expanded safely in screen space.
        guard a.w > 0.01, b.w > 0.01, a.z >= 0, b.z >= 0 else { return }
        let delta = (SIMD2(b.x, b.y) / b.w - SIMD2(a.x, a.y) / a.w) * size
        guard simd_length_squared(delta) > 0.001 else { return }
        let normal = simd_normalize(SIMD2(-delta.y, delta.x)) * width / size
        let offsetA = SIMD4<Float>(normal.x * a.w, normal.y * a.w, 0, 0)
        let offsetB = SIMD4<Float>(normal.x * b.w, normal.y * b.w, 0, 0)
        for point in [a - offsetA, b - offsetB, a + offsetA, a + offsetA, b - offsetB, b + offsetB] {
            vertices.append(Vertex(position: point, color: color))
        }
    }

    private func appendAircraft(_ observation: FlightTrack.Observation, placement: MapPlacement,
                                projection: simd_float4x4, size: SIMD2<Float>, vertices: inout [Vertex]) {
        let origin = placement.roomPoint(for: observation.coordinate)
        let clip = projection * SIMD4<Float>(SIMD3<Float>(origin), 1)
        guard clip.w > 0.01, clip.z >= 0 else { return }
        let heading = observation.track * .pi / 180
        let ahead = MapAnchor(latitude: observation.latitude + cos(heading) * 0.001,
            longitude: observation.longitude + sin(heading) * 0.001 / cos(observation.latitude * .pi / 180),
            altitudeMeters: observation.altitudeMSL)
        let nose = projection * SIMD4<Float>(SIMD3<Float>(placement.roomPoint(for: ahead)), 1)
        let delta = nose.w > 0.01 ? (SIMD2(nose.x, nose.y) / nose.w - SIMD2(clip.x, clip.y) / clip.w) * size : .zero
        let up = simd_length_squared(delta) > 0.0001 ? simd_normalize(delta) : SIMD2<Float>(0, 1)
        let right = SIMD2(up.y, -up.x)
        // A screen-sized locator remains readable; it does not pretend to be aircraft attitude.
        let scale = SIMD2<Float>(18 / size.x, 18 / size.y) * clip.w
        for offset in [SIMD2<Float>(0, 1.5), [-1, -1], [0, -0.5], [0, 1.5], [0, -0.5], [1, -1]] {
            let rotated = right * offset.x + up * offset.y
            vertices.append(Vertex(position: clip + SIMD4<Float>(rotated.x * scale.x, rotated.y * scale.y, 0, 0),
                                   color: [1, 0.8, 0.2, 1]))
        }
    }
}
