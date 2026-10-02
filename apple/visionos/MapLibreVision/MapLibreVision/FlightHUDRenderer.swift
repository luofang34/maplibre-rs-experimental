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
    private let layout = indicate_svs_display_contract()
    private let renderer = SceneRenderer(atlas: IndicateGlyphAtlas())
    private let pipeline: MTLRenderPipelineState
    private var slots: [FlightHUDSurface]
    private var slot = 0
    private var cadence = FlightHUDCadence()
    private var lastScene: [UInt8] = []
    private var lastContext = ""
    private var revision: UInt64?
    private var lastValid = false
    private var lastElapsed = -1.0
    private var lastCaption = ""
    private var glancing = false
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
        pipeline = try device.makeRenderPipelineState(descriptor: descriptor)
        slots = try (0..<3).map { _ in try FlightHUDSurface(device: device) }
    }

    func prepare(frame: FlightReplay.Frame, head: simd_float4x4, placement: MapPlacement, terrainValid: Bool, boarding: Bool,
              telemetry: ReplayTelemetry) {
        guard frame.view == .fpv || frame.returnSeconds != nil else { return }
        let reference = indicate_svs_reference(telemetry)
        func vector(_ v: (Float, Float, Float)) -> SIMD3<Float> { [v.0, v.1, v.2] }
        let instrument = FlightViewBasis.instrument(rotation: placement.current.rotation,
            right: vector(reference.right), up: vector(reference.up), forward: vector(reference.forward))
        let alignment = simd_dot(SIMD3(head.columns.2.x, head.columns.2.y, head.columns.2.z),
                                 SIMD3(instrument.columns.2.x, instrument.columns.2.y, instrument.columns.2.z))
        let nextGlance = boarding || frame.view != .fpv
            || indicate_svs_compact(telemetry, alignment, glancing ? 1 : 0) != 0
        let layoutChanged = glancing != nextGlance
        glancing = nextGlance
        let now = ProcessInfo.processInfo.systemUptime
        let caption = "\(frame.playing)-\(boarding)-\(frame.returnSeconds ?? -1)"
        if cadence.consume(now: now, changed: lastElapsed != frame.elapsed || lastCaption != caption,
                           urgent: layoutChanged || revision != frame.cameraRevision || lastValid != terrainValid) {
            // Both eyes use one telemetry snapshot. Collimated geometry updates every frame;
            // text raster work is bounded independently of head tracking.
            update(frame, input: telemetry, terrainValid: terrainValid, boarding: boarding)
            revision = frame.cameraRevision
            lastValid = terrainValid
            lastElapsed = frame.elapsed
            lastCaption = caption
        }
    }

    func encode(head: simd_float4x4, drawable: LayerRenderer.Drawable, index: Int,
                encoder: MTLRenderCommandEncoder) {
        // All text shares one projection. Earth-referenced cues use FlightSymbolRenderer.
        let panel = head
        let view = drawable.views[index]
        let projection = drawable.computeProjection(viewIndex: index) * simd_inverse(head * view.transform) * panel
        let corners: [(SIMD4<Float>, SIMD2<Float>)] = [
            ([-1.2, 0.6, -2, 0], [0, 0]), ([-1.2, -0.6, -2, 0], [0, 1]), ([1.2, 0.6, -2, 0], [1, 0]),
            ([1.2, 0.6, -2, 0], [1, 0]), ([-1.2, -0.6, -2, 0], [0, 1]), ([1.2, -0.6, -2, 0], [1, 1])]
        let vertices = corners.map { Vertex(position: projection * $0.0, uv: $0.1) }
        encoder.setRenderPipelineState(pipeline)
        vertices.withUnsafeBytes { bytes in
            if let base = bytes.baseAddress { encoder.setVertexBytes(base, length: bytes.count, index: 0) }
        }
        encoder.setFragmentTexture(slots[slot].texture, index: 0)
        encoder.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 6)
    }

    private func update(_ frame: FlightReplay.Frame, input: ReplayTelemetry, terrainValid: Bool, boarding: Bool) {
        let start = ProcessInfo.processInfo.systemUptime
        // Terrain availability cannot invalidate an independent aircraft-data source.
        var scene = glancing ? indicate_svs_glance(input) : indicate_svs_render(input)
        let length = scene.length <= 8192 ? Int(scene.length) : 0
        let bytes = withUnsafeBytes(of: &scene) { Array($0.dropFirst(4).prefix(length)) }
        let caption = contextText(frame, terrainValid: terrainValid, boarding: boarding)
        guard bytes != lastScene || caption != lastContext else { return }
        slot = (slot + 1) % slots.count
        let surface = slots[slot]
        surface.begin()
        defer { surface.end() }
        var context = surface.context
        context.clear(CGRect(x: 0, y: 0, width: 1200, height: 600))
        context.saveGState()
        context.translateBy(x: 0, y: 600)
        context.scaleBy(x: 1, y: -1)
        FlightHUDContrast.apply(to: context)
        do {
            if frame.view == .fpv {
                let report = try renderer.render(bytes, into: context)
                guard layout.reference == 2, report.unknownOpcodes == 0, report.layersPresent.contains(.tapes),
                      report.layersPresent.contains(.annunciation) else { throw ProducerFault(reason: .paintFailed) }
            }
            drawContext(caption, context: context)
            lastScene = bytes
            lastContext = caption
        } catch {
            lastScene = []
            lastContext = ""
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

    private func contextText(_ frame: FlightReplay.Frame, terrainValid: Bool, boarding: Bool) -> String {
        // Playback state belongs to the host; it must never look like a live sensor annunciation.
        let status = !terrainValid ? "VIEW UNAVAILABLE" : boarding ? "BOARDING" : frame.observation == nil ? "TRACK DATA GAP" : frame.playing ? "FPV REPLAY" : "REPLAY PAUSED"
        let caption = frame.track?.isSimulation == true ? "SIMULATED" : "RECORDED"
        return frame.returnSeconds.map { "FREE LOOK · RETURN \($0)s · PINCH FOR CONTROLS" }
            ?? "\(caption) · \(status)"
    }

    private func drawContext(_ text: String, context: CGContext) {
        let zone = layout.host_context
        let width = CGFloat(zone.2) - 4
        let attributes: [NSAttributedString.Key: Any] = [
            .font: CTFontCreateWithName("Menlo-Bold" as CFString, 11, nil),
            .foregroundColor: CGColor(red: 1, green: 0.8, blue: 0.25, alpha: 1)]
        let line = CTLineCreateWithAttributedString(NSAttributedString(string: text, attributes: attributes))
        let measured = CGFloat(CTLineGetTypographicBounds(line, nil, nil, nil))
        let scale = min(1, width / max(measured, 1))
        context.saveGState()
        context.translateBy(x: CGFloat(zone.0) + 2, y: CGFloat(zone.1 + zone.3) - 3)
        context.scaleBy(x: scale, y: -scale)
        context.textPosition = .zero
        CTLineDraw(line, context)
        context.restoreGState()
    }
}
