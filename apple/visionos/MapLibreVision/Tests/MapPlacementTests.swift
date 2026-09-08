import XCTest
import simd
@testable import MapInteraction

final class MapPlacementTests: XCTestCase {
    private let heights = [40_000_000.0, 20_000_000.0, 8_000_000.0, 1_000_000.0]

    func testGrabbedSurfaceTracksHandDisplacementAtEveryGlobeSize() throws {
        for height in heights {
            for offset in [SIMD2<Double>(0, 0), SIMD2<Double>(0.15, 0.1)] {
                var placement = placed(height)
                let (center, radius) = sphere(placement)
                let local = SIMD3<Double>(offset.x, offset.y, sqrt(1 - simd_length_squared(offset)))
                let grabbed = center + placement.current.rotation.act(local) * radius
                let from = simd_normalize(grabbed)
                let point = geographicDirection(local, placement.viewpoint)
                var aim = grabbed
                for step in 0..<12 {
                    let travel = SIMD3<Double>(0.002, 0.001, 0)
                    aim += travel
                    // An indirect pinch can originate close to the body; its synthetic
                    // angular ray must not amplify the actual hand displacement.
                    let indirect = simd_normalize(from + travel * 5)
                    placement.apply(.init(moves: [.init(
                        travel: travel, beginsGesture: step == 0,
                        rayOrigin: .zero, rayFrom: from, rayTo: indirect)]))
                    let moved = center + placement.current.rotation.act(localDirection(point, placement.viewpoint)) * radius
                    XCTAssertLessThan(simd_length(simd_cross(simd_normalize(moved), simd_normalize(aim))), 1e-7,
                                      "grabbed point drifts from hand displacement at height \(height)")
                }
                XCTAssertEqual(placement.viewpoint.bearing, 0)
            }
        }
    }

    func testGrabbedPointDoesNotAccelerateWhenHandLeavesGlobe() {
        var placement = placed(MapPlacement.tableHeight)
        let before = placement.viewpoint
        let from = simd_normalize(placement.current.translation)
        for (index, travel) in [SIMD3<Double>(1, 0, 0), SIMD3<Double>(-1, 0, 0)].enumerated() {
            placement.apply(.init(moves: [.init(travel: travel, beginsGesture: index == 0,
                rayOrigin: .zero, rayFrom: from, rayTo: from)]))
            XCTAssertEqual(angularDistance(before, placement.viewpoint), 0, accuracy: 1e-8)
        }
    }

    func testOffGlobeDragKeepsTableAngularSpeedWhileZooming() {
        for height in heights {
            var placement = placed(height)
            let before = placement.viewpoint
            let (center, _) = sphere(placement)
            let from = simd_normalize(-center)
            let travel = placement.current.rotation.act(SIMD3<Double>(0.01, 0, 0))
            placement.apply(MapGestureInput.Delta(moves: [MapGestureInput.Move(
                travel: travel, beginsGesture: true, rayOrigin: .zero, rayFrom: from,
                rayTo: simd_normalize(from + travel))]))
            let displacement = angularDistance(before, placement.viewpoint)
            XCTAssertEqual(displacement, 0.01 / MapPlacement.tableRadius, accuracy: 1e-9,
                           "off-globe drag slows down at height \(height)")
        }
    }

    func testOffGlobePinchKeepsItsModeWhenRayCrossesOntoGlobe() {
        var placement = placed(8_000_000)
        let (center, _) = sphere(placement)
        let away = simd_normalize(-center)
        let travel = placement.current.rotation.act(SIMD3<Double>(0.01, 0, 0))
        placement.apply(MapGestureInput.Delta(moves: [MapGestureInput.Move(
            travel: travel, beginsGesture: true, rayOrigin: .zero, rayFrom: away, rayTo: away)]))
        let first = placement.viewpoint
        let from = simd_normalize(placement.current.translation)
        placement.apply(MapGestureInput.Delta(moves: [MapGestureInput.Move(
            travel: travel, rayOrigin: .zero, rayFrom: from, rayTo: from)]))
        XCTAssertEqual(angularDistance(first, placement.viewpoint),
                       0.01 / MapPlacement.tableRadius, accuracy: 1e-9)
    }

    func testTwoHandCarryMovesGlobeWithoutRotatingIt() {
        var placement = placed(MapPlacement.tableHeight)
        let before = placement.viewpoint
        let center = placement.tableCenter
        let travel = SIMD3<Double>(0.1, 0.05, 0)
        placement.apply(MapGestureInput.Delta(moves: [MapGestureInput.Move(
            travel: travel, rayOrigin: nil, rayFrom: nil, rayTo: nil)]))
        XCTAssertEqual(placement.tableCenter, center + travel)
        XCTAssertEqual(placement.viewpoint.latitude, before.latitude, accuracy: 1e-9)
        XCTAssertEqual(placement.viewpoint.longitude, before.longitude, accuracy: 1e-9)
    }

    private func angularDistance(_ a: Viewpoint, _ b: Viewpoint) -> Double {
        let a = geographicDirection(SIMD3<Double>(0, 0, 1), a)
        let b = geographicDirection(SIMD3<Double>(0, 0, 1), b)
        return atan2(simd_length(simd_cross(a, b)), simd_dot(a, b))
    }

    private func placed(_ height: Double) -> MapPlacement {
        var placement = MapPlacement(viewpoint: .above(.innsbruck, height: height))
        placement.tableCenter = SIMD3<Double>(0, 0, -1)
        placement.place(viewer: .zero)
        return placement
    }

    private func sphere(_ placement: MapPlacement) -> (SIMD3<Double>, Double) {
        let radius = MapPlacement.earthRadiusMeters * exp(placement.current.logScale)
        let center = placement.current.translation - placement.current.rotation.act(SIMD3<Double>(0, 0, radius))
        return (center, radius)
    }

    private func intersection(_ ray: SIMD3<Double>, _ center: SIMD3<Double>, _ radius: Double) -> SIMD3<Double>? {
        let along = simd_dot(ray, center)
        let discriminant = along * along - simd_length_squared(center) + radius * radius
        guard discriminant >= 0, along > 0 else { return nil }
        return ray * (along - sqrt(discriminant))
    }

    private func basis(_ viewpoint: Viewpoint) -> simd_double3x3 {
        let lat = viewpoint.latitude * .pi / 180
        let lon = viewpoint.longitude * .pi / 180
        return simd_double3x3(
            SIMD3<Double>(-sin(lon), cos(lon), 0),
            SIMD3<Double>(-sin(lat) * cos(lon), -sin(lat) * sin(lon), cos(lat)),
            SIMD3<Double>(cos(lat) * cos(lon), cos(lat) * sin(lon), sin(lat)))
    }

    private func geographicDirection(_ local: SIMD3<Double>, _ viewpoint: Viewpoint) -> SIMD3<Double> {
        basis(viewpoint) * local
    }

    private func localDirection(_ geographic: SIMD3<Double>, _ viewpoint: Viewpoint) -> SIMD3<Double> {
        basis(viewpoint).transpose * geographic
    }
}

final class GlobeOrientationTests: XCTestCase {
    func testDragCrossesBothPolesWithoutAnOrientationJump() throws {
        for sign in [-1.0, 1.0] {
            var latitude = sign * 80
            var longitude = 11.0
            var roll = 0.0
            let grabbed = SIMD3<Double>(0, 0, 1)
            let pulled = simd_quatd(angle: sign * 0.05, axis: SIMD3<Double>(1, 0, 0)).act(grabbed)
            var reachedCap = false
            for _ in 0..<12 {
                let before = simd_double3x3(simd_quatd(angle: roll, axis: grabbed))
                    * GlobeDrag.geographicBasis(latitude: latitude, longitude: longitude).transpose
                let focus = try XCTUnwrap(GlobeDrag.focus(grabbed: grabbed, pulled: simd_quatd(angle: -roll, axis: grabbed).act(pulled),
                                                        latitude: latitude, longitude: longitude, roll: roll))
                latitude = focus.latitude
                longitude = focus.longitude
                roll = focus.roll
                reachedCap = reachedCap || abs(latitude) > 88
                let after = simd_double3x3(simd_quatd(angle: roll, axis: grabbed))
                    * GlobeDrag.geographicBasis(latitude: latitude, longitude: longitude).transpose
                let change = simd_quatd(after * before.transpose)
                XCTAssertEqual(abs(change.angle), 0.05, accuracy: 1e-6)
            }
            XCTAssertTrue(reachedCap)
            XCTAssertLessThan(abs(latitude), 80, "drag continues onto the other side of the pole")
        }
    }

    func testTiltIsExplicitAndPreservesEyeHeightAndPhysicalHeadMotion() {
        var placement = MapPlacement(viewpoint: .above(.innsbruck, height: 4000), cameraPolicy: .fixedViewpoint)
        let viewer = SIMD3<Double>(0, 1.6, 0)
        placement.place(viewer: viewer)
        placement.setTilt(.pi / 4)
        let pose = placement.current
        XCTAssertEqual(simd_length(pose.translation - viewer), 4000, accuracy: 1e-7)
        let eyeInScene = pose.rotation.inverse.act(viewer - pose.translation)
        XCTAssertEqual(eyeInScene.z, 4000, accuracy: 1e-7)
        let movedHead = viewer + SIMD3<Double>(0.1, 0, 0)
        let translated = pose.rotation.inverse.act(movedHead - pose.translation) - eyeInScene
        XCTAssertEqual(simd_length(translated), 0.1, accuracy: 1e-7)
        _ = placement.advance(at: 1)
        XCTAssertEqual(placement.current.translation, pose.translation)
        placement.levelView()
        XCTAssertEqual(placement.viewpoint.tilt, 0)
        XCTAssertEqual(placement.current.translation, viewer - SIMD3<Double>(0, 4000, 0))
    }
}
