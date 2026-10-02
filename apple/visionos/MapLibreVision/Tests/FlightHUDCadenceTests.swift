import XCTest
@testable import MapInteraction

final class FlightHUDCadenceTests: XCTestCase {
    func testThirtyUpdatesPerSecondAtNinetyDisplayFrames() {
        var cadence = FlightHUDCadence()
        let count = (0..<900).filter { cadence.consume(now: Double($0) / 90, changed: true, urgent: false) }.count
        XCTAssertEqual(count, 300)
    }

    func testUnchangedSamplesDoNotPaintAndUrgentChangesBypassDeadline() {
        var cadence = FlightHUDCadence()
        XCTAssertTrue(cadence.consume(now: 0, changed: true, urgent: false))
        XCTAssertFalse(cadence.consume(now: 0.01, changed: true, urgent: false))
        XCTAssertTrue(cadence.consume(now: 0.02, changed: false, urgent: true))
        XCTAssertFalse(cadence.consume(now: 2, changed: false, urgent: false))
        XCTAssertTrue(cadence.consume(now: 2, changed: true, urgent: false))
        XCTAssertFalse(cadence.consume(now: 2.001, changed: true, urgent: false))
    }
}
