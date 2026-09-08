import ARKit
import CompositorServices
import Metal
import QuartzCore
import os
import SwiftUI
import simd

/// Drives the compositor frame loop: per frame it places the scene for the current mode and,
/// hands all eye poses to the map together and presents their completed textures.
final class MapRenderer {
    static let spaceID = "map"

    private let layerRenderer: LayerRenderer
    private let device: MTLDevice
    private let arSession = ARKitSession()
    private let worldTracking = WorldTrackingProvider()
    private let modeStore = MapModeStore.shared
    private let gestures = MapGestures()
    private var placement: MapPlacement
    private var map: OpaquePointer?
    /// Size the map was created at, so a later layer of the same size can take it over.
    private var mapSize = (width: 0, height: 0)
    /// The map and the viewpoint of a compositor layer that was invalidated. Changing the
    /// room's immersion recreates the layer; without this the next layer would build a
    /// second map beside the first and start its viewpoint from the table again, so a
    /// flight that crosses the immersion threshold would never arrive.
    private static var parked: (map: OpaquePointer, width: Int, height: Int, placement: MapPlacement)?
    /// The map's own queue; copies and presentation committed on it run after the map's
    /// draws without a wait on the CPU.
    private var mapQueue: MTLCommandQueue?
    private let eyeTargets = MapEyeTargets()
    private static var loggedProjection = false
    private var placedFromHead = false
    private var stats = FrameStats()
    private var skippedFrames = 0
    /// Frames the GPU may still be drawing while the next is encoded. Without a bound the
    /// CPU runs ahead until the queue holds its limit of command buffers and Metal blocks
    /// inside the map's encoding, past the compositor's patience.
    private let framesInFlight = DispatchSemaphore(value: MapRenderer.maxFramesInFlight)
    private static let maxFramesInFlight = 2
    /// Tiles are requested this far beyond each eye's frustum, so a turn of the head finds
    /// them loaded.
    private static let requestOverscan: Float = 1.5
    /// Bytes `--memory-cap N` (megabytes) pretends are the most the process may still take.
    private static let memoryCap: UInt64 = {
        let arguments = ProcessInfo.processInfo.arguments
        if let index = arguments.firstIndex(of: "--memory-cap"), index + 1 < arguments.count,
           let megabytes = UInt64(arguments[index + 1])
        {
            return megabytes << 20
        }
        return UInt64.max
    }()
    private var lastAvailableMemory: UInt64 = 0

    init(layerRenderer: LayerRenderer) {
        self.layerRenderer = layerRenderer
        device = layerRenderer.device
        if let parked = MapRenderer.parked {
            placement = parked.placement
        } else {
            placement = MapPlacement(
                viewpoint: Viewpoint.above(MapAnchor.innsbruck, height: modeStore.initialHeight))
        }
    }

    func start() {
        layerRenderer.onSpatialEvent = { [weak self] events in
            guard let self else {
                return
            }
            gestures.handle(events)
        }
        Task {
            do {
                try await arSession.run([worldTracking])
            } catch {
                print("world tracking unavailable: \(error)")
            }
        }
        let thread = Thread { [self] in renderLoop() }
        thread.name = "maplibre render"
        thread.start()
    }

    private func renderLoop() {
        var lastState = layerRenderer.state
        while true {
            let state = layerRenderer.state
            if state != lastState {
                let line = "layer renderer state \(lastState) -> \(state)"
                print(line)
                maplibre_visionos_note(line)
                lastState = state
            }
            switch state {
            case .invalidated:
                if let map {
                    MapRenderer.parked = (map, mapSize.width, mapSize.height, placement)
                    maplibre_visionos_note("layer invalidated: map parked for the next layer")
                }
                map = nil
                return
            case .paused:
                layerRenderer.waitUntilRunning()
            default:
                // The render thread has no run loop to drain its autorelease pool, so
                // without a pool per frame every Metal object a frame autoreleases stays
                // until the thread ends, and the footprint climbs for as long as the map
                // is looked at.
                autoreleasepool {
                    renderFrame()
                }
            }
        }
    }

    private func ensureMap(width: Int, height: Int) -> OpaquePointer? {
        if let map {
            return map
        }
        if let parked = MapRenderer.parked, parked.width == width, parked.height == height {
            MapRenderer.parked = nil
            map = parked.map
            mapSize = (width, height)
            if let raw = maplibre_visionos_command_queue(parked.map) {
                mapQueue = Unmanaged<AnyObject>.fromOpaque(raw).takeUnretainedValue() as? MTLCommandQueue
            }
            maplibre_visionos_note("map taken over by the new compositor layer")
            return map
        }
        guard let url = Bundle.main.url(forResource: "style", withExtension: "json"),
              let style = try? String(contentsOf: url, encoding: .utf8)
        else {
            print("style.json missing from the bundle")
            return nil
        }
        let cache = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)
            .first?.appendingPathComponent("maplibre-tiles").path
        map = maplibre_visionos_create(style, UInt32(width), UInt32(height), cache)
        mapSize = (width, height)
        if map == nil {
            print("maplibre_visionos_create failed")
            maplibre_visionos_note("maplibre_visionos_create failed")
        }
        if let map, let raw = maplibre_visionos_command_queue(map) {
            mapQueue = Unmanaged<AnyObject>.fromOpaque(raw).takeUnretainedValue() as? MTLCommandQueue
        }
        if mapQueue == nil {
            maplibre_visionos_note("the map's command queue is unavailable")
        }
        return map
    }

    /// Whether the map can draw straight into the drawable: every view owns a whole texture
    /// (no layered layout, whose slices the map's import does not address) and the texture
    /// allows a view in the map's linear format. Otherwise each eye has a separate
    /// intermediate texture copied into its slice.
    private static func drawsDirectly(_ drawable: LayerRenderer.Drawable) -> Bool {
        let dedicated = drawable.views.allSatisfy { $0.textureMap.sliceIndex == 0 }
            && Set(drawable.views.map { $0.textureMap.textureIndex }).count == drawable.views.count
        return dedicated && drawable.colorTextures.allSatisfy {
            $0.textureType == .type2D && $0.usage.contains(.pixelFormatView)
        }
    }

    /// Wall-clock cost of the frames since the last report, to tell a CPU-bound frame loop
    /// from tiles that arrive late.
    private struct FrameStats {
        var frames = 0
        var total = 0.0
        var render = 0.0
        var copy = 0.0
        var longest = 0.0
        var late = 0
        var since = CACurrentMediaTime()
        static let reportEvery = 90
        static let reportAfterSeconds = 1.0
        static let budget = 1.0 / 90.0

        mutating func add(total: Double, render: Double, copy: Double, gpuAllocated: Int, availableMB: UInt64) {
            frames += 1
            self.total += total
            self.render += render
            self.copy += copy
            longest = max(longest, total)
            if total > FrameStats.budget {
                late += 1
            }
            let now = CACurrentMediaTime()
            let elapsed = now - since
            // A slow frame loop would otherwise go unreported for as long as it takes to
            // collect the frames.
            if frames == FrameStats.reportEvery || elapsed >= FrameStats.reportAfterSeconds {
                let frames = Double(self.frames)
                let ms = { (seconds: Double) in String(format: "%.2f", seconds * 1000 / frames) }
                let memory = FrameStats.memoryMB()
                let footprint = String(format: "%.0f", memory.footprint)
                let heap = String(format: "%.0f", memory.heap)
                let gpu = String(format: "%.0f", Double(gpuAllocated) / 1_048_576)
                let line = "frame time ms: total \(ms(self.total)) render \(ms(self.render)) copy \(ms(self.copy)) longest \(String(format: "%.2f", longest * 1000)) late \(late)/\(self.frames) over \(String(format: "%.1f", elapsed)) s footprint \(footprint) MB heap \(heap) MB gpu \(gpu) MB avail \(availableMB) MB"
                print(line)
                maplibre_visionos_note(line)
                self = FrameStats()
            }
        }

        /// Resident memory as the system's memory limit counts it, and the part of it that
        /// is the process's own heap; the rest is textures, buffers and the compositor's
        /// surfaces.
        static func memoryMB() -> (footprint: Double, heap: Double) {
            var info = task_vm_info_data_t()
            var count = mach_msg_type_number_t(MemoryLayout<task_vm_info_data_t>.size / MemoryLayout<natural_t>.size)
            let result = withUnsafeMutablePointer(to: &info) {
                $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
                    task_info(mach_task_self_, task_flavor_t(TASK_VM_INFO), $0, &count)
                }
            }
            guard result == KERN_SUCCESS else { return (.nan, .nan) }
            return (Double(info.phys_footprint) / 1_048_576, Double(info.internal) / 1_048_576)
        }
    }

    private func renderFrame() {
        guard let frame = layerRenderer.queryNextFrame() else {
            skippedFrames += 1
            if skippedFrames == 1 || skippedFrames % 300 == 0 {
                print("no frame from the compositor (\(skippedFrames) so far)")
            }
            return
        }
        framesInFlight.wait()
        var completionOwnsPermit = false
        defer {
            if !completionOwnsPermit { framesInFlight.signal() }
        }
        var renderSeconds = 0.0
        var copySeconds = 0.0
        frame.startUpdate()
        frame.endUpdate()
        guard let timing = frame.predictTiming() else {
            return
        }
        LayerRenderer.Clock().wait(until: timing.optimalInputTime)
        // The wait paces the loop; the work of the frame starts here.
        let frameStart = CACurrentMediaTime()
        frame.startSubmission()
        // A frame without a drawable is skipped without ending its submission, as the
        // Compositor Services sample does; ending it aborts the process.
        guard let drawable = frame.queryDrawable() else {
            print("no drawable for the frame")
            return
        }
        let presentation = MapRenderer.seconds(
            LayerRenderer.Clock.Instant.epoch.duration(to: drawable.frameTiming.presentationTime))
        let deviceAnchor = worldTracking.queryDeviceAnchor(atTimestamp: presentation)
        drawable.deviceAnchor = deviceAnchor

        let firstTexture = drawable.colorTextures[0]
        guard let map = ensureMap(width: firstTexture.width, height: firstTexture.height) else {
            frame.endSubmission()
            return
        }
        guard let queue = mapQueue else {
            maplibre_visionos_note("map queue unavailable; stereo frame skipped")
            frame.endSubmission()
            return
        }
        guard let commandBuffer = queue.makeCommandBuffer() else {
            frame.endSubmission()
            return
        }
        let head = deviceAnchor.map { anchor -> SIMD3<Double> in
            let column = anchor.originFromAnchorTransform.columns.3
            return SIMD3<Double>(Double(column.x), Double(column.y), Double(column.z))
        } ?? SIMD3<Double>(0, 0, 0)
        let controls = modeStore.takeControls(isGlobe: placement.viewpoint.height > MapPlacement.groundHeightLimit)
        let headMatrix = deviceAnchor?.originFromAnchorTransform ?? matrix_identity_float4x4
        placement.updateViewRay(origin: head, direction: -SIMD3<Double>(
            Double(headMatrix.columns.2.x), Double(headMatrix.columns.2.y), Double(headMatrix.columns.2.z)))
        gestures.updateContext(
            head: head, right: SIMD3<Double>(SIMD3<Float>(headMatrix.columns.0.x, headMatrix.columns.0.y, headMatrix.columns.0.z)),
            up: SIMD3<Double>(SIMD3<Float>(headMatrix.columns.1.x, headMatrix.columns.1.y, headMatrix.columns.1.z)), isGlobe: placement.viewpoint.height > MapPlacement.groundHeightLimit)
        if let tilt = controls.0 { placement.setTilt(tilt) }
        if controls.1 { placement.levelView() }
        if !placedFromHead, let deviceAnchor {
            // The globe sits a metre ahead of where the viewer first looks, a little below
            // the eyes, whatever height the tracker's origin has; the world lies under them.
            placedFromHead = true
            let transform = deviceAnchor.originFromAnchorTransform
            var forward = -SIMD3<Double>(SIMD3<Float>(transform.columns.2.x, transform.columns.2.y, transform.columns.2.z))
            forward.y = 0
            if simd_length(forward) > 1e-3 {
                forward = simd_normalize(forward)
            } else {
                forward = SIMD3<Double>(0, 0, -1)
            }
            placement.tableCenter = head + forward * 1.0 - SIMD3<Double>(0, 0.3, 0)
            placement.place(viewer: head)
            print("table globe centred at \(placement.tableCenter) from head at \(head)")
        }
        if let request = modeStore.takeFlightRequest() {
            placement.fly(to: request.height, at: presentation, viewer: head)
        }
        if let immersion = placement.advance(at: presentation) {
            modeStore.showImmersion(immersion)
        }
        // What the system would still let the process take; the map keeps its drapes and
        // requests within it. `--memory-cap N` pretends only N megabytes are left, so the
        // simulator can show the map living within a headset's limit.
        let reported = UInt64(os_proc_available_memory())
        let footprintMB = FrameStats.memoryMB().footprint
        let footprint = footprintMB.isFinite ? UInt64(max(0, footprintMB) * 1_048_576) : 0
        let available = min(MapMemoryBudget.available(reported: reported, footprint: footprint), MapRenderer.memoryCap)
        maplibre_visionos_set_available_memory(map, available)
        lastAvailableMemory = available
        maplibre_visionos_set_opaque_environment(map, placement.immersion == .full)
        let input = gestures.take()
        placement.apply(input)
        let scenePose = placement.current.worldFromScene()
        var worldFromScene = simd_float4x4(columns: (
            SIMD4<Float>(scenePose.columns.0), SIMD4<Float>(scenePose.columns.1),
            SIMD4<Float>(scenePose.columns.2), SIMD4<Float>(scenePose.columns.3)))
        let anchor = placement.anchor
        let destination = placement.destination()?.worldFromScene()
        var prefetchFromScene = simd_float4x4(columns: (
            SIMD4<Float>(destination?.columns.0 ?? SIMD4<Double>(1, 0, 0, 0)),
            SIMD4<Float>(destination?.columns.1 ?? SIMD4<Double>(0, 1, 0, 0)),
            SIMD4<Float>(destination?.columns.2 ?? SIMD4<Double>(0, 0, 1, 0)),
            SIMD4<Float>(destination?.columns.3 ?? SIMD4<Double>(0, 0, 0, 1))))
        let hasPrefetch = destination != nil
        let originFromDevice = deviceAnchor?.originFromAnchorTransform ?? matrix_identity_float4x4
        let depthRange = drawable.depthRange
        let near = depthRange.y
        // A compositor that reprojects reports no far plane; the map's fallback puts one far
        // enough out that the flat map meets the sky at the horizon.
        let far = depthRange.x.isFinite && depthRange.x > near ? depthRange.x : Float.infinity

        // Every eye's pose and frustum, laid out flat so the C structs can point into them.
        var matrices: [Float] = []
        var tangents: [Float] = []
        for (index, view) in drawable.views.enumerated() {
            let worldFromEye = originFromDevice * view.transform
            for column in [worldFromEye.columns.0, worldFromEye.columns.1, worldFromEye.columns.2, worldFromEye.columns.3] {
                matrices.append(contentsOf: [column.x, column.y, column.z, column.w])
            }
            // Mixed immersion forbids reading a view's tangents; the projection the compositor
            // computes carries the same frustum, and its x and y rows read back as tangents.
            let projection = drawable.computeProjection(viewIndex: index)
            tangents.append(contentsOf: [
                (1 - projection.columns.2.x) / projection.columns.0.x,
                (1 + projection.columns.2.x) / projection.columns.0.x,
                (1 + projection.columns.2.y) / projection.columns.1.y,
                (1 - projection.columns.2.y) / projection.columns.1.y,
            ])
            if !MapRenderer.loggedProjection {
                MapRenderer.loggedProjection = true
                let maps = drawable.views.map { "\($0.textureMap.textureIndex)/\($0.textureMap.sliceIndex)" }
                let usage = drawable.colorTextures[index].usage
                let depthUsage = drawable.depthTextures.first?.usage.rawValue ?? 0
                print("drawable \(firstTexture.width)x\(firstTexture.height) depth usage \(depthUsage)")
                print("compositor views \(drawable.views.count) colour textures \(drawable.colorTextures.count) depth textures \(drawable.depthTextures.count) texture/slice \(maps) depthRange \(depthRange) projection z row \(projection.columns.2.z) \(projection.columns.3.z) colour usage \(usage.rawValue) direct \(MapRenderer.drawsDirectly(drawable))")
            }
        }
        let direct = MapRenderer.drawsDirectly(drawable)
        guard let colors = eyeTargets.colors(for: drawable, device: device, direct: direct) else {
            frame.endSubmission()
            return
        }
        let depths = eyeTargets.depths(for: drawable)
        commandBuffer.addCompletedHandler { [framesInFlight] _ in
            framesInFlight.signal()
        }
        completionOwnsPermit = true
        let group = Array(drawable.views.indices)
        let renderStart = CACurrentMediaTime()
        let result: UnsafeRawPointer? = withUnsafePointer(to: &worldFromScene) { scenePointer in
            scenePointer.withMemoryRebound(to: Float.self, capacity: 16) { sceneFloats in
              withUnsafePointer(to: &prefetchFromScene) { prefetchPointer in
               prefetchPointer.withMemoryRebound(to: Float.self, capacity: 16) { prefetchFloats in
                matrices.withUnsafeBufferPointer { matrixBuffer in
                    tangents.withUnsafeBufferPointer { tangentBuffer in
                        var placementC = MaplibreVisionOSPlacement(
                            anchor_latitude: anchor.latitude,
                            anchor_longitude: anchor.longitude,
                            anchor_altitude_meters: anchor.altitudeMeters,
                            world_from_scene: sceneFloats)
                        var prefetchC = MaplibreVisionOSPlacement(
                            anchor_latitude: anchor.latitude,
                            anchor_longitude: anchor.longitude,
                            anchor_altitude_meters: anchor.altitudeMeters,
                            world_from_scene: prefetchFloats)
                        var eyes = group.map { index -> MaplibreVisionOSEye in
                            let color: MTLTexture? = colors[index]
                            let depth = depths[index]
                            return MaplibreVisionOSEye(
                                world_from_eye: matrixBuffer.baseAddress! + 16 * index,
                                tangents: tangentBuffer.baseAddress! + 4 * index,
                                near: near,
                                far: far,
                                color_texture: color.map { UnsafeRawPointer(Unmanaged.passUnretained($0 as AnyObject).toOpaque()) },
                                depth_texture: depth.map { UnsafeRawPointer(Unmanaged.passUnretained($0 as AnyObject).toOpaque()) })
                        }
                        return eyes.withUnsafeMutableBufferPointer { eyeBuffer in
                            withUnsafePointer(to: &prefetchC) { prefetchPlacement in
                                maplibre_visionos_render_frame(
                                    map, &placementC, hasPrefetch ? prefetchPlacement : nil,
                                    eyeBuffer.baseAddress, UInt32(eyeBuffer.count),
                                    MapRenderer.requestOverscan, presentation)
                            }
                        }
                    }
                }
               }
              }
            }
        }
        if let selection = input.selection, let eye = drawable.views.first {
            modeStore.showSelection(selectLabel(selection, worldFromEye: originFromDevice * eye.transform,
                projection: drawable.computeProjection(viewIndex: 0), map: map))
        }
        renderSeconds += CACurrentMediaTime() - renderStart
        guard result != nil else {
            maplibre_visionos_note("stereo frame failed")
            commandBuffer.commit()
            frame.endSubmission()
            return
        }
        let copyStart = CACurrentMediaTime()
        if !direct, !eyeTargets.copy(to: drawable, commandBuffer: commandBuffer) {
            commandBuffer.commit()
            frame.endSubmission()
            return
        }
        copySeconds = CACurrentMediaTime() - copyStart

        drawable.encodePresent(commandBuffer: commandBuffer)
        commandBuffer.commit()
        frame.endSubmission()
        // The map knows the terrain under the focus; the viewer stands that much higher, so
        // a height is a height above the ground. The ground eases to a newly loaded DEM
        // rather than jumping to it.
        let focus = placement.anchor
        let elevation = Double(maplibre_visionos_terrain_elevation(map, focus.latitude, focus.longitude))
        if elevation.isFinite, (-500...9000).contains(elevation) {
            // The ground under the focus rises the moment a finer elevation says so, since a
            // viewer below the drawn terrain sees its underside; it sinks gently.
            if elevation > placement.focusElevation {
                placement.focusElevation = elevation
            } else {
                placement.focusElevation += (elevation - placement.focusElevation) * 0.1
            }
        }
        stats.add(
            total: CACurrentMediaTime() - frameStart, render: renderSeconds, copy: copySeconds,
            gpuAllocated: device.currentAllocatedSize, availableMB: lastAvailableMemory >> 20)
    }

    private static func seconds(_ duration: Duration) -> TimeInterval {
        let parts = duration.components
        return TimeInterval(parts.seconds) + TimeInterval(parts.attoseconds) / 1e18
    }
}
