import XCTest
@testable import MapInteraction

final class MapMemoryBudgetTests: XCTestCase {
    func testFootprintLimitsGrowthEvenWhenSystemReportsSpareMemory() {
        let gib: UInt64 = 1_024 * 1_024 * 1_024
        XCTAssertEqual(MapMemoryBudget.available(reported: 5 * gib, footprint: 2 * gib), gib)
        XCTAssertEqual(MapMemoryBudget.available(reported: gib / 4, footprint: gib), gib / 4)
    }

    func testExhaustedAndUnknownReportsRemainBounded() {
        XCTAssertEqual(MapMemoryBudget.available(reported: 0, footprint: 0), MapMemoryBudget.footprintLimit)
        XCTAssertEqual(MapMemoryBudget.available(reported: 5_000_000_000, footprint: UInt64.max), 1)
    }
}
