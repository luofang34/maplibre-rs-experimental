import Foundation

/// Deadlines stay on a fixed cadence instead of drifting to the next display frame.
struct FlightHUDCadence {
    private let period = 1.0 / 30.0
    private var deadline: Double?

    mutating func consume(now: Double, changed: Bool, urgent: Bool) -> Bool {
        guard changed || urgent else { return false }
        if !urgent, let deadline, now < deadline { return false }
        if urgent || deadline == nil {
            deadline = now + period
        } else {
            let next = (deadline ?? now) + period
            deadline = next > now ? next : now + period
        }
        return true
    }
}
