import XCTest
import simd
@testable import MapInteraction

final class MapDragPlaneTests: XCTestCase {
    func testHorizonAndSkyPanRemainFiniteAndCannotChangeAltitude() {
        for y in [-0.1, -0.001, 0, 0.001, 0.1] {
            let ray = simd_normalize(SIMD3<Double>(0, y, -1))
            let plane = MapDragPlane(origin: .zero, ray: ray,
                                     surface: SIMD3<Double>(0, -4000, 0), normal: SIMD3<Double>(0, 1, 0))
            let next = simd_normalize(ray + SIMD3<Double>(0.01, 0.01, 0))
            let travel = plane.translation(from: ray, to: next)
            XCTAssertTrue(travel.x.isFinite && travel.z.isFinite)
            XCTAssertEqual(travel.y, 0, accuracy: 1e-9)
            XCTAssertLessThan(simd_length(travel), 240)
        }
    }

    func testCapturedPlaneDoesNotSwitchWhenPointerCrossesHorizon() {
        let ray = simd_normalize(SIMD3<Double>(0, -0.001, -1))
        let plane = MapDragPlane(origin: .zero, ray: ray,
                                 surface: SIMD3<Double>(0, -1000, 0), normal: SIMD3<Double>(0, 1, 0))
        let center = SIMD3<Double>(0, 0, -1)
        let sky = simd_normalize(SIMD3<Double>(0, 0.001, -1))
        let a = plane.translation(from: ray, to: center)
        let b = plane.translation(from: center, to: sky)
        XCTAssertLessThan(simd_length(a - b), 0.01)
    }
    func testVerticalHorizonDragMovesAcrossGroundInsteadOfLosingItsVerticalComponent() {
        let ray = SIMD3<Double>(0, 0, -1)
        let plane = MapDragPlane(origin: .zero, ray: ray,
                                 surface: SIMD3<Double>(0, -1000, 0), normal: SIMD3<Double>(0, 1, 0))
        let travel = plane.translation(from: ray, to: simd_normalize(SIMD3<Double>(0, 0.01, -1)))
        XCTAssertEqual(travel.y, 0, accuracy: 1e-9)
        XCTAssertEqual(travel.z, -40, accuracy: 1e-9)
        XCTAssertEqual(travel.x, 0, accuracy: 1e-9)
    }

}
