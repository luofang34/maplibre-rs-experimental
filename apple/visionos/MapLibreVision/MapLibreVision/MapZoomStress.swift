import simd

/// A deterministic zoom sweep for exercising dense terrain while tiles arrive.
struct MapZoomStress {
    static let anchor = MapAnchor(latitude: 40.75, longitude: -74.0, altitudeMeters: 0)
    private var start: Double?
    private var phase: Int?

    mutating func input(at time: Double, height: Double,
                        origin: SIMD3<Double>, focus: SIMD3<Double>) -> MapGestureInput.Delta {
        if start == nil { start = time }
        let elapsed = time - (start ?? time) - 8
        guard elapsed >= 0 else { return .init() }
        let cycle = min(Int(elapsed / 24), 2)
        let local = elapsed - Double(cycle) * 24
        let segment = cycle == 2 ? 4 : local < 8 ? 0 : local < 12 ? 1 : local < 20 ? 2 : 3
        let nextPhase = cycle * 4 + segment
        let share = segment == 0 ? local / 8 : segment == 1 ? 1 : segment == 2 ? 1 - (local - 12) / 8 : 0
        let target = exp(log(MapPlacement.tableHeight) * (1 - share) + log(4000) * share)
        let ray = focus - origin
        let begins = phase != nextPhase
        phase = nextPhase
        guard simd_length(ray) > 1e-6 else { return .init() }
        return .init(logScale: log(height / target), beginsZoom: begins,
                     focusAnchor: (origin, simd_normalize(ray)))
    }
}
