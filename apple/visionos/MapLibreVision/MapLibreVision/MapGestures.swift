import CompositorServices
import Foundation
import Spatial
import SwiftUI
import simd

/// Hand input from the compositor, gathered into the moves visionOS's standard gestures
/// stand for: a pinch dragged moves, two pinches dragged apart or together zoom, two pinches
/// turned about each other rotate.
///
/// A single pinch also carries the ray it grabbed along: the ray the pinch began with,
/// turned about the head by the same angle the hand has turned since, so what it points at
/// can follow the hand however far away it is.
///
/// Spatial events arrive on the compositor's thread; the render thread takes the gathered
/// input once per frame and tells it where the head is.
final class MapGestures {
    /// Hands closer than this, measured level, give no zoom or twist: the span's length and
    /// heading between them would follow tracking noise rather than the hands.
    static let minimumSpanMeters = 0.05

    /// One pinch's travel since the last frame.
    struct Move {
        /// Travel of the hand in room metres.
        var travel: SIMD3<Double>
        /// The grabbed ray's origin, and its direction before and after the travel.
        var rayOrigin: SIMD3<Double>?
        var rayFrom: SIMD3<Double>?
        var rayTo: SIMD3<Double>?
    }

    /// Input gathered since the last frame.
    struct Delta {
        var moves: [Move] = []
        /// Natural log of how much two pinches moved apart; negative when together.
        var logScale = 0.0
        /// Turn of two pinches about the vertical, radians, counterclockwise seen from above.
        var turn = 0.0
        /// The ray the first hand of a pair grabbed, which a zoom keeps its point under.
        var zoomAnchor: (origin: SIMD3<Double>, direction: SIMD3<Double>)?
    }

    private struct Pinch {
        var hand: SIMD3<Double>
        /// The hand where the pinch began, and the ray it grabbed then.
        let firstHand: SIMD3<Double>
        let rayOrigin: SIMD3<Double>?
        let firstDirection: SIMD3<Double>?
        /// The grabbed ray as last reported.
        var direction: SIMD3<Double>?
    }

    private struct Pair {
        let ids: Set<SpatialEventCollection.Event.ID>
        let center: SIMD3<Double>
        let span: SIMD3<Double>
    }

    private let lock = NSLock()
    private var pinches: [SpatialEventCollection.Event.ID: Pinch] = [:]
    /// The order pinches began in, so the pair keeps its span's direction.
    private var order: [SpatialEventCollection.Event.ID] = []
    private var previousPair: Pair?
    /// What the hands of the current pair are doing; decided once from their first
    /// motion and held to the end of the pair, so a carry does not also zoom and a zoom
    /// does not also carry the scene.
    private var pairMode = PairMode.undecided(spanChange: 0, travel: 0)

    private enum PairMode {
        case undecided(spanChange: Double, travel: Double)
        case carry
        case zoom
    }

    /// Change of the hands' separation, as a ratio, that makes a pair a zoom.
    static let zoomDecisionRatio = 0.06
    /// Travel of the hands' midpoint, metres, that makes a pair a carry.
    static let carryDecisionMeters = 0.03
    private var pending = Delta()
    private var head = SIMD3<Double>(0, 0, 0)

    /// Where the head is, from the render thread, so a pinch's ray can turn about it.
    func updateHead(_ position: SIMD3<Double>) {
        lock.withLock { head = position }
    }

    func handle(_ events: SpatialEventCollection) {
        lock.withLock {
            for event in events {
                switch event.phase {
                case .active:
                    guard let pose = event.inputDevicePose else {
                        continue
                    }
                    let point = pose.pose3D.position
                    let hand = SIMD3<Double>(point.x, point.y, point.z)
                    if var pinch = pinches[event.id] {
                        let travel = hand - pinch.hand
                        pinch.hand = hand
                        let turned = pinch.firstDirection.map { first -> SIMD3<Double> in
                            let before = simd_normalize(pinch.firstHand - head)
                            let after = simd_normalize(hand - head)
                            guard before.x.isFinite, after.x.isFinite else {
                                return first
                            }
                            return simd_quatd(from: before, to: after).act(first)
                        }
                        // A single pinch moves; the pair below covers two.
                        if order.count == 1 {
                            pending.moves.append(Move(
                                travel: travel, rayOrigin: pinch.rayOrigin, rayFrom: pinch.direction,
                                rayTo: turned))
                        }
                        pinch.direction = turned
                        pinches[event.id] = pinch
                    } else {
                        let ray = event.selectionRay
                        let origin = ray.map { SIMD3<Double>($0.origin.x, $0.origin.y, $0.origin.z) }
                        let direction = ray.map { simd_normalize(SIMD3<Double>($0.direction.x, $0.direction.y, $0.direction.z)) }
                        pinches[event.id] = Pinch(
                            hand: hand, firstHand: hand, rayOrigin: origin, firstDirection: direction,
                            direction: direction)
                        order.append(event.id)
                    }
                default:
                    pinches[event.id] = nil
                    order.removeAll { $0 == event.id }
                }
            }
            let pair = self.pair()
            // A pinch that began or ended starts a new gesture; only the same hands moving
            // count as input.
            if let previous = previousPair, let pair, previous.ids == pair.ids {
                let travel = pair.center - previous.center
                // Hands close together make the span's length and heading ill-conditioned:
                // a centimetre of tracking noise would read as a large zoom or twist.
                let spanWas = simd_length(previous.span)
                let ratio = spanWas >= MapGestures.minimumSpanMeters
                    ? simd_length(pair.span) / spanWas : 1
                let logRatio = ratio.isFinite && ratio > 0 ? log(ratio) : 0
                if case .undecided(let spanChange, let travelled) = pairMode {
                    let spanChange = spanChange + abs(logRatio)
                    let travelled = travelled + simd_length(travel)
                    if spanChange >= MapGestures.zoomDecisionRatio {
                        pairMode = .zoom
                    } else if travelled >= MapGestures.carryDecisionMeters {
                        pairMode = .carry
                    } else {
                        pairMode = .undecided(spanChange: spanChange, travel: travelled)
                    }
                }
                switch pairMode {
                case .carry:
                    pending.moves.append(Move(travel: travel, rayOrigin: nil, rayFrom: nil, rayTo: nil))
                case .zoom:
                    if let first = order.first, let pinch = pinches[first], let origin = pinch.rayOrigin,
                       let direction = pinch.firstDirection
                    {
                        pending.zoomAnchor = (origin, direction)
                    }
                    pending.logScale += logRatio
                    let level = SIMD2<Double>(pair.span.x, -pair.span.z)
                    let levelWas = SIMD2<Double>(previous.span.x, -previous.span.z)
                    if simd_length(level) >= MapGestures.minimumSpanMeters,
                       simd_length(levelWas) >= MapGestures.minimumSpanMeters
                    {
                        var turn = atan2(level.y, level.x) - atan2(levelWas.y, levelWas.x)
                        if turn > .pi {
                            turn -= 2 * .pi
                        } else if turn < -.pi {
                            turn += 2 * .pi
                        }
                        pending.turn += turn
                    }
                case .undecided:
                    break
                }
            } else {
                pairMode = .undecided(spanChange: 0, travel: 0)
            }
            previousPair = pair
        }
    }

    private func pair() -> Pair? {
        let held = order.compactMap { id in pinches[id].map { (id, $0.hand) } }
        guard held.count >= 2 else {
            return nil
        }
        let (first, second) = (held[0], held[1])
        return Pair(ids: [first.0, second.0], center: (first.1 + second.1) / 2, span: second.1 - first.1)
    }

    /// The input since the last call.
    func take() -> Delta {
        lock.withLock {
            let delta = pending
            pending = Delta()
            return delta
        }
    }
}
