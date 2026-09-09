import XCTest
import simd
@testable import MapInteraction

final class MapControlLawTests: XCTestCase {
    func testZoomMagnificationMatchesHandRatioAtEveryScale() {
        for height in [4000.0, 60000, 100000, 1e6, 8e6, 4e7] {
            var map = placed(height)
            let radius = MapPlacement.earthRadiusMeters
            let angle = min(0.3, height / radius * 0.5)
            let local = SIMD3<Double>(radius * sin(angle), 0, radius * (cos(angle) - 1))
            let point = map.current.translation + map.current.rotation.act(local * exp(map.current.logScale))
            let ray = simd_normalize(point)
            let coordinate = map.geographicPosition(ofRoomPoint: point)
            let before = exp(map.current.logScale) / simd_length(point)
            map.apply(.init(logScale: 0.03, beginsZoom: true, focusAnchor: (.zero, ray)))
            let moved = map.roomPoint(for: coordinate)
            let after = exp(map.current.logScale) / simd_length(moved)
            XCTAssertEqual(after / before, exp(0.03), accuracy: 1e-7, "height \(height)")
        }
    }

    func testShallowTerrainGrabStaysUnderPointer() {
        for tilt in [0.0, 0.6] {
            var map = placed(4000)
            map.setTilt(tilt)
            let before = map.viewpoint
            let up = map.current.rotation.act(SIMD3<Double>(0, 0, 1))
            let north = map.current.rotation.act(SIMD3<Double>(0, 1, 0))
            let direction = simd_normalize(north - up * 0.22)
            let distance = simd_dot(map.current.translation, up) / simd_dot(direction, up)
            let picked = direction * distance
            let local = map.current.rotation.inverse.act(picked - map.current.translation)
            let geographic = mercator(before) + SIMD2<Double>(local.x, local.y) / cos(before.latitude * .pi / 180)
            let next = simd_normalize(direction + map.current.rotation.act(SIMD3<Double>(0.01, 0, 0)))
            map.apply(.init(moves: [.init(travel: .zero, beginsGesture: true,
                                         rayOrigin: .zero, rayFrom: direction, rayTo: next)]))
            let delta = (geographic - mercator(map.viewpoint)) * cos(map.viewpoint.latitude * .pi / 180)
            let moved = map.current.translation + map.current.rotation.act(SIMD3<Double>(delta.x, delta.y, 0))
            XCTAssertLessThan(simd_length(simd_cross(simd_normalize(moved), next)), 1e-6)
        }
    }

    func testOffCenterTwistDoesNotCarryTheGlobe() {
        var map = placed(4e7)
        let radius = MapPlacement.earthRadiusMeters * exp(map.current.logScale)
        let center = map.current.translation - map.current.rotation.act(SIMD3<Double>(0, 0, radius))
        let ray = simd_normalize(center + SIMD3<Double>(0.07, 0.04, 0.13))
        map.apply(.init(turn: 0.4, pitch: 0.2, beginsOrbit: true, orbitAnchor: (.zero, ray)))
        let moved = map.current.translation - map.current.rotation.act(SIMD3<Double>(0, 0, radius))
        XCTAssertLessThan(simd_length(moved - center), 1e-9)
    }

    private func placed(_ height: Double) -> MapPlacement {
        var map = MapPlacement(viewpoint: .above(.innsbruck, height: height))
        map.tableCenter = SIMD3<Double>(0, 0, -1)
        map.place(viewer: .zero)
        return map
    }

    private func mercator(_ point: Viewpoint) -> SIMD2<Double> {
        MapPlacement.earthRadiusMeters * SIMD2<Double>(point.longitude * .pi / 180,
            log(tan(.pi / 4 + point.latitude * .pi / 360)))
    }
}

extension MapControlLawTests {
    func testTiltKeepsOrbitFocusWhenLookingAtControls() {
        var map = placed(4000)
        map.setTilt(0.5)
        let point = map.current.translation + map.current.rotation.act(SIMD3<Double>(300, 200, 0))
        let local = map.current.rotation.inverse.act(point - map.current.translation)
        let ray = simd_normalize(point)
        map.updateViewRay(origin: .zero, direction: ray)
        map.apply(.init(turn: 0.2, pitch: 0.1, beginsOrbit: true, orbitAnchor: (.zero, ray)))
        map.updateViewRay(origin: .zero, direction: SIMD3<Double>(1, 0, 0))
        map.beginTilt()
        map.setTilt(0.3)
        map.endTilt()
        let moved = map.current.translation + map.current.rotation.act(local)
        XCTAssertLessThan(simd_length(moved - point), 0.02)
    }

    func testFirstTiltAfterFlightPreservesArrivalFocus() {
        var map = placed(4e7)
        map.updateViewRay(origin: .zero, direction: SIMD3<Double>(0, 0, -1))
        map.fly(to: 4000, at: 0, viewer: .zero)
        _ = map.advance(at: MapPlacement.transitionSeconds)
        let point = map.current.translation
        map.updateViewRay(origin: .zero, direction: simd_normalize(SIMD3<Double>(1, -1, 0)))
        map.beginTilt()
        map.setTilt(0.2)
        map.endTilt()
        XCTAssertLessThan(simd_length(map.current.translation - point), 0.02)
    }

    func testTerrainPickRejectsNonfiniteRefinement() {
        let point = MapTerrainRay.intersection(origin: .zero, direction: SIMD3<Double>(0, -1, 0), reach: 10) {
            let depth = -$0.y
            return depth > 0.96 && depth < 1 ? .nan : 1 - depth
        }
        XCTAssertNil(point)
    }

    func testTiltSessionKeepsTerrainAnchorThroughHeadMotionAndDemRefinement() {
        var map = placed(4000)
        let ray = simd_normalize(SIMD3<Double>(0.3, -1, -1))
        map.updateViewRay(origin: .zero, direction: ray)
        let point = ray * (3000 / -ray.y)
        let local = map.current.rotation.inverse.act(point - map.current.translation)
        map.beginTilt(elevation: { _ in 1000 })
        for angle in [0.1, 0.4, 0.8, 0.3, 0] {
            map.updateViewRay(origin: SIMD3<Double>(0.2, 0.1, 0), direction: SIMD3<Double>(1, 0, 0))
            map.setTilt(angle, elevation: { _ in 1100 })
            let result = map.current.translation + map.current.rotation.act(local)
            XCTAssertLessThan(simd_length(result - point), 0.02)
            XCTAssertEqual(map.sceneTilt, angle, accuracy: 1e-7)
        }
        map.endTilt()
    }

    func testTiltStopsBeforeTerrainWithoutDroppingPivot() {
        var map = placed(4000)
        map.setTilt(0.8)
        let up = map.current.rotation.act(SIMD3<Double>(0, 0, 1))
        let point = SIMD3<Double>(0, 100, (simd_dot(up, map.current.translation) - up.y * 100) / up.z)
        let local = map.current.rotation.inverse.act(point - map.current.translation)
        map.updateViewRay(origin: .zero, direction: simd_normalize(point))
        map.beginTilt(elevation: { _ in 0 })
        map.setTilt(0, elevation: { _ in 0 })
        XCTAssertTrue(map.tiltWasLimited)
        XCTAssertGreaterThan(map.sceneTilt, 0)
        XCTAssertLessThan(map.sceneTilt, 0.8)
        let result = map.current.translation + map.current.rotation.act(local)
        XCTAssertLessThan(simd_length(result - point), 0.02)
        let eye = map.current.rotation.inverse.act(-map.current.translation)
        XCTAssertGreaterThanOrEqual(eye.z, MapPlacement.minHeight - 0.02)
    }

    func testFirstPlacementCentersGlobeAtPitchedAndRolledView() {
        let origin = SIMD3<Double>(3, 1.7, -2)
        for direction in [SIMD3<Double>(1, -0.5, -1), SIMD3<Double>(0, 1, 0), SIMD3<Double>(0, -1, 0)] {
            var map = placed(4e7)
            map.placeInView(origin: origin, direction: direction)
            XCTAssertLessThan(simd_length(map.tableCenter - origin - simd_normalize(direction)), 1e-9)
            let center = map.tableCenter
            map.updateViewRay(origin: origin + SIMD3<Double>(0.1, 0, 0), direction: SIMD3<Double>(1, 0, 0))
            _ = map.advance(at: 1)
            XCTAssertEqual(map.tableCenter, center)
        }
    }

    func testPanPreservesGeographicGrabAcrossManyFramesAtHighLatitude() {
        var map = MapPlacement(viewpoint: .above(.init(latitude: 70, longitude: 179.99, altitudeMeters: 0), height: 60000))
        map.place(viewer: .zero)
        map.setTilt(0.7)
        let start = map.viewpoint
        let point = map.current.translation
        let ray = simd_normalize(point)
        let right = map.current.rotation.act(SIMD3<Double>(1, 0, 0))
        let north = map.current.rotation.act(SIMD3<Double>(0, 1, 0))
        var previous = ray
        for step in 1...100 {
            let next = simd_normalize(ray + (right * 0.001 + north * 0.002) * Double(step))
            map.apply(.init(moves: [.init(travel: .zero, beginsGesture: step == 1,
                rayOrigin: .zero, rayFrom: previous, rayTo: next)]))
            var difference = mercator(start) - mercator(map.viewpoint)
            let circumference = 2 * .pi * MapPlacement.earthRadiusMeters
            difference.x -= (difference.x / circumference).rounded() * circumference
            difference *= cos(map.viewpoint.latitude * .pi / 180)
            let actual = map.current.translation + map.current.rotation.act(SIMD3<Double>(difference.x, difference.y, 0))
            XCTAssertLessThan(simd_length(simd_cross(simd_normalize(actual), next)), 1e-8)
            previous = next
        }
    }

    func testInputBufferPreservesGestureOrderAndCancelsOverflow() {
        var buffer = MapGestureInput.Buffer()
        XCTAssertTrue(buffer.append(.init(logScale: 0.1, beginsZoom: true)))
        XCTAssertTrue(buffer.append(.init(turn: 0.2, beginsOrbit: true)))
        XCTAssertTrue(buffer.append(.init(logScale: -0.1, beginsZoom: true)))
        let actions = buffer.take()
        XCTAssertEqual(actions.map(\.logScale), [0.1, 0, -0.1])
        XCTAssertEqual(actions.map(\.turn), [0, 0.2, 0])
        XCTAssertTrue(buffer.take().isEmpty)
        for _ in 0..<64 { XCTAssertTrue(buffer.append(.init(logScale: 0.01))) }
        XCTAssertFalse(buffer.append(.init(logScale: 0.01)))
        XCTAssertTrue(buffer.take().isEmpty)
    }
}

extension MapControlLawTests {
    func testZoomKeepsGeographicPointAcrossGlobeToTerrainAndBack() {
        var map = placed(4e7)
        let r = MapPlacement.earthRadiusMeters
        let local = SIMD3<Double>(r * sin(0.4), 0, r * (cos(0.4) - 1))
        let point = map.current.translation + map.current.rotation.act(local * exp(map.current.logScale))
        let coordinate = map.geographicPosition(ofRoomPoint: point)
        let direction = simd_normalize(point)
        let orientation = simd_double3x3(map.current.rotation)
            * GlobeDrag.geographicBasis(latitude: map.viewpoint.latitude, longitude: map.viewpoint.longitude).transpose
        for step in 0..<240 {
            map.apply(.init(logScale: step < 120 ? 0.08 : -0.08, beginsZoom: step == 0,
                            focusAnchor: (.zero, direction)))
            let result = map.roomPoint(for: coordinate)
            XCTAssertLessThan(simd_length(simd_cross(simd_normalize(result), direction)), 1e-7)
            let nextOrientation = simd_double3x3(map.current.rotation)
                * GlobeDrag.geographicBasis(latitude: map.viewpoint.latitude, longitude: map.viewpoint.longitude).transpose
            XCTAssertLessThan(simd_length(nextOrientation.columns.0 - orientation.columns.0), 1e-7)
        }
        XCTAssertEqual(map.viewpoint.height, 4e7, accuracy: 1e-4)
    }

    func testGlobeDragAfterOrbitTracksThePickedGeographicPoint() {
        var map = placed(4e7)
        map.apply(.init(turn: 0.4, pitch: 0.2, beginsOrbit: true))
        let r = MapPlacement.earthRadiusMeters
        let local = SIMD3<Double>(r * sin(0.1), 0, r * (cos(0.1) - 1))
        let point = map.current.translation + map.current.rotation.act(local * exp(map.current.logScale))
        let coordinate = map.geographicPosition(ofRoomPoint: point)
        let start = simd_normalize(point)
        var previous = start
        for step in 1...25 {
            let next = simd_normalize(start + SIMD3<Double>(0.001, -0.0002, 0) * Double(step))
            map.apply(.init(moves: [.init(travel: .zero, beginsGesture: step == 1,
                rayOrigin: .zero, rayFrom: previous, rayTo: next)]))
            let result = map.roomPoint(for: coordinate)
            XCTAssertLessThan(simd_length(simd_cross(simd_normalize(result), next)), 1e-7)
            previous = next
        }
    }
}
