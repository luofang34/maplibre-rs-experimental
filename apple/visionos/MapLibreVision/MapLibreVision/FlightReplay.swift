import Foundation

/// A single clock and immutable recording keep the desk globe and both terrain eyes in step.
final class FlightReplay: @unchecked Sendable {
    struct Frame {
        let elapsed: Double
        let playing: Bool
        let following: Bool
        let rate: Double
        let observation: FlightTrack.Observation?
    }

    let track: FlightTrack?
    let loadingError: String?
    private let lock = NSLock()
    private var clock = FlightPlayback()
    private var follows = false

    init(bundle: Bundle = .main, restored: DeskGlobeState = .init()) {
        do {
            guard let url = bundle.url(forResource: "innsbruck-approach", withExtension: "json") else {
                throw CocoaError(.fileNoSuchFile)
            }
            let loaded = try FlightTrack.decode(Data(contentsOf: url))
            track = loaded
            loadingError = nil
            clock.offset = restored.playbackTime.isFinite ? min(max(restored.playbackTime, 0), loaded.duration) : 0
            clock.rate = [1.0, 4, 16].contains(restored.playbackRate) ? restored.playbackRate : 1
            #if DEBUG
            let arguments = ProcessInfo.processInfo.arguments
            if let index = arguments.firstIndex(of: "--replay-rate"), index + 1 < arguments.count,
               let rate = Double(arguments[index + 1]), [1.0, 4, 16].contains(rate) { clock.rate = rate }
            #endif
        } catch {
            track = nil
            loadingError = "The bundled approach could not load: \(error.localizedDescription)"
        }
    }

    func frame(at now: Double = ProcessInfo.processInfo.systemUptime) -> Frame {
        let state = lock.withLock { (clock, follows) }
        let elapsed = state.0.time(at: now, duration: track?.duration ?? 0)
        return Frame(elapsed: elapsed, playing: state.0.started != nil && elapsed < (track?.duration ?? 0),
                     following: state.1, rate: state.0.rate, observation: track?.sample(at: elapsed))
    }

    func toggle() {
        lock.withLock {
            let now = ProcessInfo.processInfo.systemUptime, duration = track?.duration ?? 0
            if clock.started != nil, clock.time(at: now, duration: duration) < duration {
                clock.pause(at: now, duration: duration)
            } else { clock.play(at: now, duration: duration) }
        }
    }

    func pause() {
        lock.withLock { clock.pause(at: ProcessInfo.processInfo.systemUptime, duration: track?.duration ?? 0) }
    }

    func seek(_ seconds: Double) {
        guard seconds.isFinite else { return }
        lock.withLock {
            clock.offset = min(max(seconds, 0), track?.duration ?? 0)
            if clock.started != nil { clock.started = ProcessInfo.processInfo.systemUptime }
        }
    }

    func setRate(_ rate: Double) {
        lock.withLock { clock.setRate(rate, at: ProcessInfo.processInfo.systemUptime, duration: track?.duration ?? 0) }
    }

    func follow(_ enabled: Bool) { lock.withLock { follows = enabled } }
}
