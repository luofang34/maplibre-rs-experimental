import simd

struct FlightCamera {
    private var viewer: SIMD3<Double>?
    private var yaw = simd_quatd(angle: 0, axis: SIMD3<Double>(0, 1, 0))

    mutating func update(_ observation: FlightTrack.Observation, placement: inout MapPlacement,
                         head: SIMD3<Double>, forward: SIMD3<Double>) {
        if viewer == nil {
            viewer = head
            yaw = simd_quatd(angle: atan2(-forward.x, -forward.z), axis: SIMD3<Double>(0, 1, 0))
        }
        placement.flight = nil
        placement.viewpoint = Viewpoint(latitude: observation.latitude, longitude: observation.longitude,
            height: max(150, observation.altitudeMSL - placement.focusElevation + 600),
            bearing: observation.track * .pi / 180)
        placement.viewerReference = viewer ?? head
        placement.sceneRotation = yaw
        placement.sceneOffset = yaw.act(SIMD3<Double>(0, 0, -3500))
        placement.immersion = .full
        placement.current = placement.pose(for: placement.viewpoint)
    }

    mutating func detach() { viewer = nil }
}
