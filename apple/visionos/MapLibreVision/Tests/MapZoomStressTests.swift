import XCTest
import simd
@testable import MapInteraction

final class MapZoomStressTests: XCTestCase {
    func testSweepReachesGroundAndTableWithBoundedContinuousSteps() {
        var sweep = MapZoomStress()
        var height = MapPlacement.tableHeight
        var minimum = height
        var maximumStep = 0.0
        for step in 0...3600 {
            let input = sweep.input(at: Double(step) / 60, height: height,
                                    origin: .zero, focus: SIMD3<Double>(0, 0, -1))
            maximumStep = max(maximumStep, abs(input.logScale))
            height /= exp(input.logScale)
            minimum = min(minimum, height)
            XCTAssertGreaterThanOrEqual(height, 3999.99)
            XCTAssertLessThanOrEqual(height, MapPlacement.tableHeight + 0.01)
        }
        XCTAssertEqual(minimum, 4000, accuracy: 1e-6)
        XCTAssertEqual(height, MapPlacement.tableHeight, accuracy: 1e-6)
        XCTAssertLessThan(maximumStep, 0.02)
    }
}
