import XCTest
@testable import MapInteraction

final class FlightReturnTests: XCTestCase {
    func testFreeLookCountdownResetsOnNavigationAndExplicitFreeCancelsIt() {
        let replay = FlightReplay()
        replay.setView(.fpv, at: 0)
        replay.navigate(at: 1)
        XCTAssertEqual(replay.frame(at: 5).returnSeconds, 6)
        replay.navigate(at: 8)
        XCTAssertEqual(replay.frame(at: 10).returnSeconds, 8)
        replay.advanceReturn(at: 17)
        XCTAssertEqual(replay.frame(at: 17).view, .free)
        replay.advanceReturn(at: 18)
        XCTAssertEqual(replay.frame(at: 18).view, .fpv)
        replay.navigate(at: 20)
        replay.setView(.free, at: 22)
        replay.advanceReturn(at: 100)
        XCTAssertEqual(replay.frame(at: 100).view, .free)
        XCTAssertNil(replay.frame(at: 100).returnSeconds)
    }
}
