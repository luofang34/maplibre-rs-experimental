import CompositorServices
import Foundation
import Spatial
import SwiftUI
import simd

/// Serializes platform event batches with the render thread's input consumption.
final class MapGestures {
    typealias Delta = MapGestureInput.Delta
    private let lock = NSLock()
    private var recognizer = MapGestureRecognizer<SpatialEventCollection.Event.ID>()
    private var pending = Delta()

    func updateContext(head: SIMD3<Double>, right: SIMD3<Double>, up: SIMD3<Double>, isGlobe: Bool) {
        lock.withLock {
            recognizer.head = head
            recognizer.right = right
            recognizer.up = up
            if recognizer.setGlobe(isGlobe) { pending = Delta() }
        }
    }

    func handle(_ events: SpatialEventCollection) {
        let samples = events.map { event -> MapGestureRecognizer<SpatialEventCollection.Event.ID>.Sample in
            let position = event.inputDevicePose?.pose3D.position
            let ray = event.selectionRay
            return .init(
                id: event.id,
                position: event.phase == .active ? position.map { SIMD3<Double>($0.x, $0.y, $0.z) } : nil,
                rayOrigin: ray.map { SIMD3<Double>($0.origin.x, $0.origin.y, $0.origin.z) },
                rayDirection: ray.map { simd_normalize(SIMD3<Double>($0.direction.x, $0.direction.y, $0.direction.z)) },
                timestamp: ProcessInfo.processInfo.systemUptime, cancelled: event.phase == .cancelled)
        }
        lock.withLock {
            let delta = recognizer.handle(samples)
            pending.moves.append(contentsOf: delta.moves)
            pending.translation += delta.translation
            if let reference = delta.carryReference { pending.carryReference = reference }
            pending.logScale += delta.logScale
            pending.beginsZoom = pending.beginsZoom || delta.beginsZoom
            if let focus = delta.focusAnchor { pending.focusAnchor = focus }
            pending.turn += delta.turn
            pending.pitch += delta.pitch
            pending.beginsOrbit = pending.beginsOrbit || delta.beginsOrbit
            if let anchor = delta.orbitAnchor { pending.orbitAnchor = anchor }
            if let selection = delta.selection { pending.selection = selection }
            if let anchor = delta.zoomAnchor { pending.zoomAnchor = anchor }
        }
    }

    func take() -> Delta {
        lock.withLock {
            defer { pending = Delta() }
            return pending
        }
    }
}
