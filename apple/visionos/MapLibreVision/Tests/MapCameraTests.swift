import XCTest
import simd
@testable import MapInteraction

final class MapCameraTests: XCTestCase {
    func testOrbitKeepsCenterGroundTargetWhileEyeCirclesIt() {
        var placement = MapPlacement(viewpoint: .above(.innsbruck, height: 4000))
        placement.place(viewer: .zero)
        let radius = MapPlacement.earthRadiusMeters
        let localTarget = SIMD3<Double>(0, radius * sin(0.001), radius * (cos(0.001) - 1))
        let target = placement.current.translation + placement.current.rotation.act(localTarget)
        placement.updateViewRay(origin: .zero, direction: simd_normalize(target))
        let before = placement.current.worldFromScene().inverse.columns.3
        placement.apply(.init(turn: 0.3, pitch: 0.2, beginsOrbit: true))
        let moved = placement.current.translation + placement.current.rotation.act(localTarget)
        XCTAssertLessThan(simd_length(moved - target), 1e-6)
        let after = placement.current.worldFromScene().inverse.columns.3
        XCTAssertGreaterThan(simd_length(after - before), 100)
        let eyeBefore = SIMD3<Double>(before.x, before.y, before.z)
        let eyeAfter = SIMD3<Double>(after.x, after.y, after.z)
        XCTAssertEqual(simd_length(eyeBefore - localTarget), simd_length(eyeAfter - localTarget), accuracy: 1e-6)
        placement.updateViewRay(origin: SIMD3<Double>(0.1, 0, 0), direction: SIMD3<Double>(1, 0, 0))
        placement.apply(.init(turn: 0.1))
        let still = placement.current.translation + placement.current.rotation.act(localTarget)
        XCTAssertLessThan(simd_length(still - target), 1e-6, "head motion cannot steer an active orbit")
    }

    func testFixedViewpointTurnsWithoutMovingEyeInScene() {
        var placement = MapPlacement(viewpoint: .above(.innsbruck, height: 4000), cameraPolicy: .fixedViewpoint)
        placement.place(viewer: .zero)
        let before = placement.current.worldFromScene().inverse.columns.3
        placement.apply(.init(turn: 0.5, pitch: 0.4, beginsOrbit: true))
        let after = placement.current.worldFromScene().inverse.columns.3
        XCTAssertLessThan(simd_length(after - before), 1e-6)
    }

    func testSkyFacingOrbitUsesFiniteSurfaceFocus() {
        var placement = MapPlacement(viewpoint: .above(.innsbruck, height: 4000))
        placement.place(viewer: .zero)
        placement.updateViewRay(origin: .zero, direction: SIMD3<Double>(0, 1, 0))
        let target = placement.current.translation
        placement.apply(.init(turn: 0.3, pitch: 0.3, beginsOrbit: true))
        XCTAssertLessThan(simd_length(placement.current.translation - target), 1e-6)
    }
}
