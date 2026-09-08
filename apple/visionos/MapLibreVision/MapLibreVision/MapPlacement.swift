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

/// The scene's place in the room for a viewpoint, the gestures that move the viewpoint, and
/// the flights the launcher's buttons start.
struct MapPlacement {
    static let earthRadiusMeters = 6_371_008.8
    /// Radius of the globe on the table, room metres.
    static let tableRadius = 0.15
    /// Below this height the world lies level under the viewer at full size; above it the
    /// scene climbs the bridge towards the globe on the table.
    static let groundHeightLimit = 60_000.0
    /// The height that puts the viewer at the table, the top of the range.
    static let tableHeight = 40_000_000.0
    /// The lowest height, above the terrain at the focus.
    static let minHeight = 150.0
    /// The height the Immersive button flies to.
    static let flyToHeight = 4000.0
    /// How long a flight between the two buttons' heights takes.
    static let transitionSeconds = 6.0
    /// The room gives way to the world below this share of the bridge, counted from the
    /// ground, and returns above a higher one, so a zoom that hovers near the boundary does
    /// not flap the immersion.
    static let fullImmersionBelowShare = 0.55
    static let mixedImmersionAboveShare = 0.7
    /// A drag whose ray meets the surface further than this many heights away pulls as if
    /// it had grabbed a point at that distance, so a drag near the horizon cannot sling the
    /// world by thousands of kilometres.
    static let maxGrabDistanceInHeights = 20.0
    /// How many times the pinch ray's own turn the surface under it may turn in one move.
    /// Where the ray grazes the limb, a small change of the ray moves its surface point far
    /// around the globe; the turn is held to what the hand's motion warrants.
    static let maxTurnPerRayTurn = 3.0
    /// Latitudes beyond this fold the Mercator plane; the focus stays inside.
    static let latitudeLimit = 84.0

    private struct Flight {
        let from: Viewpoint
        let to: Viewpoint
        let fromPose: ScenePose
        let toPose: ScenePose
        let start: Double
    }

    private(set) var viewpoint: Viewpoint
    private(set) var current: ScenePose
    private(set) var immersion: MapImmersion
    private var flight: Flight?
    /// Whether this pinch began on the rendered sphere; crossing its edge keeps the mode.
    private var globeGrabbed: Bool?
    /// Where the viewer stood when the scene was last placed about them; the world keeps to
    /// that place while they move about the room.
    private var viewerReference = SIMD3<Double>(0, 0, 0)
    let cameraPolicy: MapCameraPolicy
    private var viewRay: (origin: SIMD3<Double>, direction: SIMD3<Double>)?
    private var orbitTarget: (local: SIMD3<Double>, world: SIMD3<Double>)?
    private var sceneOffset = SIMD3<Double>.zero
    private var sceneRotation = simd_quatd(angle: 0, axis: SIMD3<Double>(0, 1, 0))
    private var tableRotation: simd_quatd?
    private var immersiveTilt = Double.pi / 4
    private var orbitRight = SIMD3<Double>(1, 0, 0)
    private var orbitUp = SIMD3<Double>(0, 1, 0)
    /// Terrain elevation at the focus, metres, as the map last reported it.
    var focusElevation = 0.0

    /// Where the table globe's center goes: a metre ahead of the viewer, a little below the
    /// eyes, set once the head is tracked.
    var tableCenter = SIMD3<Double>(0, 1.1, -1.0)

    init(viewpoint: Viewpoint, cameraPolicy: MapCameraPolicy = .freeOrbit) {
        self.viewpoint = viewpoint
        self.cameraPolicy = cameraPolicy
        current = ScenePose(rotation: simd_quatd(angle: 0, axis: SIMD3<Double>(0, 1, 0)), translation: .zero, logScale: 0)
        immersion = MapPlacement.immersion(forHeight: viewpoint.height)
        current = pose(for: viewpoint)
    }

    /// The immersion a height starts in: the world below the middle of the bridge, the room
    /// above it.
    static func immersion(forHeight height: Double) -> MapImmersion {
        let low = log(groundHeightLimit)
        let high = log(tableHeight)
        let share = min(max((log(height) - low) / (high - low), 0), 1)
        return share < fullImmersionBelowShare ? .full : .mixed
    }

    /// Whether a flight is under way.
    var inFlight: Bool { flight != nil }

    /// The anchor the map's scene is placed at: the focus on the terrain.
    var anchor: MapAnchor {
        MapAnchor(latitude: viewpoint.latitude, longitude: viewpoint.longitude, altitudeMeters: focusElevation)
    }

    /// Places the scene about `viewer`; the world stays put about that place afterwards.
    mutating func place(viewer: SIMD3<Double>) {
        viewerReference = viewer
        tableRotation = nil
        tableRotation = tablePose().rotation
        orbitTarget = nil
        current = pose(for: viewpoint)
    }

    mutating func updateViewRay(origin: SIMD3<Double>, direction: SIMD3<Double>) {
        guard simd_length(direction) > 0.5 else { return }
        viewRay = (origin, simd_normalize(direction))
    }

    mutating func setTilt(_ radians: Double) {
        rotate(bearing: 0, pitch: radians - viewpoint.tilt, begins: false)
    }

    private mutating func rotate(
        bearing: Double, pitch: Double, begins: Bool,
        anchor: (origin: SIMD3<Double>, direction: SIMD3<Double>)? = nil
    ) {
        if orbitTarget == nil || begins { captureOrbitTarget(anchor: anchor) }
        let before = current
        let nextTilt = min(max(viewpoint.tilt + pitch, 0), 70 * .pi / 180)
        let appliedPitch = nextTilt - viewpoint.tilt
        viewpoint.bearing += bearing
        viewpoint.tilt = nextTilt
        let yaw = simd_quatd(angle: bearing, axis: orbitUp)
        let pitchTurn = simd_quatd(angle: appliedPitch, axis: orbitRight)
        let posed = pose(for: viewpoint)
        sceneRotation = pitchTurn * yaw * before.rotation * posed.rotation.inverse * sceneRotation
        current = pose(for: viewpoint)
        if let target = orbitTarget {
            let moved = current.translation + current.rotation.act(target.local * exp(current.logScale))
            sceneOffset += target.world - moved
            current = pose(for: viewpoint)
        }
    }

    private mutating func captureOrbitTarget(
        anchor: (origin: SIMD3<Double>, direction: SIMD3<Double>)?
    ) {
        let scale = exp(current.logScale)
        let radius = MapPlacement.earthRadiusMeters * scale
        let up = current.rotation.act(SIMD3<Double>(0, 0, 1))
        let center = current.translation - up * radius
        let ray = anchor ?? viewRay
        let surface = ray.flatMap {
            hit(origin: $0.origin, direction: $0.direction, center: center,
                radius: radius, reach: MapPlacement.maxGrabDistanceInHeights * viewpoint.height * scale)
        } ?? current.translation
        orbitUp = simd_normalize(surface - center)
        let right = simd_cross(ray?.direction ?? SIMD3<Double>(0, 0, -1), orbitUp)
        orbitRight = simd_length(right) > 1e-6 ? simd_normalize(right) : current.rotation.act(SIMD3<Double>(1, 0, 0))
        let target = cameraPolicy == .fixedViewpoint ? (viewRay?.origin ?? viewerReference) : surface
        orbitTarget = (current.rotation.inverse.act(target - current.translation) / scale, target)
    }

    mutating func levelView() {
        viewpoint.tilt = 0
        viewpoint.bearing = 0
        viewpoint.globeRoll = 0
        orbitTarget = nil
        sceneOffset = .zero
        sceneRotation = simd_quatd(angle: 0, axis: SIMD3<Double>(0, 1, 0))
        current = pose(for: viewpoint)
    }

    /// Starts a flight to `height` from wherever the viewpoint is.
    mutating func fly(to height: Double, at time: Double, viewer: SIMD3<Double>) {
        let from = viewpoint
        let fromPose = current
        if viewpoint.height <= MapPlacement.groundHeightLimit { immersiveTilt = viewpoint.tilt }
        viewerReference = viewer
        orbitTarget = nil
        sceneOffset = .zero
        sceneRotation = simd_quatd(angle: 0, axis: SIMD3<Double>(0, 1, 0))
        var arrival = viewpoint
        arrival.height = height
        arrival.globeRoll = 0
        arrival.tilt = height <= MapPlacement.groundHeightLimit ? immersiveTilt : 0
        let heading = viewRay.flatMap { ray -> Double? in
            guard simd_length(SIMD2<Double>(ray.direction.x, ray.direction.z)) > 1e-4 else { return nil }
            return atan2(-ray.direction.x, -ray.direction.z)
        } ?? viewpoint.bearing
        arrival.bearing = height <= MapPlacement.groundHeightLimit ? heading : 0
        flight = Flight(from: from, to: arrival, fromPose: fromPose,
                        toPose: pose(for: arrival), start: time)
    }

    func destination() -> ScenePose? { flight?.toPose }

    mutating func advance(at time: Double) -> MapImmersion? {
        if let flight {
            let t = min(max((time - flight.start) / MapPlacement.transitionSeconds, 0), 1)
            let eased = t * t * (3 - 2 * t)
            viewpoint.height = exp(log(flight.from.height) + (log(flight.to.height) - log(flight.from.height)) * eased)
            viewpoint.tilt = flight.from.tilt + (flight.to.tilt - flight.from.tilt) * eased
            viewpoint.bearing = flight.from.bearing + (flight.to.bearing - flight.from.bearing) * eased
            viewpoint.globeRoll = flight.from.globeRoll * (1 - eased)
            current = ScenePose.flight(from: flight.fromPose, to: flight.toPose, eased, viewer: viewerReference)
            if t >= 1 {
                viewpoint = flight.to
                self.flight = nil
                current = pose(for: viewpoint)
            }
        } else {
            current = pose(for: viewpoint)
        }
        let share = bridgeShare(viewpoint.height)
        let wanted: MapImmersion? = share < MapPlacement.fullImmersionBelowShare
            ? .full : share > MapPlacement.mixedImmersionAboveShare ? .mixed : nil
        if let wanted, wanted != immersion {
            immersion = wanted
            return wanted
        }
        return nil
    }

    /// Applies hand input to the viewpoint; input during a flight is dropped.
    /// The rendered surface follows the hand: the point a pinch's ray grabbed keeps to the ray
    /// while the hand moves, whether that surface is the ground under the viewer or the globe
    /// at its rendered size. A pinch begun outside the globe keeps the table globe's angular
    /// speed as its size changes. Ground drags beyond reach pull at the reach limit.
    /// Explicit placement input carries the globe; paired navigation input zooms or rotates it.
    mutating func apply(_ input: MapGestureInput.Delta) {
        guard flight == nil else {
            return
        }
        let onGround = viewpoint.height <= MapPlacement.groundHeightLimit
        if !input.moves.isEmpty || input.logScale != 0 { orbitTarget = nil }
        if !onGround {
            var travel = input.translation
            for move in input.moves where move.rayOrigin == nil { travel += move.travel }
            let before = current.translation
            tableCenter += travel
            sceneOffset += before + travel - pose(for: viewpoint).translation
        }
        let scale = exp(current.logScale)
        let up = current.rotation.act(SIMD3<Double>(0, 0, 1))
        let radius = MapPlacement.earthRadiusMeters * scale
        let center = current.translation - up * radius
        let reach = MapPlacement.maxGrabDistanceInHeights * max(viewpoint.height, 1.0) * scale
        var turn = simd_quatd(angle: 0, axis: up)
        var slide = SIMD3<Double>(repeating: 0)
        for move in input.moves {
            if let origin = move.rayOrigin, let from = move.rayFrom, let to = move.rayTo {
                if !onGround {
                    slide += dragGlobe(move, origin: origin, from: from, to: to,
                                       center: center, radius: radius, reach: reach)
                    continue
                }
                globeGrabbed = nil
                if let grabbed = hit(origin: origin, direction: from, center: center, radius: radius, reach: reach),
                   let pulled = hit(origin: origin, direction: to, center: center, radius: radius, reach: reach)
                {
                    let before = simd_normalize(grabbed - center)
                    let after = simd_normalize(pulled - center)
                    if before.x.isFinite, after.x.isFinite {
                        let rayTurn = acos(min(max(simd_dot(simd_normalize(from), simd_normalize(to)), -1), 1))
                        let distance = simd_length(origin - center)
                        let limit = rayTurn * MapPlacement.maxTurnPerRayTurn * max(distance / radius, 1)
                        var move = simd_quatd(from: before, to: after)
                        if move.angle > limit, move.angle > 1e-9 {
                            move = simd_quatd(angle: limit, axis: move.axis)
                        }
                        turn = move * turn
                    }
                } else {
                    slide += (to - from) * reach
                }
            } else if onGround {
                slide += move.travel
            }
        }
        // The surface point the turn brought under the focus is the new focus.
        if onGround {
            let focusWas = turn.inverse.act(up)
            moveFocus(toLocalDirection: current.rotation.inverse.act(focusWas))
        }
        // The surface moving east under the viewer puts the focus further west.
        let localSlide = current.rotation.inverse.act(slide) / scale
        let earth = MapPlacement.earthRadiusMeters
        if onGround {
            viewpoint.latitude -= localSlide.y / earth * 180 / .pi
            viewpoint.longitude -= localSlide.x / (earth * max(cos(viewpoint.latitude * .pi / 180), 0.1)) * 180 / .pi
        } else if simd_length(localSlide) > 1e-9,
                  let focus = GlobeDrag.focus(
                    grabbed: offGlobeDirection(localSlide / earth),
                    pulled: SIMD3<Double>(0, 0, 1), latitude: viewpoint.latitude,
                    longitude: viewpoint.longitude, roll: viewpoint.globeRoll) {
            viewpoint.latitude = focus.latitude
            viewpoint.longitude = focus.longitude
            viewpoint.globeRoll = focus.roll
        }
        if input.turn != 0 || input.pitch != 0 {
            rotate(bearing: input.turn, pitch: input.pitch, begins: input.beginsOrbit, anchor: input.orbitAnchor)
        }
        let heightBefore = viewpoint.height
        viewpoint.height = min(
            max(viewpoint.height / exp(input.logScale), MapPlacement.minHeight),
            MapPlacement.tableHeight)
        // A zoom keeps the point the eyes grabbed where it is: the focus moves toward it by
        // the share the height shrank, and away from it as the height grows.
        if let anchor = input.zoomAnchor, viewpoint.height != heightBefore,
           let grabbed = hit(origin: anchor.origin, direction: anchor.direction, center: center, radius: radius, reach: reach),
           let gazed = geographic(ofLocalDirection: current.rotation.inverse.act(simd_normalize(grabbed - center)))
        {
            let share = viewpoint.height / heightBefore
            var eastward = viewpoint.longitude - gazed.longitude
            eastward = (eastward + 540).truncatingRemainder(dividingBy: 360) - 180
            viewpoint.latitude = gazed.latitude + (viewpoint.latitude - gazed.latitude) * share
            viewpoint.longitude = gazed.longitude + eastward * share
        }
        let limit = onGround ? MapPlacement.latitudeLimit : 89.999999
        viewpoint.latitude = min(max(viewpoint.latitude, -limit), limit)
        viewpoint.longitude = (viewpoint.longitude + 540).truncatingRemainder(dividingBy: 360) - 180
        current = pose(for: viewpoint)
    }

    private func offGlobeDirection(_ travel: SIMD3<Double>) -> SIMD3<Double> {
        let tangent = SIMD3<Double>(-travel.x, -travel.y, 0)
        let angle = simd_length(tangent)
        guard angle > 1e-9 else { return SIMD3<Double>(0, 0, 1) }
        return tangent / angle * sin(angle) + SIMD3<Double>(0, 0, cos(angle))
    }

    private mutating func dragGlobe(
        _ move: MapGestureInput.Move, origin: SIMD3<Double>, from: SIMD3<Double>,
        to: SIMD3<Double>, center: SIMD3<Double>, radius: Double, reach: Double
    ) -> SIMD3<Double> {
        let grabbed = hit(origin: origin, direction: from, center: center, radius: radius, reach: reach)
        if move.beginsGesture || globeGrabbed == nil { globeGrabbed = grabbed != nil }
        if globeGrabbed == true, let grabbed,
           let pulled = hit(origin: origin, direction: to, center: center, radius: radius, reach: reach),
           let focus = GlobeDrag.focus(
               grabbed: current.rotation.inverse.act(simd_normalize(grabbed - center)),
               pulled: current.rotation.inverse.act(simd_normalize(pulled - center)),
               latitude: viewpoint.latitude, longitude: viewpoint.longitude,
               roll: viewpoint.globeRoll)
        {
            viewpoint.latitude = focus.latitude
            viewpoint.longitude = focus.longitude
            viewpoint.globeRoll = focus.roll
            current = pose(for: viewpoint)
            return .zero
        }
        // Off-sphere navigation keeps the table globe's angular speed at every size.
        return move.travel * (radius / MapPlacement.tableRadius)
    }

    /// Where a ray meets the rendered globe, unless it misses or the point is beyond reach.
    private func hit(
        origin: SIMD3<Double>, direction: SIMD3<Double>, center: SIMD3<Double>, radius: Double, reach: Double
    ) -> SIMD3<Double>? {
        let toCenter = origin - center
        let b = simd_dot(toCenter, direction)
        let c = simd_dot(toCenter, toCenter) - radius * radius
        let discriminant = b * b - c
        guard discriminant >= 0 else {
            return nil
        }
        let root = discriminant.squareRoot()
        // The nearer root, in the form that keeps its precision when the sphere is the Earth
        // and the origin is a few hundred metres above it.
        let distance = b < 0 ? c / (-b + root) : -b - root
        guard distance.isFinite, distance > 0, distance <= reach else {
            return nil
        }
        return origin + direction * distance
    }

    /// Moves the focus to the surface point whose direction from the globe's centre, in the
    /// current focus's east, north and up frame, is `direction`.
    private mutating func moveFocus(toLocalDirection direction: SIMD3<Double>) {
        guard let moved = geographic(ofLocalDirection: direction) else {
            return
        }
        // A drag that would carry the focus over the pole stops at the cap; clamping the
        // latitude after the fact would flip the longitude and spin the globe half a turn.
        guard abs(moved.latitude) <= MapPlacement.latitudeLimit else {
            return
        }
        viewpoint.latitude = moved.latitude
        viewpoint.longitude = moved.longitude
    }

    /// The surface point whose direction from the globe's centre, in the current focus's
    /// east, north and up frame, is `direction`.
    private func geographic(ofLocalDirection direction: SIMD3<Double>) -> (latitude: Double, longitude: Double)? {
        guard direction.x.isFinite, simd_length(direction) > 0.5 else {
            return nil
        }
        let latitude = viewpoint.latitude * .pi / 180
        let longitude = viewpoint.longitude * .pi / 180
        let east = SIMD3<Double>(-sin(longitude), cos(longitude), 0)
        let north = SIMD3<Double>(-sin(latitude) * cos(longitude), -sin(latitude) * sin(longitude), cos(latitude))
        let up = SIMD3<Double>(cos(latitude) * cos(longitude), cos(latitude) * sin(longitude), sin(latitude))
        let point = simd_normalize(east * direction.x + north * direction.y + up * direction.z)
        return (asin(max(-1, min(1, point.z))) * 180 / .pi, atan2(point.y, point.x) * 180 / .pi)
    }

    /// How far up the bridge from the ground to the table a height stands, 0 to 1.
    func bridgeShare(_ height: Double) -> Double {
        let low = log(MapPlacement.groundHeightLimit)
        let high = log(MapPlacement.tableHeight)
        return min(max((log(height) - low) / (high - low), 0), 1)
    }

    /// The scene's pose for a viewpoint: level under the viewer near the ground, the globe on
    /// the table at the top, and the flight between them by height in between.
    func pose(for viewpoint: Viewpoint) -> ScenePose {
        let ground = groundPose(
            height: min(viewpoint.height, MapPlacement.groundHeightLimit), bearing: viewpoint.bearing, tilt: viewpoint.tilt)
        var pose = viewpoint.height <= MapPlacement.groundHeightLimit ? ground :
            ScenePose.flight(from: ground, to: tablePose(), bridgeShare(viewpoint.height), viewer: viewerReference)
        pose.rotation = sceneRotation * pose.rotation
            * simd_quatd(angle: viewpoint.globeRoll, axis: SIMD3<Double>(0, 0, 1))
        pose.translation += sceneOffset
        return pose
    }

    /// The world at full size and level with the room, the focus `height` below the viewer,
    /// north away from them unless the world is turned.
    private func groundPose(height: Double, bearing: Double, tilt: Double) -> ScenePose {
        let east = SIMD3<Double>(1, 0, 0)
        let north = SIMD3<Double>(0, 0, -1)
        let up = SIMD3<Double>(0, 1, 0)
        let level = simd_quatd(simd_double3x3(east, north, up))
        let turn = simd_quatd(angle: bearing, axis: up)
        let pitch = simd_quatd(angle: tilt, axis: SIMD3<Double>(1, 0, 0))
        return ScenePose(rotation: pitch * turn * level,
                         translation: viewerReference + pitch.act(-up * height), logScale: 0)
    }

    /// A globe of the table's radius centred on the table, the focus facing the viewer with
    /// north upwards, so a zoom into it lands on the focus.
    private func tablePose() -> ScenePose {
        var facing = simd_normalize(viewerReference - tableCenter)
        if !facing.x.isFinite || simd_length(facing) < 0.5 {
            facing = SIMD3<Double>(0, 0, 1)
        }
        var north = SIMD3<Double>(0, 1, 0) - facing * facing.y
        if simd_length(north) < 1e-6 {
            north = SIMD3<Double>(0, 0, -1)
        }
        north = simd_normalize(north)
        let east = simd_cross(north, facing)
        let rotation = tableRotation ?? simd_quatd(simd_double3x3(east, north, facing))
        return ScenePose(
            rotation: rotation,
            translation: tableCenter + rotation.act(SIMD3<Double>(0, 0, MapPlacement.tableRadius)),
            logScale: log(MapPlacement.tableRadius / MapPlacement.earthRadiusMeters))
    }
}
