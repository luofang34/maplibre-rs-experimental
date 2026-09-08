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

final class MapScaleCameraTests: XCTestCase {
    func testPairSurfacePivotRemainsFixedAtEveryScale() {
        for height in [4000.0, 60_000, 100_000, 1_000_000, 8_000_000, 40_000_000] {
            var placement = MapPlacement(viewpoint: .above(.innsbruck, height: height))
            placement.tableCenter = SIMD3<Double>(0, 0, -1)
            placement.place(viewer: .zero)
            let radius = MapPlacement.earthRadiusMeters
            let local = SIMD3<Double>(radius * sin(0.001), 0, radius * (cos(0.001) - 1))
            let target = placement.current.translation + placement.current.rotation.act(local * exp(placement.current.logScale))
            placement.updateViewRay(origin: .zero, direction: SIMD3<Double>(0, 1, 0))
            let before = placement.current.rotation
            placement.apply(.init(turn: 0.2, pitch: 0.1, beginsOrbit: true,
                                  orbitAnchor: (.zero, simd_normalize(target))))
            let moved = placement.current.translation + placement.current.rotation.act(local * exp(placement.current.logScale))
            XCTAssertLessThan(simd_length(moved - target), 1e-5, "pivot at height \(height)")
            XCTAssertGreaterThan(abs((placement.current.rotation * before.inverse).angle), 0.19)
        }
    }

    func testCarryMatchesRoomDistanceAndOrientationAtIntermediateScales() {
        for height in [100_000.0, 1_000_000, 8_000_000, 40_000_000] {
            var placement = MapPlacement(viewpoint: .above(.innsbruck, height: height))
            placement.tableCenter = SIMD3<Double>(0, 0, -1)
            placement.place(viewer: .zero)
            let before = placement.current
            let travel = SIMD3<Double>(0.1, 0.05, 0.07)
            placement.apply(.init(translation: travel))
            XCTAssertLessThan(simd_length(placement.current.translation - before.translation - travel), 1e-6)
            XCTAssertLessThan(abs((placement.current.rotation * before.rotation.inverse).angle), 1e-6)
        }
    }

    func testFlightCapturesActualPoseAndArrivesWithoutSnap() {
        var placement = MapPlacement(viewpoint: .above(.innsbruck, height: 4_000))
        placement.place(viewer: .zero)
        placement.apply(.init(turn: 0.5, pitch: 0.4, beginsOrbit: true))
        let before = placement.current
        placement.fly(to: MapPlacement.tableHeight, at: 1, viewer: SIMD3<Double>(0.2, 0, 0))
        _ = placement.advance(at: 1)
        XCTAssertLessThan(simd_length(placement.current.translation - before.translation), 1e-6)
        XCTAssertLessThan(abs((placement.current.rotation * before.rotation.inverse).angle), 1e-6)
        _ = placement.advance(at: 1 + MapPlacement.transitionSeconds - 1e-5)
        let almost = placement.current
        _ = placement.advance(at: 1 + MapPlacement.transitionSeconds)
        XCTAssertLessThan(simd_length(placement.current.translation - almost.translation), 1e-5)
        XCTAssertLessThan(abs((placement.current.rotation * almost.rotation.inverse).angle), 1e-5)
        XCTAssertEqual(placement.viewpoint.tilt, 0)
        XCTAssertEqual(placement.viewpoint.globeRoll, 0)
    }
}

final class TableTwistTests: XCTestCase {
    func testCentralTwistSpinsGlobeAroundItsVisibleNormalWithoutSwingingCenter() {
        var placement = MapPlacement(viewpoint: .above(.innsbruck, height: MapPlacement.tableHeight))
        placement.tableCenter = SIMD3<Double>(0, 0, -1)
        placement.place(viewer: .zero)
        let before = placement.current
        let target = before.translation
        placement.apply(.init(turn: 0.3, beginsOrbit: true, orbitAnchor: (.zero, simd_normalize(target))))
        let radius = MapPlacement.earthRadiusMeters * exp(before.logScale)
        let center = placement.current.translation - placement.current.rotation.act(SIMD3<Double>(0, 0, radius))
        XCTAssertLessThan(simd_length(center - placement.tableCenter), 1e-6)
        XCTAssertLessThan(simd_length(placement.current.translation - target), 1e-6)
    }
}

final class MapZoomAnchorTests: XCTestCase {
    func testZoomIntoGroundCannotCarryEitherEyeInsideTheGlobe() {
        for tilt in [-0.25, 0.0, 0.25] {
            var placement = MapPlacement(viewpoint: .above(.innsbruck, height: MapPlacement.tableHeight))
            placement.tableCenter = SIMD3<Double>(0, tilt, -1)
            placement.place(viewer: .zero)
            let target = placement.current.translation
            let ray = simd_normalize(target)
            let orientation = placement.current.rotation
            for step in 0..<160 {
                placement.apply(.init(logScale: 0.08, beginsZoom: step == 0, focusAnchor: (.zero, ray)))
                let scale = exp(placement.current.logScale)
                let radius = MapPlacement.earthRadiusMeters * scale
                XCTAssertLessThan(abs((placement.current.rotation * orientation.inverse).angle), 1e-7)
                let center = placement.current.translation - placement.current.rotation.act(SIMD3<Double>(0, 0, radius))
                for offset in [-0.035, 0.035] {
                    XCTAssertGreaterThan(simd_length(SIMD3<Double>(offset, 0, 0) - center), radius,
                                         "tilt \(tilt), height \(placement.viewpoint.height)")
                }
            }
        }
    }

    func testZoomKeepsCapturedSurfaceUnderRayAcrossScalesAndHeadMotion() {
        for height in [4000.0, 60_000, 100_000, 1_000_000, 8_000_000, 40_000_000] {
            var placement = MapPlacement(viewpoint: .above(.innsbruck, height: height))
            placement.tableCenter = SIMD3<Double>(0, 0, -1)
            placement.place(viewer: .zero)
            let r = MapPlacement.earthRadiusMeters
            let local = SIMD3<Double>(r * sin(0.0001), 0, r * (cos(0.0001) - 1))
            let point = placement.current.translation + placement.current.rotation.act(local * exp(placement.current.logScale))
            let ray = simd_normalize(point)
            for step in 0..<20 {
                placement.updateViewRay(origin: SIMD3<Double>(0.2, 0.1, 0), direction: SIMD3<Double>(0, 1, 0))
                placement.apply(.init(logScale: step < 10 ? 0.08 : -0.08, beginsZoom: step == 0,
                                      focusAnchor: (.zero, ray)))
                let result = placement.current.translation + placement.current.rotation.act(local * exp(placement.current.logScale))
                XCTAssertLessThan(simd_length(simd_cross(simd_normalize(result), ray)), 1e-7,
                                  "height \(height), step \(step)")
            }
        }
    }

    func testTwoHandAnchorBacksUpSkyFacingFocus() {
        var placement = MapPlacement(viewpoint: .above(.innsbruck, height: 4000))
        placement.place(viewer: .zero)
        let ray = simd_normalize(placement.current.translation)
        placement.apply(.init(logScale: 0.4, beginsZoom: true,
                              focusAnchor: (.zero, SIMD3<Double>(0, 1, 0)), zoomAnchor: (.zero, ray)))
        XCTAssertLessThan(simd_length(simd_cross(simd_normalize(placement.current.translation), ray)), 1e-9)
        XCTAssertLessThan(placement.viewpoint.height, 4000)
    }
}
