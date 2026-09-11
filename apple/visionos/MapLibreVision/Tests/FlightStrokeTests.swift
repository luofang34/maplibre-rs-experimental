import XCTest
import simd
@testable import MapInteraction

final class FlightStrokeTests: XCTestCase {
    private func corners(_ a: SIMD4<Float>, _ b: SIMD4<Float>, width: Float,
                         viewport: SIMD2<Float>) -> [FlightStroke.Corner] {
        var result: [FlightStroke.Corner] = []
        FlightStroke.forEachCorner(a, b, width: width, viewport: viewport) { result.append($0) }
        return result
    }

    func testVisiblePartSurvivesNearPlaneCrossing() {
        let corners = corners([-0.2, 0, 0.5, 1], [0.5, 0, 0.5, -1], width: 2, viewport: [800, 600])
        XCTAssertEqual(corners.count, 6)
        for c in corners {
            XCTAssertGreaterThan(c.position.w, 0)
            XCTAssertGreaterThanOrEqual(c.position.z, 0)
            XCTAssertLessThanOrEqual(c.position.z, c.position.w + 0.0001)
            XCTAssertTrue((0..<4).allSatisfy { c.position[$0].isFinite })
        }
    }

    func testStrokeKeepsPixelWidthAcrossPerspectiveDepth() {
        let viewport = SIMD2<Float>(800, 600)
        let corners = corners([-0.3, 0, 0.5, 1], [3, 0, 0.5, 10], width: 2, viewport: viewport)
        XCTAssertEqual(corners.count, 6)
        let y = corners.map { $0.position.y / $0.position.w * viewport.y * 0.5 }
        XCTAssertEqual(y[2] - y[0], 3.5, accuracy: 0.001)
        XCTAssertEqual(y[5] - y[1], 3.5, accuracy: 0.001)
        XCTAssertEqual(corners[0].capsule.w, 1)
    }

    func testBehindEyeAndDegenerateSegmentsDoNotAllocateGeometry() {
        let pairs: [(SIMD4<Float>, SIMD4<Float>)] = [(SIMD4<Float>(0, 0, 1, -1), SIMD4(1, 0, 1, -1)),
                       (SIMD4(0, 0, 0.5, 1), SIMD4(0, 0, 0.5, 1))]
        for (a, b) in pairs {
            XCTAssertTrue(corners(a, b, width: 2, viewport: [800, 600]).isEmpty)
        }
    }
}
