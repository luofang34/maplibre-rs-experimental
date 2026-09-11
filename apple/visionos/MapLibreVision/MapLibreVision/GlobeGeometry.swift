import simd

enum GlobeGeometry {
    static func direction(_ coordinate: MapAnchor) -> SIMD3<Double> {
        let latitude = coordinate.latitude * .pi / 180
        let longitude = coordinate.longitude * .pi / 180
        return SIMD3(cos(latitude) * sin(longitude), sin(latitude), cos(latitude) * cos(longitude))
    }

    static func coordinate(_ direction: SIMD3<Double>) -> MapAnchor {
        let point = simd_normalize(direction)
        return .init(latitude: asin(min(max(point.y, -1), 1)) * 180 / .pi,
                     longitude: atan2(point.x, point.z) * 180 / .pi, altitudeMeters: 0)
    }

}
