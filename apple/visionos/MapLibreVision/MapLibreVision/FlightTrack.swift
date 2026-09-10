import Foundation
import simd

struct FlightTrack: Codable {
    struct Observation: Codable {
        let time: Double
        let latitude: Double
        let longitude: Double
        let altitudeMSL: Double
        let altitudeGNSS: Double?
        let geoidSeparation: Double?
        let groundSpeed: Double
        let track: Double
        let roll: Double?
        var pitch: Double? = nil
        var heading: Double? = nil
        var startsSegment: Bool? = nil
        var indicatedAirspeed: Double? = nil
        var velocityAvailable: Bool? = nil

        var hasVelocity: Bool { velocityAvailable != false }

        var hasAttitude: Bool { roll != nil && pitch != nil && heading != nil }

        var coordinate: MapAnchor {
            .init(latitude: latitude, longitude: longitude, altitudeMeters: altitudeMSL)
        }
    }

    struct Provenance: Codable {
        let url: String
        let license: String
        let sha256: String
        let altitudeConversion: String
        let coverage: String
    }

    enum Invalid: Error { case observations, order, value }
    enum Kind: String, Codable { case recorded, simulation }
    var title: String? = nil
    var kind: Kind? = nil

    let callsign: String
    let registration: String
    let aircraft: String
    let destination: String
    let startUTC: Double
    let source: Provenance
    let observations: [Observation]

    var displayTitle: String { title ?? callsign }
    var isSimulation: Bool { kind == .simulation }
    var duration: Double { observations.last?.time ?? 0 }

    static func decode(_ data: Data) throws -> FlightTrack {
        guard data.count <= 8 * 1024 * 1024 else { throw Invalid.observations }
        let track = try JSONDecoder().decode(Self.self, from: data)
        guard (2...20000).contains(track.observations.count), track.observations.first?.time == 0 else {
            throw Invalid.observations
        }
        guard track.startUTC.isFinite else { throw Invalid.value }
        var previous = -1.0
        var preceding: Observation?
        for point in track.observations {
            guard point.time.isFinite, point.time <= 259200, point.time > previous else { throw Invalid.order }
            guard point.latitude.isFinite, abs(point.latitude) <= 84,
                  point.longitude.isFinite, abs(point.longitude) <= 180,
                  point.altitudeMSL.isFinite, (-500...20000).contains(point.altitudeMSL),
                  point.altitudeGNSS.map({ $0.isFinite && (-650...20150).contains($0) }) ?? true,
                  point.geoidSeparation.map({ $0.isFinite && abs($0) <= 150 }) ?? true,
                  point.groundSpeed.isFinite, (0...500).contains(point.groundSpeed),
                  point.track.isFinite, (0...360).contains(point.track),
                  point.roll.map({ $0.isFinite && abs($0) <= 180 }) ?? true,
                  point.pitch.map({ $0.isFinite && abs($0) <= 90 }) ?? true,
                  point.heading.map({ $0.isFinite && (0...360).contains($0) }) ?? true,
                  point.indicatedAirspeed.map({ $0.isFinite && (0...500).contains($0) }) ?? true else { throw Invalid.value }
            if let gnss = point.altitudeGNSS, let geoid = point.geoidSeparation {
                guard abs(gnss - geoid - point.altitudeMSL) < 0.1 else { throw Invalid.value }
            } else if point.altitudeGNSS != nil || point.geoidSeparation != nil { throw Invalid.value }
            if let preceding, point.startsSegment != true, point.time - preceding.time <= 20 {
                let dot = simd_dot(GlobeGeometry.direction(preceding.coordinate), GlobeGeometry.direction(point.coordinate))
                let distance = acos(min(max(dot, -1), 1)) * MapPlacement.earthRadiusMeters
                guard distance <= (point.time - preceding.time) * 500 + 300 else { throw Invalid.value }
            }
            previous = point.time
            preceding = point
        }
        return track
    }

    func sample(at seconds: Double) -> Observation? {
        guard seconds.isFinite, let first = observations.first, let last = observations.last else { return nil }
        let time = min(max(seconds, first.time), last.time)
        var low = 0, high = observations.count - 1
        while low + 1 < high {
            let middle = (low + high) / 2
            if observations[middle].time <= time { low = middle } else { high = middle }
        }
        let a = observations[low], b = observations[high]
        if time == a.time { return a }
        if time == b.time { return b }
        // Receiver outages must remain gaps, not plausible-looking invented flight paths.
        guard b.time - a.time <= 20, b.startsSegment != true else { return nil }
        let t = (time - a.time) / (b.time - a.time)
        let direction = simd_normalize(simd_mix(GlobeGeometry.direction(a.coordinate),
                                              GlobeGeometry.direction(b.coordinate), SIMD3(repeating: t)))
        let coordinate = GlobeGeometry.coordinate(direction)
        let lerp = { (x: Double, y: Double) in x + (y - x) * t }
        let headingDelta = atan2(sin((b.track - a.track) * .pi / 180), cos((b.track - a.track) * .pi / 180))
        return Observation(time: time, latitude: coordinate.latitude, longitude: coordinate.longitude,
            altitudeMSL: lerp(a.altitudeMSL, b.altitudeMSL), altitudeGNSS: a.altitudeGNSS.flatMap { x in b.altitudeGNSS.map { lerp(x, $0) } },
            geoidSeparation: a.geoidSeparation.flatMap { x in b.geoidSeparation.map { lerp(x, $0) } }, groundSpeed: lerp(a.groundSpeed, b.groundSpeed),
            track: (a.track + headingDelta * t * 180 / .pi + 360).truncatingRemainder(dividingBy: 360),
            roll: a.roll.flatMap { x in b.roll.map { Self.interpolateAngle(x, $0, t) } },
            pitch: a.pitch.flatMap { x in b.pitch.map { lerp(x, $0) } },
            heading: a.heading.flatMap { x in b.heading.map { (Self.interpolateAngle(x, $0, t) + 360).truncatingRemainder(dividingBy: 360) } },
            indicatedAirspeed: a.indicatedAirspeed.flatMap { x in b.indicatedAirspeed.map { lerp(x, $0) } },
            velocityAvailable: a.hasVelocity && b.hasVelocity)
    }
    static func interpolateAngle(_ a: Double, _ b: Double, _ t: Double) -> Double {
        let delta = atan2(sin((b - a) * .pi / 180), cos((b - a) * .pi / 180))
        return (a + delta * t * 180 / .pi + 540).truncatingRemainder(dividingBy: 360) - 180
    }

    func verticalSpeed(at time: Double) -> Double? {
        guard let a = sample(at: max(0, time - 1)), let b = sample(at: min(duration, time + 1)),
              b.time > a.time else { return nil }
        return (b.altitudeMSL - a.altitudeMSL) / (b.time - a.time)
    }
}

struct FlightPlayback {
    var offset = 0.0
    var rate = 1.0
    var started: Double?

    func time(at clock: Double, duration: Double) -> Double {
        min(max(offset + (started.map { max(clock - $0, 0) * rate } ?? 0), 0), duration)
    }

    mutating func pause(at clock: Double, duration: Double) {
        offset = time(at: clock, duration: duration)
        started = nil
    }

    mutating func play(at clock: Double, duration: Double) {
        if time(at: clock, duration: duration) >= duration { offset = 0 }
        started = clock
    }

    mutating func setRate(_ value: Double, at clock: Double, duration: Double) {
        guard [1.0, 4, 16].contains(value) else { return }
        let playing = started != nil
        pause(at: clock, duration: duration)
        rate = value
        if playing { started = clock }
    }
}
