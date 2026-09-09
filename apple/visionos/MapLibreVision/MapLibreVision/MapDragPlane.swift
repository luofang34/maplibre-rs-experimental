import simd

/// A captured manipulation plane prevents sky and grazing rays from changing pan gain.
struct MapDragPlane {
    private let origin: SIMD3<Double>
    private let point: SIMD3<Double>
    private let normal: SIMD3<Double>
    private let surfaceRotation: simd_quatd
    private let reach: Double
    private let followsSurface: Bool
    private let initialIntersection: SIMD3<Double>
    let referencePoint: SIMD3<Double>

    init(origin: SIMD3<Double>, ray: SIMD3<Double>, surface: SIMD3<Double>, normal: SIMD3<Double>) {
        self.origin = origin
        let height = max(abs(simd_dot(origin - surface, normal)), 1e-6)
        reach = height * 20
        let incidence = -simd_dot(ray, normal)
        followsSurface = incidence >= 0.05
        if followsSurface {
            self.normal = normal
            surfaceRotation = simd_quatd(angle: 0, axis: normal)
            point = surface
            referencePoint = origin + ray * (height / incidence)
            initialIntersection = referencePoint
        } else {
            // A plane facing the captured ray provides a finite continuation at the horizon.
            self.normal = -ray
            surfaceRotation = simd_quatd(from: -ray, to: normal)
            point = origin + ray * height * 4
            initialIntersection = point
            referencePoint = surface
        }
    }

    func target(ray: SIMD3<Double>) -> SIMD3<Double>? {
        guard let intersection = intersection(ray) else { return nil }
        return referencePoint + surfaceRotation.act(intersection - initialIntersection)
    }

    func translation(from: SIMD3<Double>, to: SIMD3<Double>) -> SIMD3<Double> {
        guard let a = intersection(from), let b = intersection(to) else { return .zero }
        let displacement = b - a
        let tangent = surfaceRotation.act(displacement)
        // A held drag approaching a grazing angle cannot accelerate without bound.
        if followsSurface { return tangent }
        let limit = reach / 5 * simd_length(to - from) / 0.35
        let length = simd_length(tangent)
        return length > limit && length > 1e-9 ? tangent * (limit / length) : tangent
    }

    private func intersection(_ ray: SIMD3<Double>) -> SIMD3<Double>? {
        let denominator = simd_dot(ray, normal)
        guard abs(denominator) > 1e-6 else { return nil }
        let distance = simd_dot(point - origin, normal) / denominator
        guard distance.isFinite, distance > 0, distance <= reach else { return nil }
        return origin + ray * distance
    }
}

enum MapTerrainRay {
    static func intersection(origin: SIMD3<Double>, direction: SIMD3<Double>, reach: Double,
                             clearance: (SIMD3<Double>) -> Double) -> SIMD3<Double>? {
        guard reach.isFinite, reach > 0, clearance(origin) > 0 else { return nil }
        var lower = 0.0
        // Quadratic spacing favors nearby terrain without allocating a mesh or waiting for tiles.
        for step in 1...64 {
            let share = Double(step) / 64
            var upper = reach * share * share
            let height = clearance(origin + direction * upper)
            guard height.isFinite else { return nil }
            if height <= 0 {
                for _ in 0..<18 {
                    let middle = (lower + upper) / 2
                    let middleHeight = clearance(origin + direction * middle)
                    guard middleHeight.isFinite else { return nil }
                    if middleHeight > 0 { lower = middle } else { upper = middle }
                }
                return origin + direction * ((lower + upper) / 2)
            }
            lower = upper
        }
        return nil
    }
}
