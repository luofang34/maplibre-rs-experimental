import XCTest
import simd
@testable import MapInteraction

final class MapTiltTests: XCTestCase {
    func testZeroTiltLevelsTheRenderedSurfaceAfterZoomFromGlobe() {
        var placement = MapPlacement(viewpoint: .above(.innsbruck, height: MapPlacement.tableHeight))
        placement.tableCenter = SIMD3<Double>(0.3, -0.2, -1)
        placement.place(viewer: .zero)
        let ray = simd_normalize(placement.current.translation)
        placement.apply(.init(logScale: log(MapPlacement.tableHeight / 4000), beginsZoom: true,
                              focusAnchor: (.zero, ray)))
        XCTAssertGreaterThan(placement.sceneTilt, 0.5)
        placement.updateViewRay(origin: .zero, direction: ray)
        placement.setTilt(0)
        XCTAssertEqual(placement.sceneTilt, 0, accuracy: 1e-7)
        let up = placement.current.rotation.act(SIMD3<Double>(0, 0, 1))
        XCTAssertLessThan(simd_length(up - SIMD3<Double>(0, 1, 0)), 1e-7)
    }

    func testAbsoluteTiltAndYawAgreeWithRenderedOrientation() {
        var placement = MapPlacement(viewpoint: .above(.innsbruck, height: 4000))
        placement.place(viewer: .zero)
        placement.updateViewRay(origin: .zero, direction: simd_normalize(SIMD3<Double>(1, -1, -1)))
        for angle in [0.8, 0.2, 0.0, 1.1] {
            placement.setTilt(angle)
            XCTAssertEqual(placement.sceneTilt, angle, accuracy: 1e-7)
            placement.apply(.init(turn: 0.4, beginsOrbit: true))
            XCTAssertEqual(placement.sceneTilt, angle, accuracy: 1e-7)
        }
        let before = placement.current.worldFromScene()
        placement.updateViewRay(origin: SIMD3<Double>(0.2, 0.1, 0), direction: SIMD3<Double>(0, 1, 0))
        _ = placement.advance(at: 20)
        XCTAssertLessThan(simd_length(placement.current.worldFromScene().columns.1 - before.columns.1), 1e-7)
    }

    func testLevelKeepsFreeCameraTargetAndFixedCameraEye() {
        for policy in [MapCameraPolicy.freeOrbit, .fixedViewpoint] {
            var placement = MapPlacement(viewpoint: .above(.innsbruck, height: 4000), cameraPolicy: policy)
            placement.place(viewer: .zero)
            placement.setTilt(0.7)
            let point = policy == .freeOrbit ? placement.current.translation : .zero
            let local = placement.current.rotation.inverse.act(point - placement.current.translation)
            placement.updateViewRay(origin: .zero, direction: simd_normalize(placement.current.translation))
            placement.levelView()
            let moved = placement.current.translation + placement.current.rotation.act(local)
            XCTAssertLessThan(simd_length(point - moved), 1e-6)
            XCTAssertEqual(placement.sceneTilt, 0, accuracy: 1e-7)
        }
    }
}
