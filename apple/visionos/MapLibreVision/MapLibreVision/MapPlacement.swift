import simd

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
    /// Latitudes beyond this fold the Mercator plane; the focus stays inside.
    static let latitudeLimit = 84.0

    struct Flight {
        let from: Viewpoint
        let to: Viewpoint
        let fromPose: ScenePose
        let toPose: ScenePose
        let start: Double
        var clearanceOffset = SIMD3<Double>.zero
    }

    var viewpoint: Viewpoint
    var current: ScenePose
    var immersion: MapImmersion
    var flight: Flight?
    /// Whether this pinch began on the rendered sphere; crossing its edge keeps the mode.
    var globeGrabbed: Bool?
    var groundDrag: MapDragPlane?
    var groundCoordinate: SIMD2<Double>?
    var carryMapping: (axis: SIMD3<Double>, gain: Double)?
    /// Where the viewer stood when the scene was last placed about them; the world keeps to
    /// that place while they move about the room.
    var viewerReference = SIMD3<Double>(0, 0, 0)
    let cameraPolicy: MapCameraPolicy
    var viewRay: (origin: SIMD3<Double>, direction: SIMD3<Double>)?
    struct ZoomTarget {
        var local: SIMD3<Double>
        var origin: SIMD3<Double>
        var direction: SIMD3<Double>
        var distance: Double
    }
    var zoomTarget: ZoomTarget?
    var orbitTarget: (local: SIMD3<Double>, world: SIMD3<Double>)?
    var tiltIsEditing = false
    var retainedFocus: (local: SIMD3<Double>, world: SIMD3<Double>)?
    var physicalEyes: [SIMD3<Double>] = []
    var tiltWasLimited = false
    var sceneOffset = SIMD3<Double>.zero
    var sceneRotation = simd_quatd(angle: 0, axis: SIMD3<Double>(0, 1, 0))
    var tableRotation: simd_quatd?
    var immersiveTilt = Double.pi / 4
    var orbitRight = SIMD3<Double>(1, 0, 0)
    var orbitUp = SIMD3<Double>(0, 1, 0)
    /// Terrain elevation at the focus, metres, as the map last reported it.
    var focusElevation = 0.0

    /// Captured at the first tracked view so later head motion cannot carry the globe.
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

    var isTableObject: Bool {
        guard viewpoint.height > Self.groundHeightLimit else { return false }
        let radius = Self.earthRadiusMeters * exp(current.logScale)
        let center = current.translation - current.rotation.act(SIMD3<Double>(0, 0, radius))
        return radius <= 1.5 && radius < simd_length(center - (viewRay?.origin ?? viewerReference)) * 0.5
    }

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

    mutating func placeInView(origin: SIMD3<Double>, direction: SIMD3<Double>) {
        guard direction.x.isFinite, direction.y.isFinite, direction.z.isFinite,
              simd_length(direction) > 1e-6 else { return }
        tableCenter = origin + simd_normalize(direction)
        place(viewer: origin)
    }

    mutating func updateViewRay(origin: SIMD3<Double>, direction: SIMD3<Double>) {
        guard simd_length(direction) > 0.5 else { return }
        viewRay = (origin, simd_normalize(direction))
    }

    mutating func updateEyes(_ eyes: [SIMD3<Double>]) { physicalEyes = eyes }

    /// Keeps every physical eye above the loaded terrain, including an orbit's displaced eye.
    mutating func constrainCamera(eyes: [SIMD3<Double>], elevation: (MapAnchor) -> Double?) {
        guard cameraPolicy == .freeOrbit else { return }
        let scale = exp(current.logScale)
        let globe = viewpoint.height > MapPlacement.groundHeightLimit
        for eye in eyes {
            let local = current.rotation.inverse.act(eye - current.translation) / scale
            let position = cameraPosition(local: local, globe: globe)
            let ground = elevation(position) ?? focusElevation
            let margin = max(MapPlacement.minHeight, 0.05 / scale)
            let deficit = ground + margin - position.altitudeMeters
            guard deficit.isFinite, deficit > 0 else { continue }
            let normal = globe
                ? simd_normalize(local + SIMD3<Double>(0, 0, MapPlacement.earthRadiusMeters))
                : SIMD3<Double>(0, 0, 1)
            let correction = current.rotation.act(normal * deficit * scale)
            sceneOffset -= correction
            flight?.clearanceOffset -= correction
            current.translation -= correction
            // A constrained orbit must acquire a reachable target for subsequent input.
            orbitTarget = nil
            zoomTarget = nil
            retainedFocus = nil
        }
    }

    func cameraPosition(local: SIMD3<Double>, globe: Bool) -> MapAnchor {
        let earth = MapPlacement.earthRadiusMeters
        if globe {
            let radial = local + SIMD3<Double>(0, 0, earth)
            let location = geographic(ofLocalDirection: simd_normalize(radial))
            return MapAnchor(latitude: location?.latitude ?? viewpoint.latitude,
                             longitude: location?.longitude ?? viewpoint.longitude,
                             altitudeMeters: focusElevation + simd_length(radial) - earth)
        }
        let latitude = viewpoint.latitude * .pi / 180
        let meters = earth * max(cos(latitude), 1e-6)
        let north = log(tan(.pi / 4 + latitude / 2)) + local.y / meters
        return MapAnchor(latitude: (2 * atan(exp(north)) - .pi / 2) * 180 / .pi,
                         longitude: viewpoint.longitude + local.x / meters * 180 / .pi,
                         altitudeMeters: focusElevation + local.z)
    }

    /// Starts a flight to `height` from wherever the viewpoint is.
    mutating func fly(to height: Double, at time: Double, viewer: SIMD3<Double>) {
        let from = viewpoint
        let fromPose = current
        if viewpoint.height <= MapPlacement.groundHeightLimit { immersiveTilt = sceneTilt }
        viewerReference = viewer
        orbitTarget = nil
        tiltIsEditing = false
        retainedFocus = nil
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

    func destination() -> ScenePose? {
        guard let flight else { return nil }
        var pose = flight.toPose
        pose.translation += flight.clearanceOffset
        return pose
    }

    mutating func advance(at time: Double) -> MapImmersion? {
        if let flight {
            let t = min(max((time - flight.start) / MapPlacement.transitionSeconds, 0), 1)
            let eased = t * t * (3 - 2 * t)
            viewpoint.height = exp(log(flight.from.height) + (log(flight.to.height) - log(flight.from.height)) * eased)
            viewpoint.tilt = flight.from.tilt + (flight.to.tilt - flight.from.tilt) * eased
            viewpoint.bearing = flight.from.bearing + (flight.to.bearing - flight.from.bearing) * eased
            viewpoint.globeRoll = flight.from.globeRoll * (1 - eased)
            current = ScenePose.flight(from: flight.fromPose, to: flight.toPose, eased, viewer: viewerReference)
            current.translation += flight.clearanceOffset
            if t >= 1 {
                viewpoint = flight.to
                self.flight = nil
                current = pose(for: viewpoint)
                retainedFocus = (.zero, current.translation)
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
        pose.rotation = simd_normalize(sceneRotation * pose.rotation
            * simd_quatd(angle: viewpoint.globeRoll, axis: SIMD3<Double>(0, 0, 1)))
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
