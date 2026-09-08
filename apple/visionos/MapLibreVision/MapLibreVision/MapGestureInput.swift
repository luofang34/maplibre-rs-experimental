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

    /// Input gathered since the last frame.
    struct Delta {
        var moves: [Move] = []
        var translation = SIMD3<Double>.zero
        /// Natural log of how much two pinches moved apart; negative when together.
        var logScale = 0.0
        /// Turn of two pinches about the vertical, radians, counterclockwise seen from above.
        var turn = 0.0
        var pitch = 0.0
        var beginsOrbit = false
        /// The ray the first hand of a pair grabbed, which a zoom keeps its point under.
        var zoomAnchor: (origin: SIMD3<Double>, direction: SIMD3<Double>)?
        /// A short stationary pinch selects the rendered label under its ray.
        var selection: (origin: SIMD3<Double>, direction: SIMD3<Double>)?
    }

}
