import simd

/// A place on the globe the map's scene is anchored to.
struct MapAnchor {
    let latitude: Double
    let longitude: Double
    let altitudeMeters: Double

    /// Innsbruck: the world first appears above the Inn valley with the Alps as terrain, and
    /// the table globe turns the same point towards the viewer.
    static let innsbruck = MapAnchor(latitude: 47.26, longitude: 11.39, altitudeMeters: 0)
}

/// The viewer's place over the world: the point of the surface under them, or facing them
/// on the globe, how high above it they stand in world metres, and which way the world is
/// turned. One continuous height runs from the street to the globe on the table, so a zoom
/// out from the ground ends at the globe and a zoom into the globe ends on the ground.
struct Viewpoint {
    var latitude: Double
    var longitude: Double
    /// Height above the focus, world metres.
    var height: Double
    /// The world's turn about the viewer's vertical, radians, counterclockwise from above.
    var bearing: Double
    var globeRoll = 0.0
    var tilt = 0.0

    static func above(_ anchor: MapAnchor, height: Double) -> Viewpoint {
        Viewpoint(latitude: anchor.latitude, longitude: anchor.longitude, height: height, bearing: 0)
    }
}

/// The room's immersion the scene needs: passthrough around a globe, or the world alone.
enum MapImmersion {
    case mixed
    case full
}

enum MapCameraPolicy {
    case freeOrbit
    case fixedViewpoint
}

/// Where the scene stands in the room: a rotation, a position and a scale, kept apart so a
/// move between two placements can interpolate each on its own terms.
struct ScenePose {
    var rotation: simd_quatd
    var translation: SIMD3<Double>
    /// Natural logarithm of the room metres per scene metre, so a change of size eases
    /// through every magnitude on the way.
    var logScale: Double

    func worldFromScene() -> simd_double4x4 {
        var matrix = simd_double4x4(rotation)
        let scale = exp(logScale)
        matrix.columns.0 *= scale
        matrix.columns.1 *= scale
        matrix.columns.2 *= scale
        matrix.columns.3 = SIMD4<Double>(translation, 1)
        return matrix
    }

    /// The pose a share `t` of the way from `from` to `to`, seen from `viewer`.
    ///
    /// The scene origin keeps to the arc between its two directions from the viewer while
    /// its distance and the scale ease through every magnitude, so a globe on a table grows
    /// into the ground under the viewer's feet instead of sweeping past them.
    static func flight(
        from: ScenePose, to: ScenePose, _ t: Double, viewer: SIMD3<Double>
    ) -> ScenePose {
        let fromOffset = from.translation - viewer
        let toOffset = to.translation - viewer
        let fromDistance = max(simd_length(fromOffset), 1e-3)
        let toDistance = max(simd_length(toOffset), 1e-3)
        let direction = slerp(fromOffset / fromDistance, toOffset / toDistance, t)
        let distance = exp(log(fromDistance) + (log(toDistance) - log(fromDistance)) * t)
        return ScenePose(
            rotation: simd_slerp(from.rotation, to.rotation, t),
            translation: viewer + direction * distance,
            logScale: from.logScale + (to.logScale - from.logScale) * t)
    }

    /// Unit vector a share `t` of the way along the shorter arc from `a` to `b`.
    private static func slerp(_ a: SIMD3<Double>, _ b: SIMD3<Double>, _ t: Double) -> SIMD3<Double> {
        let cosine = min(max(simd_dot(a, b), -1), 1)
        let angle = acos(cosine)
        if angle < 1e-6 {
            return simd_normalize(simd_mix(a, b, SIMD3<Double>(repeating: t)))
        }
        if angle > .pi - 1e-6 {
            // Opposite directions: any arc will do, so turn about a perpendicular axis.
            var axis = simd_cross(a, SIMD3<Double>(0, 1, 0))
            if simd_length(axis) < 1e-6 {
                axis = simd_cross(a, SIMD3<Double>(1, 0, 0))
            }
            return simd_quatd(angle: angle * t, axis: simd_normalize(axis)).act(a)
        }
        return (sin((1 - t) * angle) * a + sin(t * angle) * b) / sin(angle)
    }
}

