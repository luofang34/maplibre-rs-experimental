import simd

/// Angular flight symbols in local east/north/up coordinates, independent of eyes and pixels.
struct FlightHUDGeometry {
    struct Stroke { let a: SIMD3<Double>; let b: SIMD3<Double> }
    enum Marker { case prograde, retrograde }
    let velocity: SIMD3<Double>?
    let bodyForward: SIMD3<Double>?
    let bodyRight: SIMD3<Double>

    init(point: FlightTrack.Observation, verticalSpeed: Double?) {
        let track = point.track * .pi / 180
        if point.hasVelocity, point.groundSpeed > 1, let verticalSpeed, verticalSpeed.isFinite {
            velocity = simd_normalize(SIMD3(sin(track) * point.groundSpeed, cos(track) * point.groundSpeed, verticalSpeed))
        } else { velocity = nil }
        let heading = (point.heading ?? point.track) * .pi / 180
        let right = SIMD3<Double>(cos(heading), -sin(heading), 0)
        let pitch = (point.hasAttitude ? point.pitch ?? 0 : 0) * .pi / 180
        let forward = SIMD3<Double>(sin(heading) * cos(pitch), cos(heading) * cos(pitch), sin(pitch))
        let roll = (point.hasAttitude ? point.roll ?? 0 : 0) * .pi / 180
        bodyRight = right * cos(roll) - simd_cross(right, forward) * sin(roll)
        bodyForward = point.hasAttitude ? forward : nil
    }

    func marker(_ kind: Marker) -> [Stroke] {
        guard let velocity else { return [] }
        let center = kind == .prograde ? velocity : -velocity
        let wing = bodyRight - center * simd_dot(bodyRight, center)
        guard simd_length_squared(wing) > 0.00001 else { return [] }
        let right = simd_normalize(wing), up = simd_normalize(simd_cross(right, center))
        let radius = tan(0.55 * .pi / 180)
        func point(_ x: Double, _ y: Double) -> SIMD3<Double> { simd_normalize(center + radius * (right * x + up * y)) }
        var strokes: [Stroke] = []
        for i in 0..<32 {
            let a = Double(i) * 2 * .pi / 32, b = Double(i + 1) * 2 * .pi / 32
            strokes.append(.init(a: point(cos(a), sin(a)), b: point(cos(b), sin(b))))
        }
        let wings: [(SIMD2<Double>, SIMD2<Double>)] = [([-2, 0], [-1, 0]), ([1, 0], [2, 0]), ([0, 1], [0, 1.7])]
        for (a, b) in wings {
            strokes.append(.init(a: point(a.x, a.y), b: point(b.x, b.y)))
        }
        if kind == .retrograde {
            strokes.append(.init(a: point(-0.65, -0.65), b: point(0.65, 0.65)))
            strokes.append(.init(a: point(-0.65, 0.65), b: point(0.65, -0.65)))
        }
        return strokes
    }

    func references() -> [Stroke] {
        guard let forward = bodyForward else { return [] }
        var lines: [Stroke] = []
        let up = simd_normalize(simd_cross(bodyRight, forward)), radius = tan(0.35 * .pi / 180)
        lines.append(.init(a: simd_normalize(forward - bodyRight * radius), b: simd_normalize(forward + bodyRight * radius)))
        lines.append(.init(a: simd_normalize(forward - up * radius), b: simd_normalize(forward + up * radius)))
        // The local horizon is segmented around all azimuths; it remains useful when looking aft.
        for azimuth in stride(from: -180, to: 180, by: 3) {
            lines.append(.init(a: direction(Double(azimuth), 0), b: direction(Double(azimuth + 3), 0)))
        }
        for pitch in stride(from: -30, through: 30, by: 5) where pitch != 0 {
            let center = atan2(forward.x, forward.y) * 180 / .pi
            for side in [-1.0, 1.0] {
                for x in 2..<8 where pitch > 0 || x % 2 == 0 {
                    lines.append(.init(a: direction(center + side * Double(x), Double(pitch)),
                                       b: direction(center + side * Double(x + 1), Double(pitch))))
                }
                lines.append(.init(a: direction(center + side * 8, Double(pitch)),
                                   b: direction(center + side * 8, Double(pitch) + (pitch > 0 ? -1 : 1))))
            }
            lines += pitchLabel(pitch, azimuth: center + 10)
        }
        return lines
    }

    private func pitchLabel(_ pitch: Int, azimuth: Double) -> [Stroke] {
        let digits: [Character: Int] = ["0": 0x3f, "1": 0x06, "2": 0x5b, "3": 0x4f, "4": 0x66,
                                      "5": 0x6d, "6": 0x7d, "7": 0x07, "8": 0x7f, "9": 0x6f, "-": 0x40]
        let segments: [(SIMD2<Double>, SIMD2<Double>)] = [([0, 2], [1, 2]), ([1, 2], [1, 1]),
            ([1, 1], [1, 0]), ([1, 0], [0, 0]), ([0, 0], [0, 1]), ([0, 1], [0, 2]), ([0, 1], [1, 1])]
        var result: [Stroke] = []
        for (index, digit) in String(pitch).enumerated() {
            for (bit, segment) in segments.enumerated() where (digits[digit] ?? 0) & (1 << bit) != 0 {
                let x = azimuth + Double(index) * 0.8
                result.append(.init(a: direction(x + segment.0.x * 0.55, Double(pitch) + (segment.0.y - 1) * 0.55),
                                    b: direction(x + segment.1.x * 0.55, Double(pitch) + (segment.1.y - 1) * 0.55)))
            }
        }
        return result
    }

    private func direction(_ azimuth: Double, _ elevation: Double) -> SIMD3<Double> {
        let a = azimuth * .pi / 180, e = elevation * .pi / 180
        return [sin(a) * cos(e), cos(a) * cos(e), sin(e)]
    }
}
