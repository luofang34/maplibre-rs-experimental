import ARKit
import CompositorServices
import Metal
import simd

/// Drives the compositor frame loop: per frame and eye, hands the head pose to the map and
/// copies the map's texture into the drawable.
final class MapRenderer {
    static let spaceID = "map"

    private let layerRenderer: LayerRenderer
    private let device: MTLDevice
    private let commandQueue: MTLCommandQueue
    private let arSession = ARKitSession()
    private let worldTracking = WorldTrackingProvider()
    private var map: OpaquePointer?
    /// Where the local frame sits: the eye starts here and moves in metres from it.
    private let anchor = MapAnchor.innsbruckOverlook

    init(layerRenderer: LayerRenderer) {
        self.layerRenderer = layerRenderer
        device = layerRenderer.device
        guard let queue = device.makeCommandQueue() else {
            fatalError("Metal command queue unavailable")
        }
        commandQueue = queue
    }

    func start() {
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
        while true {
            switch layerRenderer.state {
            case .invalidated:
                maplibre_visionos_destroy(map)
                map = nil
                return
            case .paused:
                layerRenderer.waitUntilRunning()
            default:
                renderFrame()
            }
        }
    }

    private func ensureMap(width: Int, height: Int) -> OpaquePointer? {
        if let map {
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
        if map == nil {
            print("maplibre_visionos_create failed")
        }
        return map
    }

    private func renderFrame() {
        guard let frame = layerRenderer.queryNextFrame() else {
            return
        }
        frame.startUpdate()
        frame.endUpdate()
        guard let timing = frame.predictTiming() else {
            return
        }
        LayerRenderer.Clock().wait(until: timing.optimalInputTime)
        frame.startSubmission()
        guard let drawable = frame.queryDrawable() else {
            frame.endSubmission()
            return
        }
        let presentation = MapRenderer.seconds(
            LayerRenderer.Clock.Instant.epoch.duration(to: timing.presentationTime))
        let deviceAnchor = worldTracking.queryDeviceAnchor(atTimestamp: presentation)
        drawable.deviceAnchor = deviceAnchor

        let firstTexture = drawable.colorTextures[0]
        guard let map = ensureMap(width: firstTexture.width, height: firstTexture.height),
              let commandBuffer = commandQueue.makeCommandBuffer()
        else {
            frame.endSubmission()
            return
        }
        let originFromDevice = deviceAnchor?.originFromAnchorTransform ?? matrix_identity_float4x4

        for (index, view) in drawable.views.enumerated() {
            let originFromView = originFromDevice * view.transform
            var viewFromLocal = simd_inverse(originFromView)
                * MapRenderer.originFromLocal(tiltDegrees: anchor.tiltDegrees)
            var tangents = view.tangents
            let depthRange = drawable.depthRange
            let near = depthRange.y
            let far = depthRange.x
            let raw = withUnsafePointer(to: &viewFromLocal) { viewPointer in
                viewPointer.withMemoryRebound(to: Float.self, capacity: 16) { viewFloats in
                    withUnsafePointer(to: &tangents) { tangentPointer in
                        tangentPointer.withMemoryRebound(to: Float.self, capacity: 4) { tangentFloats in
                            maplibre_visionos_render(
                                map, anchor.latitude, anchor.longitude, anchor.altitudeMeters,
                                viewFloats, tangentFloats, near, far, presentation)
                        }
                    }
                }
            }
            guard let raw else {
                continue
            }
            let source = Unmanaged<AnyObject>.fromOpaque(raw).takeUnretainedValue()
            guard let sourceTexture = source as? MTLTexture,
                  let blit = commandBuffer.makeBlitCommandEncoder()
            else {
                continue
            }
            let destination = drawable.colorTextures[index]
            let size = MTLSize(
                width: min(sourceTexture.width, destination.width),
                height: min(sourceTexture.height, destination.height),
                depth: 1)
            blit.copy(
                from: sourceTexture, sourceSlice: 0, sourceLevel: 0,
                sourceOrigin: MTLOrigin(x: 0, y: 0, z: 0), sourceSize: size,
                to: destination, destinationSlice: 0, destinationLevel: 0,
                destinationOrigin: MTLOrigin(x: 0, y: 0, z: 0))
            blit.endEncoding()
        }

        drawable.encodePresent(commandBuffer: commandBuffer)
        commandBuffer.commit()
        frame.endSubmission()
    }

    private static func seconds(_ duration: Duration) -> TimeInterval {
        let parts = duration.components
        return TimeInterval(parts.seconds) + TimeInterval(parts.attoseconds) / 1e18
    }

    /// The map's local frame is east, north, up in metres; the compositor's world is x east,
    /// y up, z south. The frame is then tilted about east so its far side rises towards the
    /// viewer, which keeps a level gaze within the map's pitch limit.
    private static func originFromLocal(tiltDegrees: Float) -> simd_float4x4 {
        let enuToWorld = simd_float4x4(columns: (
            SIMD4<Float>(1, 0, 0, 0),
            SIMD4<Float>(0, 0, -1, 0),
            SIMD4<Float>(0, 1, 0, 0),
            SIMD4<Float>(0, 0, 0, 1)
        ))
        let tilt = tiltDegrees * .pi / 180
        let tiltAboutEast = simd_float4x4(columns: (
            SIMD4<Float>(1, 0, 0, 0),
            SIMD4<Float>(0, cos(tilt), sin(tilt), 0),
            SIMD4<Float>(0, -sin(tilt), cos(tilt), 0),
            SIMD4<Float>(0, 0, 0, 1)
        ))
        return tiltAboutEast * enuToWorld
    }
}

/// A place the immersive map starts from.
struct MapAnchor {
    let latitude: Double
    let longitude: Double
    let altitudeMeters: Double
    /// How far the map is tilted towards the viewer, in degrees.
    let tiltDegrees: Float

    /// Above the Inn valley, high enough to see the Alps as terrain.
    static let innsbruckOverlook = MapAnchor(
        latitude: 47.26, longitude: 11.39, altitudeMeters: 4000, tiltDegrees: 40)
}
