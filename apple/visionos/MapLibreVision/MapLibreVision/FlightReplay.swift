import Foundation

enum FlightView: String, CaseIterable {
    case fpv = "FPV", chase = "Chase", free = "Free"
}

/// Both eyes consume the same immutable recording and clock snapshot.
final class FlightReplay: @unchecked Sendable {
    struct Frame {
        let track: FlightTrack?
        let generation: UInt64
        let cameraRevision: UInt64
        let elapsed: Double
        let playing: Bool
        let view: FlightView
        let rate: Double
        let observation: FlightTrack.Observation?
        var enabled = true
        var returnSeconds: Int? = nil
        var following: Bool { view != .free }
    }

    private let lock = NSLock()
    private var enabled = false
    private var recording: FlightTrack?
    private var failure: String?
    private var clock = FlightPlayback()
    private var view = FlightView.free
    private var generation: UInt64 = 0
    private var cameraRevision: UInt64 = 0
    private var resumeOnBoard = false
    private var boarding = false
    private var returnDeadline: Double?
    private var returnView = FlightView.fpv

    var track: FlightTrack? { lock.withLock { recording } }
    var loadingError: String? { lock.withLock { failure } }

    init(bundle: Bundle = .main, restored: ReplayPreferences = .init()) {
        do {
            guard let url = bundle.url(forResource: "innsbruck-approach", withExtension: "json") else {
                throw CocoaError(.fileNoSuchFile)
            }
            recording = try FlightTrack.decode(Data(contentsOf: url))
            clock.offset = restored.playbackTime.isFinite ? min(max(restored.playbackTime, 0), recording?.duration ?? 0) : 0
            clock.rate = [1.0, 4, 16].contains(restored.playbackRate) ? restored.playbackRate : 1
            #if DEBUG
            let arguments = ProcessInfo.processInfo.arguments
            if let index = arguments.firstIndex(of: "--replay-rate"), index + 1 < arguments.count,
               let rate = Double(arguments[index + 1]), [1.0, 4, 16].contains(rate) { clock.rate = rate }
            #endif
        } catch { failure = "The bundled approach could not load: \(error.localizedDescription)" }
    }

    func replace(with track: FlightTrack?) {
        lock.withLock {
            returnDeadline = nil
            recording = track
            failure = nil
            clock = FlightPlayback()
            generation = generation &+ 1
            cameraRevision = cameraRevision &+ 1
            view = .free
            resumeOnBoard = false
            boarding = false
        }
    }

    func frame(at now: Double = ProcessInfo.processInfo.systemUptime) -> Frame {
        let (track, clock, view, generation, revision, deadline, enabled) = lock.withLock {
            (recording, self.clock, self.view, self.generation, cameraRevision, returnDeadline, self.enabled)
        }
        let elapsed = clock.time(at: now, duration: track?.duration ?? 0)
        return Frame(track: track, generation: generation, cameraRevision: revision, elapsed: elapsed,
            playing: clock.started != nil && elapsed < (track?.duration ?? 0), view: view, rate: clock.rate,
            observation: track?.sample(at: elapsed), enabled: enabled, returnSeconds: deadline.map { Int(ceil(max(0, $0 - now))) })
    }

    func setEnabled(_ enabled: Bool, at now: Double = ProcessInfo.processInfo.systemUptime) {
        lock.withLock {
            self.enabled = enabled
            if !enabled {
                clock.pause(at: now, duration: recording?.duration ?? 0)
                view = .free
                boarding = false
                resumeOnBoard = false
                returnDeadline = nil
                cameraRevision = cameraRevision &+ 1
            }
        }
    }

    func toggle(at now: Double = ProcessInfo.processInfo.systemUptime) {
        lock.withLock {
            if boarding { resumeOnBoard.toggle(); return }
            resumeOnBoard = false
            let duration = recording?.duration ?? 0
            if clock.started != nil, clock.time(at: now, duration: duration) < duration {
                clock.pause(at: now, duration: duration)
            } else { clock.play(at: now, duration: duration) }
        }
    }

    func pause(at now: Double = ProcessInfo.processInfo.systemUptime) {
        lock.withLock {
            clock.pause(at: now, duration: recording?.duration ?? 0)
            resumeOnBoard = false
        }
    }

    func seek(_ seconds: Double) {
        guard seconds.isFinite else { return }
        lock.withLock {
            clock.offset = min(max(seconds, 0), recording?.duration ?? 0)
            if clock.started != nil { clock.started = ProcessInfo.processInfo.systemUptime }
            generation = generation &+ 1
        }
    }

    func setRate(_ rate: Double) {
        lock.withLock { clock.setRate(rate, at: ProcessInfo.processInfo.systemUptime, duration: recording?.duration ?? 0) }
    }

    func setView(_ requested: FlightView, at now: Double = ProcessInfo.processInfo.systemUptime) {
        lock.withLock { setViewLocked(requested, at: now) }
    }

    func navigate(at now: Double = ProcessInfo.processInfo.systemUptime) {
        lock.withLock {
            if view != .free {
                returnView = view
                setViewLocked(.free, at: now)
                returnDeadline = now + 10
            } else if returnDeadline != nil { returnDeadline = now + 10 }
        }
    }

    func advanceReturn(at now: Double = ProcessInfo.processInfo.systemUptime) {
        lock.withLock {
            guard let deadline = returnDeadline, now >= deadline else { return }
            setViewLocked(returnView, at: now)
        }
    }

    private func setViewLocked(_ requested: FlightView, at now: Double) {
        returnDeadline = nil
        guard requested != view else { return }
        let duration = recording?.duration ?? 0
        let playing = clock.started != nil && clock.time(at: now, duration: duration) < duration
        resumeOnBoard = resumeOnBoard || playing
        clock.pause(at: now, duration: duration)
        view = requested
        boarding = requested != .free
        cameraRevision = cameraRevision &+ 1
    }

    func follow(_ enabled: Bool) { setView(enabled ? .fpv : .free) }

    func recenter() {
        lock.withLock { cameraRevision = cameraRevision &+ 1 }
    }

    func boarded(revision: UInt64, at now: Double = ProcessInfo.processInfo.systemUptime) {
        lock.withLock {
            guard cameraRevision == revision, boarding else { return }
            boarding = false
            if resumeOnBoard { clock.play(at: now, duration: recording?.duration ?? 0) }
            resumeOnBoard = false
        }
    }
}
