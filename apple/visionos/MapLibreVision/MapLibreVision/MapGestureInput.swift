import simd

/// Render-thread input independent of the platform event collection.
enum MapGestureInput {
    /// One pinch's travel since the last frame.
    struct Move {
        /// Travel of the hand in room metres.
        var travel: SIMD3<Double>
        /// Starts a new drag mode, held until the pinch ends.
        var beginsGesture = false
        /// The grabbed ray's origin, and its direction before and after the travel.
        var rayOrigin: SIMD3<Double>?
        var rayFrom: SIMD3<Double>?
        var rayTo: SIMD3<Double>?
    }

    struct CarryReference {
        var origin: SIMD3<Double>
        var handDepth: Double
    }

    /// Input gathered since the last frame.
    struct Delta {
        var moves: [Move] = []
        var translation = SIMD3<Double>.zero
        var carryReference: CarryReference?
        /// Natural log of how much two pinches moved apart; negative when together.
        var logScale = 0.0
        var beginsZoom = false
        var focusAnchor: (origin: SIMD3<Double>, direction: SIMD3<Double>)?
        /// Turn of two pinches about the vertical, radians, counterclockwise seen from above.
        var turn = 0.0
        var pitch = 0.0
        var beginsOrbit = false
        var orbitAnchor: (origin: SIMD3<Double>, direction: SIMD3<Double>)?
        /// Midpoint of the selection rays, used when the primary focus misses the surface.
        var zoomAnchor: (origin: SIMD3<Double>, direction: SIMD3<Double>)?
        /// A short stationary pinch selects the rendered label under its ray.
        var selection: (origin: SIMD3<Double>, direction: SIMD3<Double>)?

        var isEmpty: Bool {
            moves.isEmpty && translation == .zero && logScale == 0 && turn == 0 && pitch == 0 && selection == nil
        }
    }

    struct Buffer {
        private var events: [Delta] = []

        mutating func append(_ input: Delta) -> Bool {
            guard !input.isEmpty else { return true }
            guard events.count < 64 else { events.removeAll(keepingCapacity: true); return false }
            events.append(input)
            return true
        }

        mutating func take() -> [Delta] {
            defer { events.removeAll(keepingCapacity: true) }
            return events
        }
    }

}
