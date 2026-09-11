import simd

/// A capsule in pixel coordinates, clipped before perspective division.
enum FlightStroke {
    struct Corner {
        var position: SIMD4<Float>
        var capsule: SIMD4<Float>
    }

    static func forEachCorner(_ start: SIMD4<Float>, _ end: SIMD4<Float>, width: Float,
                        viewport: SIMD2<Float>, body: (Corner) -> Void) {
        guard width > 0, width.isFinite, viewport.x > 0, viewport.y > 0,
              (0..<4).allSatisfy({ start[$0].isFinite && end[$0].isFinite }) else { return }
        var a = start, b = end
        // Reversed Metal depth: 0 <= z <= w. Clipping avoids a whole segment blinking
        // when only one endpoint passes the eye, and bounds screen-space expansion.
        for plane in [SIMD4<Float>(0, 0, 1, 0), SIMD4(0, 0, -1, 1), SIMD4(0, 0, 0, 1)] {
            let epsilon: Float = plane.w == 1 && plane.z == 0 ? 0.001 : 0
            let da = simd_dot(a, plane) - epsilon, db = simd_dot(b, plane) - epsilon
            if da < 0 && db < 0 { return }
            if da < 0 { a = simd_mix(a, b, SIMD4(repeating: da / (da - db))) }
            else if db < 0 { b = simd_mix(a, b, SIMD4(repeating: da / (da - db))) }
        }
        let delta = (SIMD2(b.x, b.y) / b.w - SIMD2(a.x, a.y) / a.w) * viewport * 0.5
        let length = simd_length(delta)
        guard length > 0.001, length.isFinite else { return }
        let tangent = delta / length, normal = SIMD2(-tangent.y, tangent.x)
        let radius = width * 0.5, extent = radius + 0.75
        for index in 0..<6 {
            let end = index == 1 || index == 4 || index == 5
            let above = index == 2 || index == 3 || index == 5
            let p = SIMD2(end ? length + extent : -extent, above ? extent : -extent)
            let base = p.x < 0 ? a : b
            let offset = (tangent * (p.x < 0 ? p.x : p.x - length) + normal * p.y) * 2 / viewport
            body(Corner(position: base + SIMD4(offset.x * base.w, offset.y * base.w, 0, 0),
                          capsule: SIMD4(p.x, p.y, length, radius)))
        }
    }
}
