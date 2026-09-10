import Foundation
import simd

struct FlightCamera {
    private struct Boarding {
        let placement: MapPlacement
        let start: Double
    }
    private var viewer: SIMD3<Double>?
    private var yaw = simd_quatd(angle: 0, axis: SIMD3<Double>(0, 1, 0))
    private var revision: UInt64?
    private var generation: UInt64?
    private var view = FlightView.free
    private var boarding: Boarding?
    private(set) var isBoarding = false

    mutating func update(_ observation: FlightTrack.Observation, placement: inout MapPlacement,
                         head: SIMD3<Double>, forward: SIMD3<Double>, view: FlightView = .fpv,
                         revision: UInt64 = 0, generation: UInt64 = 0,
                         at time: Double = ProcessInfo.processInfo.systemUptime) {
        let changed = self.view != view || self.revision != revision
        if changed || viewer == nil {
            let returning = self.revision != nil
            boarding = returning ? Boarding(placement: placement, start: time) : nil
            viewer = head
            let horizontal = SIMD2<Double>(forward.x, forward.z)
            if simd_length(horizontal) > 0.001 {
                yaw = simd_quatd(angle: atan2(-forward.x, -forward.z), axis: [0, 1, 0])
            }
        }
        // Seeking is an explicit time change. Never fly an invented route across a recording gap.
        if self.generation != nil, self.generation != generation { boarding = nil }
        self.view = view
        self.revision = revision
        self.generation = generation
        placement.flight = nil
        placement.cameraPolicy = .fixedViewpoint
        placement.viewpoint = Viewpoint(latitude: observation.latitude, longitude: observation.longitude,
            height: max(150, observation.altitudeMSL - placement.focusElevation), bearing: 0)
        placement.viewerReference = viewer ?? head
        placement.sceneRotation = simd_quatd(angle: 0, axis: [0, 1, 0])
        placement.sceneOffset = .zero
        let level = simd_quatd(simd_double3x3([1, 0, 0], [0, 0, -1], [0, 1, 0]))
        let heading = view == .fpv ? observation.heading ?? observation.track : observation.track
        var attitude = simd_quatd(angle: heading * .pi / 180, axis: [0, 1, 0]) * level
        if view == .fpv, observation.hasAttitude {
            let pitch = simd_quatd(angle: -(observation.pitch ?? 0) * .pi / 180, axis: [1, 0, 0])
            let roll = simd_quatd(angle: (observation.roll ?? 0) * .pi / 180, axis: [0, 0, 1])
            attitude = roll * pitch * attitude
        }
        let rotation = simd_normalize(yaw * attitude)
        let localAircraft = SIMD3<Double>(0, 0, observation.altitudeMSL - placement.focusElevation)
        let chaseOffset = view == .chase ? yaw.act(SIMD3<Double>(0, -600, -3500)) : .zero
        var pose = ScenePose(rotation: rotation,
            translation: (viewer ?? head) + chaseOffset - rotation.act(localAircraft), logScale: 0)
        isBoarding = false
        if let transition = boarding {
            let t = min(max((time - transition.start) / 1.2, 0), 1)
            let eased = t * t * (3 - 2 * t)
            let from = transition.placement
            let origin = from.roomPoint(for: placement.anchor)
            pose = ScenePose(rotation: simd_slerp(from.current.rotation, pose.rotation, eased),
                translation: simd_mix(origin, pose.translation, SIMD3(repeating: eased)),
                logScale: from.current.logScale * (1 - eased))
            isBoarding = t < 1
            if !isBoarding { boarding = nil }
        }
        // Encode the composed pose in the free camera's representation too, so detaching
        // preserves the exact visible frame rather than snapping to a separate camera model.
        let base = placement.pose(for: placement.viewpoint)
        placement.sceneRotation = simd_normalize(pose.rotation * base.rotation.inverse)
        placement.sceneOffset = pose.translation - base.translation
        placement.current = pose
        placement.immersion = .full
    }

    mutating func detach(placement: inout MapPlacement) {
        guard view != .free else { return }
        view = .free
        viewer = nil
        boarding = nil
        isBoarding = false
        placement.cameraPolicy = .freeOrbit
        placement.orbitTarget = nil
        placement.zoomTarget = nil
        placement.groundDrag = nil
        placement.retainedFocus = nil
    }
}
