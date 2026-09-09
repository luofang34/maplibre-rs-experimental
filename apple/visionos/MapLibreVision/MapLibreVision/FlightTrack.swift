import Foundation
import simd

struct FlightTrack: Codable {
    struct Observation: Codable {
        let time: Double
        let latitude: Double
        let longitude: Double
        let altitudeMSL: Double
        let altitudeGNSS: Double
        let geoidSeparation: Double
        let groundSpeed: Double
        let track: Double
        let roll: Double?

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

    let callsign: String
    let registration: String
    let aircraft: String
    let destination: String
    let startUTC: Double
    let source: Provenance
    let observations: [Observation]

    var duration: Double { observations.last?.time ?? 0 }

    static func decode(_ data: Data) throws -> FlightTrack {
        let track = try JSONDecoder().decode(Self.self, from: data)
        guard (2...20000).contains(track.observations.count), track.observations.first?.time == 0 else {
            throw Invalid.observations
        }
        guard track.startUTC.isFinite else { throw Invalid.value }
        var previous = -1.0
        for point in track.observations {
            guard point.time.isFinite, point.time > previous else { throw Invalid.order }
            guard point.latitude.isFinite, abs(point.latitude) <= 90,
                  point.longitude.isFinite, abs(point.longitude) <= 180,
                  point.altitudeMSL.isFinite, (-500...20000).contains(point.altitudeMSL),
                  point.altitudeGNSS.isFinite, (-650...20150).contains(point.altitudeGNSS),
                  point.geoidSeparation.isFinite, abs(point.geoidSeparation) <= 150,
                  abs(point.altitudeGNSS - point.geoidSeparation - point.altitudeMSL) < 0.1,
                  point.groundSpeed.isFinite, (0...500).contains(point.groundSpeed),
                  point.track.isFinite, (0...360).contains(point.track),
                  point.roll.map({ $0.isFinite && abs($0) <= 180 }) ?? true else { throw Invalid.value }
            previous = point.time
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
        guard b.time - a.time <= 20 else { return nil }
        let t = (time - a.time) / (b.time - a.time)
        let direction = simd_normalize(simd_mix(GlobeGeometry.direction(a.coordinate),
                                              GlobeGeometry.direction(b.coordinate), SIMD3(repeating: t)))
        let coordinate = GlobeGeometry.coordinate(direction)
        let lerp = { (x: Double, y: Double) in x + (y - x) * t }
        let headingDelta = atan2(sin((b.track - a.track) * .pi / 180), cos((b.track - a.track) * .pi / 180))
        return Observation(time: time, latitude: coordinate.latitude, longitude: coordinate.longitude,
            altitudeMSL: lerp(a.altitudeMSL, b.altitudeMSL), altitudeGNSS: lerp(a.altitudeGNSS, b.altitudeGNSS),
            geoidSeparation: lerp(a.geoidSeparation, b.geoidSeparation), groundSpeed: lerp(a.groundSpeed, b.groundSpeed),
            track: (a.track + headingDelta * t * 180 / .pi + 360).truncatingRemainder(dividingBy: 360),
            roll: a.roll.flatMap { x in b.roll.map { lerp(x, $0) } })
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
