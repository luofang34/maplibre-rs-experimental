import simd

/// Transports the globe's orientation through a surface drag, including across either pole.
enum GlobeDrag {
    struct Focus {
        var latitude: Double
        var longitude: Double
        var roll: Double
    }

    static func focus(
        grabbed: SIMD3<Double>, pulled: SIMD3<Double>, latitude: Double, longitude: Double,
        roll: Double
    ) -> Focus? {
        guard grabbed.x.isFinite, pulled.x.isFinite else { return nil }
        let basis = geographicBasis(latitude: latitude, longitude: longitude)
        let turn = simd_quatd(from: grabbed, to: pulled)
        let localRoll = simd_quatd(angle: roll, axis: SIMD3<Double>(0, 0, 1))
        let point = simd_normalize(basis * turn.inverse.act(SIMD3<Double>(0, 0, 1)))
        // Keep the Mercator fallback finite even when the focus is exactly at the pole.
        let newLatitude = min(max(asin(min(max(point.z, -1), 1)) * 180 / .pi, -89.999999), 89.999999)
        let newLongitude = hypot(point.x, point.y) > 1e-10 ? atan2(point.y, point.x) * 180 / .pi : longitude
        let next = geographicBasis(latitude: newLatitude, longitude: newLongitude)
        let rotated = simd_double3x3(localRoll * turn) * basis.transpose * next
        return Focus(latitude: newLatitude, longitude: newLongitude,
                     roll: atan2(rotated.columns.0.y, rotated.columns.0.x))
    }

    static func geographicBasis(latitude: Double, longitude: Double) -> simd_double3x3 {
        let lat = latitude * .pi / 180
        let lon = longitude * .pi / 180
        return simd_double3x3(
            SIMD3<Double>(-sin(lon), cos(lon), 0),
            SIMD3<Double>(-sin(lat) * cos(lon), -sin(lat) * sin(lon), cos(lat)),
            SIMD3<Double>(cos(lat) * cos(lon), cos(lat) * sin(lon), sin(lat)))
    }
}
