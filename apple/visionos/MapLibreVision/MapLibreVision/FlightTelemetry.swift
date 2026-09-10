import Foundation

enum FlightTelemetry {
    static func resolve(_ frame: FlightReplay.Frame) -> ReplayTelemetry {
        var input = ReplayTelemetry()
        if let point = frame.observation {
            input.present = 1 | (point.hasVelocity ? 32 : 0)
            input.ground_speed = Float(point.groundSpeed)
            input.track = Float(point.track * .pi / 180)
            input.altitude_msl = Float(point.altitudeMSL)
            if let vertical = frame.track?.verticalSpeed(at: frame.elapsed) {
                input.present |= 2
                input.vertical_speed = Float(vertical)
            }
            if let ias = point.indicatedAirspeed { input.present |= 4; input.ias = Float(ias) }
            if point.hasAttitude {
                input.present |= 8
                input.roll = Float((point.roll ?? 0) * .pi / 180)
                input.pitch = Float((point.pitch ?? 0) * .pi / 180)
            }
            if let heading = point.heading {
                input.present |= 16
                input.heading = Float(heading * .pi / 180)
            }
        }
        return input
    }
}
