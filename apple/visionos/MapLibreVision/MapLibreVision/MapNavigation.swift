import simd

extension MapPlacement {
    /// Applies hand input to the viewpoint; input during a flight is dropped.
    mutating func apply(_ input: MapGestureInput.Delta, elevation: ((MapAnchor) -> Double?)? = nil) {
        guard flight == nil else {
            return
        }
        let onGround = viewpoint.height <= MapPlacement.groundHeightLimit
        if tiltIsEditing { return }
        if !input.moves.isEmpty { retainedFocus = nil }
        if !input.moves.isEmpty || input.logScale != 0 { orbitTarget = nil }
        if !input.moves.isEmpty || input.turn != 0 || input.pitch != 0 || input.translation != .zero {
            zoomTarget = nil
        }
        if !onGround {
            var travel = carryTravel(input)
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
        var slide = SIMD3<Double>(repeating: 0)
        for move in input.moves {
            if let origin = move.rayOrigin, let from = move.rayFrom, let to = move.rayTo {
                if !onGround {
                    slide += dragGlobe(move, origin: origin, from: from,
                                       to: to, center: center, radius: radius, reach: reach)
                    continue
                }
                globeGrabbed = nil
                dragGround(move, origin: origin, from: from, to: to, elevation: elevation)
            } else if onGround {
                slide += move.travel
            }
        }
        // The surface moving east under the viewer puts the focus further west.
        let localSlide = current.rotation.inverse.act(slide) / scale
        let earth = MapPlacement.earthRadiusMeters
        if onGround {
            // Local scene metres use the focus's Mercator scale in both directions.
            let latitude = viewpoint.latitude * .pi / 180
            let meters = earth * max(cos(latitude), 1e-6)
            let north = log(tan(.pi / 4 + latitude / 2)) - localSlide.y / meters
            viewpoint.latitude = (2 * atan(exp(north)) - .pi / 2) * 180 / .pi
            viewpoint.longitude -= localSlide.x / meters * 180 / .pi
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
            if input.beginsOrbit { captureOrbitTarget(anchor: input.orbitAnchor, elevation: elevation) }
            rotate(bearing: input.turn, pitch: input.pitch, begins: false,
                   anchor: input.orbitAnchor, elevation: elevation)
        }
        applyZoom(input, elevation: elevation)
        let limit = onGround ? MapPlacement.latitudeLimit : 89.999999
        viewpoint.latitude = min(max(viewpoint.latitude, -limit), limit)
        viewpoint.longitude = (viewpoint.longitude + 540).truncatingRemainder(dividingBy: 360) - 180
        current = pose(for: viewpoint)
    }

    mutating func applyZoom(_ input: MapGestureInput.Delta, elevation: ((MapAnchor) -> Double?)?) {
        guard input.logScale.isFinite, input.logScale != 0 else { return }
        if input.beginsZoom || zoomTarget == nil { captureZoomTarget(input, elevation: elevation) }
        let height = viewpoint.height
        let oldScale = exp(current.logScale)
        let orientation = current.rotation
        viewpoint.height = min(max(height / exp(input.logScale), MapPlacement.minHeight), MapPlacement.tableHeight)
        current = pose(for: viewpoint)
        // Zoom changes distance and scale; rotating the surface underneath a held
        // point can send the eye through it. Mode flights own orientation changes.
        sceneRotation = simd_normalize(orientation * current.rotation.inverse * sceneRotation)
        current = pose(for: viewpoint)
        guard var target = zoomTarget else { return }
        let nextScale = exp(current.logScale)
        // Projected scale is scene scale / anchor distance, in either representation.
        // The hand ratio therefore has the same visual gain across the entire zoom range.
        target.distance *= nextScale / oldScale * viewpoint.height / height
        let wanted = target.origin + target.direction * target.distance
        let actual = current.translation + current.rotation.act(target.local * nextScale)
        sceneOffset += wanted - actual
        current = pose(for: viewpoint)
        zoomTarget = target
        retainedFocus = (target.local, wanted)
    }

    mutating func dragGround(_ move: MapGestureInput.Move, origin: SIMD3<Double>,
                                     from: SIMD3<Double>, to: SIMD3<Double>, elevation: ((MapAnchor) -> Double?)?) {
        let scale = exp(current.logScale)
        let earth = Self.earthRadiusMeters
        if move.beginsGesture || groundDrag == nil {
            let surface = terrainHit(origin: origin, direction: from, elevation: elevation) ?? current.translation
            let plane = MapDragPlane(origin: origin, ray: from, surface: surface,
                                     normal: current.rotation.act(SIMD3<Double>(0, 0, 1)))
            groundDrag = plane
            let local = current.rotation.inverse.act(plane.referencePoint - current.translation) / scale
            let lat = viewpoint.latitude * .pi / 180
            groundCoordinate = SIMD2<Double>(viewpoint.longitude * .pi / 180,
                log(tan(.pi / 4 + lat / 2))) + SIMD2<Double>(local.x, local.y) / (earth * cos(lat))
        }
        guard let coordinate = groundCoordinate, let target = groundDrag?.target(ray: to) else { return }
        let local = current.rotation.inverse.act(target - current.translation) / scale
        // Changing the Mercator origin changes its local metre scale. Solve for the
        // captured geographic point instead of integrating latitude-dependent deltas.
        var latitude = viewpoint.latitude * .pi / 180
        let limit = Self.latitudeLimit * .pi / 180
        for _ in 0..<8 {
            let secant = 1 / cos(latitude)
            let error = log(tan(.pi / 4 + latitude / 2)) + local.y / earth * secant - coordinate.y
            let derivative = secant * (1 + local.y / earth * tan(latitude))
            guard derivative.isFinite, abs(derivative) > 1e-8 else { return }
            latitude = min(max(latitude - error / derivative, -limit), limit)
        }
        viewpoint.latitude = latitude * 180 / .pi
        viewpoint.longitude = (coordinate.x - local.x / (earth * cos(latitude))) * 180 / .pi
        current = pose(for: viewpoint)
        retainedFocus = (local, target)
    }

    mutating func captureZoomTarget(_ input: MapGestureInput.Delta, elevation: ((MapAnchor) -> Double?)?) {
        let scale = exp(current.logScale)
        let radius = MapPlacement.earthRadiusMeters * scale
        let center = current.translation - current.rotation.act(SIMD3<Double>(0, 0, radius))
        let reach = MapPlacement.maxGrabDistanceInHeights * viewpoint.height * scale
        for ray in [input.focusAnchor, input.zoomAnchor, viewRay].compactMap({ $0 }) {
            guard let point = terrainHit(origin: ray.origin, direction: ray.direction, elevation: elevation)
                ?? hit(origin: ray.origin, direction: ray.direction,
                                  center: center, radius: radius, reach: reach) else { continue }
            rebaseGlobe(at: point)
            zoomTarget = ZoomTarget(local: current.rotation.inverse.act(point - current.translation) / scale,
                                    origin: ray.origin, direction: ray.direction,
                                    distance: simd_length(point - ray.origin))
            return
        }
        let origin = viewRay?.origin ?? viewerReference
        let delta = current.translation - origin
        guard simd_length(delta) > 1e-6 else { return }
        zoomTarget = ZoomTarget(local: .zero, origin: origin, direction: simd_normalize(delta),
                                distance: simd_length(delta))
    }

    func geographicPosition(ofRoomPoint point: SIMD3<Double>) -> MapAnchor {
        cameraPosition(local: current.rotation.inverse.act(point - current.translation) / exp(current.logScale),
                       globe: viewpoint.height > Self.groundHeightLimit)
    }

    func roomPoint(for coordinate: MapAnchor) -> SIMD3<Double> {
        let earth = Self.earthRadiusMeters
        let height = coordinate.altitudeMeters - focusElevation
        let local: SIMD3<Double>
        if viewpoint.height > Self.groundHeightLimit {
            let basis = GlobeDrag.geographicBasis(latitude: viewpoint.latitude, longitude: viewpoint.longitude)
            let direction = GlobeDrag.geographicBasis(latitude: coordinate.latitude, longitude: coordinate.longitude).columns.2
            local = basis.transpose * direction * (earth + height) - SIMD3<Double>(0, 0, earth)
        } else {
            let latitude = viewpoint.latitude * .pi / 180
            let meters = earth * cos(latitude)
            let longitude = (coordinate.longitude - viewpoint.longitude) * .pi / 180
            local = SIMD3<Double>(atan2(sin(longitude), cos(longitude)) * meters,
                (log(tan(.pi / 4 + coordinate.latitude * .pi / 360)) - log(tan(.pi / 4 + latitude / 2))) * meters, height)
        }
        return current.translation + current.rotation.act(local * exp(current.logScale))
    }

    mutating func rebaseGlobe(at point: SIMD3<Double>) {
        guard viewpoint.height > Self.groundHeightLimit else { return }
        let coordinate = geographicPosition(ofRoomPoint: point)
        let oldBasis = GlobeDrag.geographicBasis(latitude: viewpoint.latitude, longitude: viewpoint.longitude)
        let newBasis = GlobeDrag.geographicBasis(latitude: coordinate.latitude, longitude: coordinate.longitude)
        let rotation = simd_normalize(current.rotation * simd_quatd(oldBasis.transpose * newBasis))
        let radius = Self.earthRadiusMeters * exp(current.logScale)
        let center = current.translation - current.rotation.act(SIMD3<Double>(0, 0, radius))
        viewpoint.latitude = coordinate.latitude
        viewpoint.longitude = coordinate.longitude
        viewpoint.globeRoll = 0
        current = pose(for: viewpoint)
        // Geographic coordinates survive the globe-to-plane transition. Rebasing the
        // tangent frame at the zoom anchor preserves the globe's composed world pose.
        sceneRotation = simd_normalize(rotation * current.rotation.inverse * sceneRotation)
        current = pose(for: viewpoint)
        sceneOffset += center + rotation.act(SIMD3<Double>(0, 0, radius)) - current.translation
        current = pose(for: viewpoint)
    }

    func terrainHit(origin: SIMD3<Double>, direction: SIMD3<Double>,
                            elevation: ((MapAnchor) -> Double?)?) -> SIMD3<Double>? {
        guard let elevation else { return nil }
        let scale = exp(current.logScale)
        let globe = viewpoint.height > Self.groundHeightLimit
        return MapTerrainRay.intersection(origin: origin, direction: direction,
            reach: Self.maxGrabDistanceInHeights * max(viewpoint.height, 1) * scale) { point in
                let local = current.rotation.inverse.act(point - current.translation) / scale
                let position = cameraPosition(local: local, globe: globe)
                return position.altitudeMeters - (elevation(position) ?? focusElevation)
            }
    }

    mutating func carryTravel(_ input: MapGestureInput.Delta) -> SIMD3<Double> {
        if let reference = input.carryReference {
            let radius = MapPlacement.earthRadiusMeters * exp(current.logScale)
            let center = current.translation - current.rotation.act(SIMD3<Double>(0, 0, radius))
            let offset = center - reference.origin
            let depth = simd_length(offset)
            if depth > 1e-6, reference.handDepth.isFinite {
                carryMapping = (offset / depth, min(max(depth / max(reference.handDepth, 0.6), 1), 4))
            }
        }
        guard let mapping = carryMapping else { return input.translation }
        // Carry at a distance follows the same virtual pointer as a surface drag. Keep
        // push/pull at room scale and latch the gain so carrying away cannot accelerate it.
        let along = mapping.axis * simd_dot(input.translation, mapping.axis)
        return along + (input.translation - along) * mapping.gain
    }

    func offGlobeDirection(_ travel: SIMD3<Double>) -> SIMD3<Double> {
        let tangent = SIMD3<Double>(-travel.x, -travel.y, 0)
        let angle = simd_length(tangent)
        guard angle > 1e-9 else { return SIMD3<Double>(0, 0, 1) }
        return tangent / angle * sin(angle) + SIMD3<Double>(0, 0, cos(angle))
    }

    mutating func dragGlobe(
        _ move: MapGestureInput.Move, origin: SIMD3<Double>, from: SIMD3<Double>,
        to: SIMD3<Double>, center: SIMD3<Double>, radius: Double, reach: Double
    ) -> SIMD3<Double> {
        groundDrag = nil
        if move.beginsGesture || globeGrabbed == nil {
            globeGrabbed = hit(origin: origin, direction: from, center: center,
                               radius: radius, reach: reach) != nil
        }
        guard globeGrabbed == true else {
            // Empty-space drags use the focus's visual depth, like a virtual trackball.
            return (to - from) * simd_length(current.translation - origin)
        }
        guard let grabbed = hit(origin: origin, direction: from, center: center, radius: radius, reach: reach),
              let pulled = hit(origin: origin, direction: to, center: center, radius: radius, reach: reach),
              let focus = GlobeDrag.focus(
                grabbed: current.rotation.inverse.act(simd_normalize(grabbed - center)),
                pulled: current.rotation.inverse.act(simd_normalize(pulled - center)),
                latitude: viewpoint.latitude, longitude: viewpoint.longitude, roll: viewpoint.globeRoll)
        else { return .zero }
        viewpoint.latitude = focus.latitude
        viewpoint.longitude = focus.longitude
        viewpoint.globeRoll = focus.roll
        current = pose(for: viewpoint)
        return .zero
    }

    /// Where a ray meets the rendered globe, unless it misses or the point is beyond reach.
    func hit(
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
    mutating func moveFocus(toLocalDirection direction: SIMD3<Double>) {
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
    func geographic(ofLocalDirection direction: SIMD3<Double>) -> (latitude: Double, longitude: Double)? {
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

}
