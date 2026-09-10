import simd

enum FlightOwnshipGeometry {
    static func vertices(observation: FlightTrack.Observation, placement: MapPlacement,
                         radius: Double) -> [SIMD3<Double>] {
        let angle = (observation.heading ?? observation.track) * .pi / 180
        let forward = SIMD3<Double>(sin(angle), cos(angle), 0)
        let right = SIMD3<Double>(cos(angle), -sin(angle), 0)
        let base = placement.roomPoint(for: observation.coordinate)
        var orientation = placement.current.rotation
        if placement.viewpoint.height > MapPlacement.groundHeightLimit {
            let origin = GlobeDrag.geographicBasis(latitude: placement.viewpoint.latitude,
                                                   longitude: placement.viewpoint.longitude)
            let local = GlobeDrag.geographicBasis(latitude: observation.latitude, longitude: observation.longitude)
            orientation = orientation * simd_quatd(origin.transpose * local)
        }
        return [SIMD2<Double>(0, 1.5), [-1, -1], [0, -0.5], [0, 1.5], [0, -0.5], [1, -1]].map {
            base + orientation.act((right * $0.x + forward * $0.y) * radius)
        }
    }
}
