import simd

/// Resolves a complete event batch before emitting input, so adding a hand cannot leak a drag.
struct MapGestureRecognizer<ID: Hashable> {
    struct Sample {
        var id: ID
        var position: SIMD3<Double>?
        var rayOrigin: SIMD3<Double>?
        var rayDirection: SIMD3<Double>?
        var timestamp = 0.0
        var cancelled = false
    }

    private struct Pinch {
        var hand: SIMD3<Double>
        var firstHand: SIMD3<Double>
        var referenceHead: SIMD3<Double>
        var origin: SIMD3<Double>?
        var firstDirection: SIMD3<Double>?
        var direction: SIMD3<Double>?
        var hasMoved = false
        var tapEligible = true
        var started = 0.0
    }

    private struct Pair {
        var distance: Double
        var angle: Double
        var center: SIMD3<Double>
    }

    private enum PairMode { case undecided, zoom, rotate, translate, orbit }
    private var pinches: [ID: Pinch] = [:]
    private var order: [ID] = []
    private var baseline: Pair?
    private var previous: Pair?
    private var pairMode = PairMode.undecided
    private var pairRight = SIMD3<Double>(1, 0, 0)
    private var pairUp = SIMD3<Double>(0, 1, 0)
    private var isGlobe = true
    private var suppressUntilReleased = false
    private var beginsZoom = false
    var head = SIMD3<Double>.zero
    var right = SIMD3<Double>(1, 0, 0)
    var up = SIMD3<Double>(0, 1, 0)

    @discardableResult
    mutating func setGlobe(_ wanted: Bool) -> Bool {
        guard isGlobe != wanted else { return false }
        isGlobe = wanted
        suppressUntilReleased = !pinches.isEmpty && pairMode != .zoom && pairMode != .translate
        return suppressUntilReleased
    }

    mutating func handle(_ samples: [Sample]) -> MapGestureInput.Delta {
        let before = pinches
        var selection: (origin: SIMD3<Double>, direction: SIMD3<Double>)?
        for sample in samples {
            guard let hand = sample.position else {
                if before.count == 1, let pinch = before[sample.id], pinch.tapEligible,
                   !pinch.hasMoved, !sample.cancelled, !suppressUntilReleased,
                   sample.timestamp - pinch.started <= 0.45,
                   let origin = pinch.origin, let direction = pinch.firstDirection {
                    selection = (origin, direction)
                }
                pinches[sample.id] = nil
                order.removeAll { $0 == sample.id }
                continue
            }
            if var pinch = pinches[sample.id] {
                pinch.hand = hand
                pinches[sample.id] = pinch
            } else {
                pinches[sample.id] = Pinch(
                    hand: hand, firstHand: hand, referenceHead: head,
                    origin: sample.rayOrigin, firstDirection: sample.rayDirection,
                    direction: sample.rayDirection, started: sample.timestamp)
                order.append(sample.id)
            }
        }
        if pinches.count > 1 {
            for id in order { pinches[id]?.tapEligible = false }
        }
        if pinches.isEmpty { suppressUntilReleased = false }
        if Set(before.keys) != Set(pinches.keys) {
            rebase()
            return .init(selection: selection)
        }
        guard !suppressUntilReleased else { return .init() }
        if pinches.count == 1, let id = order.first, let was = before[id] {
            return single(id, was: was)
        }
        return paired()
    }

    private mutating func rebase() {
        // A surviving hand starts from its current ray when the other hand releases.
        for id in order {
            guard var pinch = pinches[id] else { continue }
            pinch.direction = turned(pinch)
            pinch.firstDirection = pinch.direction
            pinch.firstHand = pinch.hand
            pinch.referenceHead = head
            pinch.hasMoved = false
            pinches[id] = pinch
        }
        pairRight = right
        pairUp = up
        baseline = pair()
        previous = baseline
        pairMode = .undecided
        beginsZoom = false
    }

    private func turned(_ pinch: Pinch) -> SIMD3<Double>? {
        guard let first = pinch.firstDirection else { return nil }
        let before = pinch.firstHand - pinch.referenceHead
        let after = pinch.hand - pinch.referenceHead
        guard simd_length(before) > 1e-4, simd_length(after) > 1e-4 else { return first }
        return simd_quatd(from: simd_normalize(before), to: simd_normalize(after)).act(first)
    }

    private mutating func single(_ id: ID, was: Pinch) -> MapGestureInput.Delta {
        guard var pinch = pinches[id] else { return .init() }
        let travel = pinch.hand - (pinch.hasMoved ? was.hand : pinch.firstHand)
        guard pinch.hasMoved || simd_length(travel) >= 0.004 else { return .init() }
        let direction = turned(pinch)
        defer {
            pinch.direction = direction
            pinch.hasMoved = true
            pinches[id] = pinch
        }
        return .init(moves: [.init(
            travel: travel, beginsGesture: !pinch.hasMoved,
            rayOrigin: pinch.origin, rayFrom: pinch.direction, rayTo: direction)])
    }

    private func pair() -> Pair? {
        guard order.count == 2, let a = pinches[order[0]], let b = pinches[order[1]] else { return nil }
        let span = b.hand - a.hand
        return Pair(distance: simd_length(span),
                    angle: atan2(simd_dot(span, pairUp), simd_dot(span, pairRight)),
                    center: (a.hand + b.hand) / 2)
    }

    private mutating func paired() -> MapGestureInput.Delta {
        guard let now = pair(), let initial = baseline, let was = previous else { return .init() }
        defer { previous = now }
        guard min(now.distance, initial.distance, was.distance) >= 0.02 else {
            baseline = now
            pairMode = .undecided
            return .init()
        }
        let zoom = log(now.distance / initial.distance)
        let twist = wrapped(now.angle - initial.angle)
        var beginsOrbit = false
        if pairMode == .undecided {
            // Net displacement, rather than accumulated absolute noise, determines intent.
            let zoomScore = abs(zoom) / 0.06
            let turnScore = abs(twist) / (3 * .pi / 180)
            let travel = now.center - initial.center
            let planar = SIMD2<Double>(simd_dot(travel, pairRight), simd_dot(travel, pairUp))
            let moveScore = isGlobe ? simd_length(travel) / 0.025 : simd_length(planar) / 0.015
            if isGlobe && coherentCarry() {
                pairMode = .translate
            } else if zoomScore >= 1, zoomScore > max(turnScore, moveScore) * 1.15 {
                pairMode = .zoom
                beginsZoom = true
            }
            if pairMode == .undecided, turnScore >= 1, turnScore > max(zoomScore, moveScore) * 1.15 { pairMode = .rotate }
            if pairMode == .undecided, moveScore >= 1, moveScore > max(zoomScore, turnScore) * 1.15 {
                pairMode = isGlobe ? .translate : .orbit
            }
            beginsOrbit = pairMode == .rotate || pairMode == .orbit
            // Zoom and carry consume their threshold to avoid a placement jump.
            if !beginsOrbit { return .init() }
        }
        switch pairMode {
        case .zoom:
            defer { beginsZoom = false }
            return .init(logScale: log(now.distance / was.distance), beginsZoom: beginsZoom,
                         focusAnchor: primaryAnchor(), zoomAnchor: pairAnchor())
        case .rotate:
            return .init(turn: wrapped(now.angle - was.angle), beginsOrbit: beginsOrbit, orbitAnchor: pairAnchor())
        case .orbit:
            let travel = now.center - was.center
            return .init(turn: simd_dot(travel, pairRight) * 2.5,
                         pitch: simd_dot(travel, pairUp) * 2.5, beginsOrbit: beginsOrbit, orbitAnchor: pairAnchor())
        case .translate:
            return .init(translation: now.center - was.center)
        case .undecided:
            return .init()
        }
    }

    private func coherentCarry() -> Bool {
        guard order.count == 2, let a = pinches[order[0]], let b = pinches[order[1]] else { return false }
        let first = a.hand - a.firstHand
        let second = b.hand - b.firstHand
        let common = (first + second) / 2
        let differential = (second - first) / 2
        return min(simd_length(first), simd_length(second)) >= 0.012
            && simd_dot(first, second) > 0
            && simd_length(common) > 2 * simd_length(differential)
    }

    private func primaryAnchor() -> (origin: SIMD3<Double>, direction: SIMD3<Double>)? {
        guard let id = order.first, let pinch = pinches[id],
              let origin = pinch.origin, let direction = pinch.firstDirection else { return nil }
        return (origin, direction)
    }

    private func pairAnchor() -> (origin: SIMD3<Double>, direction: SIMD3<Double>)? {
        let rays = order.compactMap { id -> (SIMD3<Double>, SIMD3<Double>)? in
            guard let pinch = pinches[id], let origin = pinch.origin,
                  let direction = pinch.firstDirection else { return nil }
            return (origin, direction)
        }
        guard rays.count == 2 else { return nil }
        let direction = rays[0].1 + rays[1].1
        guard simd_length(direction) > 1e-6 else { return nil }
        return ((rays[0].0 + rays[1].0) / 2, simd_normalize(direction))
    }

    private func wrapped(_ angle: Double) -> Double { atan2(sin(angle), cos(angle)) }
}
