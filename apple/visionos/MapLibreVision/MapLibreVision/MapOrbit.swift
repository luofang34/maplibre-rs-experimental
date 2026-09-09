import simd

extension MapPlacement {
    /// Artificial scene tilt relative to room gravity; head tracking is independent.
    var sceneTilt: Double {
        let up = current.rotation.act(SIMD3<Double>(0, 0, 1))
        return acos(min(max(up.y, -1), 1))
    }

    mutating func setTilt(_ radians: Double, elevation: ((MapAnchor) -> Double?)? = nil) {
        guard !inFlight, radians.isFinite else { return }
        if !tiltIsEditing { captureOrbitTarget(anchor: nil, elevation: elevation) }
        tiltWasLimited = false
        guard let elevation, cameraPolicy == .freeOrbit else { applyAbsoluteTilt(radians); return }
        let initial = self
        let start = sceneTilt
        let finish = min(max(radians, 0), .pi / 2)
        var accepted = start
        for step in 1...12 {
            let angle = start + (finish - start) * Double(step) / 12
            var candidate = initial
            candidate.applyAbsoluteTilt(angle)
            if candidate.hasClearance(elevation: elevation) {
                self = candidate
                accepted = angle
                continue
            }
            var rejected = angle
            for _ in 0..<16 {
                let middle = (accepted + rejected) / 2
                candidate = initial
                candidate.applyAbsoluteTilt(middle)
                if candidate.hasClearance(elevation: elevation) { self = candidate; accepted = middle }
                else { rejected = middle }
            }
            tiltWasLimited = true
            return
        }
    }

    func hasClearance(elevation: (MapAnchor) -> Double?) -> Bool {
        let scale = exp(current.logScale)
        let eyes = physicalEyes.isEmpty ? [viewRay?.origin ?? viewerReference] : physicalEyes
        return eyes.allSatisfy { eye in
            let local = current.rotation.inverse.act(eye - current.translation) / scale
            let position = cameraPosition(local: local, globe: viewpoint.height > Self.groundHeightLimit)
            return position.altitudeMeters >= (elevation(position) ?? focusElevation) + max(Self.minHeight, 0.05 / scale)
        }
    }

    mutating func beginTilt(elevation: ((MapAnchor) -> Double?)? = nil) {
        guard !inFlight else { return }
        if cameraPolicy == .freeOrbit, let focus = retainedFocus {
            let origin = viewRay?.origin ?? viewerReference
            let direction = focus.world - origin
            let ray = simd_length(direction) > 1e-6 ? (origin, simd_normalize(direction)) : viewRay
            captureOrbitTarget(anchor: ray, elevation: elevation)
            orbitTarget = focus
        } else {
            captureOrbitTarget(anchor: nil, elevation: elevation)
        }
        tiltIsEditing = true
    }

    mutating func endTilt() {
        retainedFocus = orbitTarget
        tiltIsEditing = false
    }

    mutating func applyAbsoluteTilt(_ radians: Double) {
        let requested = min(max(radians, 0), .pi / 2)
        let up = current.rotation.act(SIMD3<Double>(0, 0, 1))
        var slope = SIMD3<Double>(up.x, 0, up.z)
        if simd_length(slope) < 1e-6 {
            slope = simd_cross(orbitRight, SIMD3<Double>(0, 1, 0))
            slope.y = 0
        }
        if simd_length(slope) < 1e-6 { slope = SIMD3<Double>(0, 0, 1) }
        let wanted = SIMD3<Double>(0, cos(requested), 0) + simd_normalize(slope) * sin(requested)
        let rotation = simd_normalize(simd_quatd(from: simd_normalize(up), to: simd_normalize(wanted)) * current.rotation)
        viewpoint.tilt = requested
        setOrbitRotation(rotation)
    }

    mutating func setOrbitRotation(_ rotation: simd_quatd) {
        let posed = pose(for: viewpoint)
        sceneRotation = simd_normalize(rotation * posed.rotation.inverse * sceneRotation)
        current = pose(for: viewpoint)
        if let target = orbitTarget {
            let moved = current.translation + current.rotation.act(target.local * exp(current.logScale))
            sceneOffset += target.world - moved
            current = pose(for: viewpoint)
        }
    }

    mutating func rotate(
        bearing: Double, pitch: Double, begins: Bool,
        anchor: (origin: SIMD3<Double>, direction: SIMD3<Double>)? = nil,
        elevation: ((MapAnchor) -> Double?)? = nil
    ) {
        if orbitTarget == nil || begins { captureOrbitTarget(anchor: anchor, elevation: elevation) }
        if !isTableObject {
            let up = current.rotation.act(SIMD3<Double>(0, 0, 1))
            viewpoint.bearing += bearing
            setOrbitRotation(simd_quatd(angle: bearing, axis: up) * current.rotation)
            if pitch != 0 {
                tiltIsEditing = true
                setTilt(sceneTilt + pitch, elevation: elevation)
                tiltIsEditing = false
            }
            if cameraPolicy == .freeOrbit { retainedFocus = orbitTarget }
        } else {
            viewpoint.bearing += bearing
            let yaw = simd_quatd(angle: bearing, axis: orbitUp)
            let pitchTurn = simd_quatd(angle: pitch, axis: orbitRight)
            setOrbitRotation(pitchTurn * yaw * current.rotation)
        }
    }

    mutating func captureOrbitTarget(
        anchor: (origin: SIMD3<Double>, direction: SIMD3<Double>)?,
        elevation: ((MapAnchor) -> Double?)? = nil
    ) {
        let scale = exp(current.logScale)
        let radius = MapPlacement.earthRadiusMeters * scale
        let up = current.rotation.act(SIMD3<Double>(0, 0, 1))
        let center = current.translation - up * radius
        let ray = anchor ?? viewRay
        let surface = ray.flatMap {
            terrainHit(origin: $0.origin, direction: $0.direction, elevation: elevation)
                ?? hit(origin: $0.origin, direction: $0.direction, center: center,
                radius: radius, reach: MapPlacement.maxGrabDistanceInHeights * viewpoint.height * scale)
        } ?? current.translation
        orbitUp = simd_normalize(surface - center)
        let right = simd_cross(ray?.direction ?? SIMD3<Double>(0, 0, -1), orbitUp)
        orbitRight = simd_length(right) > 1e-6 ? simd_normalize(right) : current.rotation.act(SIMD3<Double>(1, 0, 0))
        let globe = isTableObject
        let target = cameraPolicy == .fixedViewpoint ? (viewRay?.origin ?? viewerReference) : globe ? center : surface
        if globe { orbitUp = simd_normalize((ray?.origin ?? viewerReference) - center) }
        orbitTarget = (current.rotation.inverse.act(target - current.translation) / scale, target)
    }

    mutating func levelView(elevation: ((MapAnchor) -> Double?)? = nil) {
        if viewpoint.height <= MapPlacement.groundHeightLimit {
            beginTilt(elevation: elevation)
            setTilt(0, elevation: elevation)
            endTilt()
            return
        }
        viewpoint.tilt = 0
        viewpoint.bearing = 0
        viewpoint.globeRoll = 0
        orbitTarget = nil
        sceneOffset = .zero
        sceneRotation = simd_quatd(angle: 0, axis: SIMD3<Double>(0, 1, 0))
        current = pose(for: viewpoint)
    }

}
