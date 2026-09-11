import simd

enum FlightViewBasis {
    static func instrument(rotation: simd_quatd, right: SIMD3<Float>, up: SIMD3<Float>,
                           forward: SIMD3<Float>) -> simd_float4x4 {
        .init(columns: (SIMD4(SIMD3<Float>(rotation.act(SIMD3<Double>(right))), 0),
                        SIMD4(SIMD3<Float>(rotation.act(SIMD3<Double>(up))), 0),
                        SIMD4(SIMD3<Float>(rotation.act(SIMD3<Double>(-forward))), 0),
                        SIMD4(0, 0, 0, 1)))
    }

    static func useGlance(wasGlancing: Bool, head: simd_float4x4, instrument: simd_float4x4) -> Bool {
        let dot = simd_dot(SIMD3(head.columns.2.x, head.columns.2.y, head.columns.2.z),
                           SIMD3(instrument.columns.2.x, instrument.columns.2.y, instrument.columns.2.z))
        // Separate entry and exit cones prevent boundary jitter from switching layouts.
        return dot < cos((wasGlancing ? 28 : 35) * Float.pi / 180)
    }

    /// Readouts follow the viewing direction, while their vertical axis stays aligned with gravity.
    static func leveled(head: simd_float4x4, up: SIMD3<Float>) -> simd_float4x4 {
        let forward = -SIMD3(head.columns.2.x, head.columns.2.y, head.columns.2.z)
        var right = simd_cross(forward, up)
        if simd_length_squared(right) < 0.0001 {
            // At the zenith, use world north instead of allowing noisy roll to select an axis.
            right = simd_cross(forward, abs(forward.y) < 0.9 ? [0, 1, 0] : [0, 0, 1])
        }
        right = simd_normalize(right)
        let vertical = simd_normalize(simd_cross(right, forward))
        return .init(columns: (SIMD4(right, 0), SIMD4(vertical, 0), SIMD4(-forward, 0), head.columns.3))
    }
}
