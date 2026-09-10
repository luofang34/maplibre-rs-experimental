import CompositorServices
import CoreGraphics
import CoreText
import UIKit
import IndicateAppleDisplay
import Metal
import simd
import os

struct IndicateGlyphAtlas: GlyphAtlas {
    let cellWidth = 5, cellHeight = 7, advance = 6
    func rows(for scalar: UInt32) -> [UInt8]? {
        let packed = indicate_svs_glyph(scalar)
        guard packed & (1 << 63) != 0 else { return nil }
        return (0..<7).map { UInt8(truncatingIfNeeded: packed >> ($0 * 8)) }
    }
}

final class FlightHUDRenderer {
    private struct Vertex { let position: SIMD4<Float>; let uv: SIMD2<Float> }
    private let renderer = SceneRenderer(atlas: IndicateGlyphAtlas())
    private let pipeline: MTLRenderPipelineState
    private let depth: MTLDepthStencilState
    private var slots: [FlightHUDSurface]
    private var slot = 0
    private var nextUpdate = 0.0
    private var revision: UInt64?
    private var lastValid = false
    private var lastElapsed = -1.0
    private var lastCaption = ""
    private(set) var renderMilliseconds = 0.0
    private var timings: [Double] = []
    private let logger = Logger(subsystem: "com.sokolysystems.maplibre.vision", category: "FlightHUD")

    init(device: MTLDevice) throws {
        enum SetupError: Error { case resources }
        let descriptor = MTLRenderPipelineDescriptor()
        let library = device.makeDefaultLibrary()
        descriptor.vertexFunction = library?.makeFunction(name: "flightHUDVertex")
        descriptor.fragmentFunction = library?.makeFunction(name: "flightHUDFragment")
        let color = descriptor.colorAttachments[0]
        color?.pixelFormat = .bgra8Unorm_srgb
        color?.isBlendingEnabled = true
        color?.sourceRGBBlendFactor = .one
        color?.destinationRGBBlendFactor = .oneMinusSourceAlpha
        color?.sourceAlphaBlendFactor = .one
        color?.destinationAlphaBlendFactor = .oneMinusSourceAlpha
        descriptor.depthAttachmentPixelFormat = .depth32Float
        pipeline = try device.makeRenderPipelineState(descriptor: descriptor)
        let state = MTLDepthStencilDescriptor()
        state.depthCompareFunction = .always
        state.isDepthWriteEnabled = true
        guard let depth = device.makeDepthStencilState(descriptor: state) else { throw SetupError.resources }
        self.depth = depth
        slots = try (0..<3).map { _ in try FlightHUDSurface(device: device) }
    }

    func draw(frame: FlightReplay.Frame, head: simd_float4x4, terrainValid: Bool, boarding: Bool,
              drawable: LayerRenderer.Drawable, command: MTLCommandBuffer) {
        guard frame.view == .fpv || frame.returnSeconds != nil else { return }
        let now = ProcessInfo.processInfo.systemUptime
        let caption = "\(frame.playing)-\(boarding)-\(frame.returnSeconds ?? -1)"
        if (now >= nextUpdate && (lastElapsed != frame.elapsed || lastCaption != caption)) || revision != frame.cameraRevision || lastValid != terrainValid {
            // Readouts derive from the terrain update's snapshot. The texture moves with the
            // display each frame; text raster work is bounded independently of refresh rate.
            update(frame, terrainValid: terrainValid, boarding: boarding)
            nextUpdate = now + 1.0 / 20
            revision = frame.cameraRevision
            lastValid = terrainValid
            lastElapsed = frame.elapsed
            lastCaption = caption
        }
        for (index, view) in drawable.views.enumerated() {
            let projection = drawable.computeProjection(viewIndex: index) * simd_inverse(head * view.transform) * head
            let corners: [(SIMD4<Float>, SIMD2<Float>)] = [
                ([-1.2, 0.6, -2, 1], [0, 0]), ([-1.2, -0.6, -2, 1], [0, 1]), ([1.2, 0.6, -2, 1], [1, 0]),
                ([1.2, 0.6, -2, 1], [1, 0]), ([-1.2, -0.6, -2, 1], [0, 1]), ([1.2, -0.6, -2, 1], [1, 1])]
            let vertices = corners.map { Vertex(position: projection * $0.0, uv: $0.1) }
            let pass = MTLRenderPassDescriptor()
            pass.colorAttachments[0].texture = drawable.colorTextures[index]
            pass.colorAttachments[0].loadAction = .load
            pass.colorAttachments[0].storeAction = .store
            pass.depthAttachment.texture = drawable.depthTextures[index]
            pass.depthAttachment.loadAction = .load
            pass.depthAttachment.storeAction = .store
            guard let encoder = command.makeRenderCommandEncoder(descriptor: pass) else { continue }
            encoder.setRenderPipelineState(pipeline)
            encoder.setDepthStencilState(depth)
            vertices.withUnsafeBytes { bytes in
                if let base = bytes.baseAddress { encoder.setVertexBytes(base, length: bytes.count, index: 0) }
            }
            encoder.setFragmentTexture(slots[slot].texture, index: 0)
            encoder.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 6)
            encoder.endEncoding()
        }
    }

    private func update(_ frame: FlightReplay.Frame, terrainValid: Bool, boarding: Bool) {
        let start = ProcessInfo.processInfo.systemUptime
        var input = ReplayTelemetry()
        if terrainValid, !boarding, let point = frame.observation {
            input.present = 1 | (point.hasVelocity ? 32 : 0)
            input.ground_speed = Float(point.groundSpeed)
            input.track = Float(point.track * .pi / 180)
            input.altitude_msl = Float(point.altitudeMSL)
            if let vertical = frame.track?.verticalSpeed(at: frame.elapsed) {
                input.present |= 2
                input.vertical_speed = Float(vertical)
            }
            if let ias = point.indicatedAirspeed { input.present |= 4; input.ias = Float(ias) }
            if point.hasAttitude {
                input.present |= 8
                input.roll = Float((point.roll ?? 0) * .pi / 180)
                input.pitch = Float((point.pitch ?? 0) * .pi / 180)
            }
            if let heading = point.heading {
                input.present |= 16
                input.heading = Float(heading * .pi / 180)
            }
        }
        var scene = indicate_svs_render(input)
        let length = scene.length <= 8192 ? Int(scene.length) : 0
        let bytes = withUnsafeBytes(of: &scene) { Array($0.dropFirst(4).prefix(length)) }
        slot = (slot + 1) % slots.count
        let surface = slots[slot]
        surface.begin()
        defer { surface.end() }
        var context = surface.context
        context.clear(CGRect(x: 0, y: 0, width: 1200, height: 600))
        context.saveGState()
        context.translateBy(x: 0, y: 600)
        context.scaleBy(x: 1, y: -1)
        do {
            if frame.view == .fpv {
                let report = try renderer.render(bytes, into: context)
                guard report.unknownOpcodes == 0, report.layersPresent.contains(.tapes),
                      report.layersPresent.contains(.annunciation) else { throw ProducerFault(reason: .paintFailed) }
            }
            drawContext(frame, terrainValid: terrainValid, boarding: boarding, context: context)
        } catch {
            // A failed layer may leave its clip/transform stack open. Discard the context
            // before painting a failure indication so that error state cannot clip it away.
            guard surface.resetContext() else { surface.clearPixels(); return }
            context = surface.context
            context.saveGState()
            context.translateBy(x: 0, y: 600)
            context.scaleBy(x: 1, y: -1)
            FailurePage.draw(into: context, pixelWidth: 1200, pixelHeight: 600, reason: .paintFailed)
        }
        context.restoreGState()
        renderMilliseconds = (ProcessInfo.processInfo.systemUptime - start) * 1000
        timings.append(renderMilliseconds)
        if timings.count == 200 {
            let sorted = timings.sorted()
            logger.info("Indicate overlay CPU p95=\(sorted[189])ms max=\(sorted[199])ms samples=200")
            timings.removeAll(keepingCapacity: true)
        }
    }

    private func drawContext(_ frame: FlightReplay.Frame, terrainValid: Bool, boarding: Bool, context: CGContext) {
        // Playback state belongs to the host; it must never look like a live sensor annunciation.
        let status = !terrainValid ? "VIEW UNAVAILABLE" : boarding ? "BOARDING" : frame.observation == nil ? "TRACK DATA GAP" : frame.playing ? "FPV REPLAY" : "REPLAY PAUSED"
        let caption = frame.track?.isSimulation == true ? "SIMULATED" : "RECORDED"
        let attributes: [NSAttributedString.Key: Any] = [
            .font: CTFontCreateWithName("Menlo-Bold" as CFString, 16, nil),
            .foregroundColor: CGColor(red: 1, green: 0.8, blue: 0.25, alpha: 1)]
        context.saveGState()
        context.translateBy(x: 40, y: 32)
        context.scaleBy(x: 1, y: -1)
        let lines = frame.returnSeconds.map { ["FREE LOOK - RETURN IN \($0)s", "Pinch to open controls / Stay free"] } ?? [status, caption]
        for (row, text) in lines.enumerated() {
            context.textPosition = CGPoint(x: 0, y: -CGFloat(row) * 24)
            CTLineDraw(CTLineCreateWithAttributedString(NSAttributedString(string: text, attributes: attributes)), context)
        }
        context.restoreGState()
    }
}
