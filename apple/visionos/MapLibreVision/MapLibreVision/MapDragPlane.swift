import simd

/// A captured manipulation plane prevents sky and grazing rays from changing pan gain.
struct MapDragPlane {
    private let origin: SIMD3<Double>
    private let point: SIMD3<Double>
    private let normal: SIMD3<Double>
    private let surfaceRotation: simd_quatd
    private let reach: Double

    init(origin: SIMD3<Double>, ray: SIMD3<Double>, surface: SIMD3<Double>, normal: SIMD3<Double>) {
        self.origin = origin
        let height = max(abs(simd_dot(origin - surface, normal)), 1e-6)
        reach = height * 4
        let incidence = -simd_dot(ray, normal)
        if incidence >= 0.35 {
            self.normal = normal
            surfaceRotation = simd_quatd(angle: 0, axis: normal)
            point = surface
        } else {
            // A plane facing the captured ray provides a finite continuation at the horizon.
            self.normal = -ray
            surfaceRotation = simd_quatd(from: -ray, to: normal)
            point = origin + ray * reach
        }
    }

    func translation(from: SIMD3<Double>, to: SIMD3<Double>) -> SIMD3<Double> {
        guard let a = intersection(from), let b = intersection(to) else { return .zero }
        let displacement = b - a
        let tangent = surfaceRotation.act(displacement)
        // A held drag approaching a grazing angle cannot accelerate without bound.
        let limit = reach * simd_length(to - from) / 0.35
        let length = simd_length(tangent)
        return length > limit && length > 1e-9 ? tangent * (limit / length) : tangent
    }

    private func intersection(_ ray: SIMD3<Double>) -> SIMD3<Double>? {
        let denominator = simd_dot(ray, normal)
        guard abs(denominator) > 1e-6 else { return nil }
        let distance = simd_dot(point - origin, normal) / denominator
        guard distance.isFinite, distance > 0 else { return nil }
        return origin + ray * distance
    }
}
