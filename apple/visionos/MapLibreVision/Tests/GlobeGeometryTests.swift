import XCTest
import simd
@testable import MapInteraction

final class GlobeGeometryTests: XCTestCase {
    func testGeographicRoundTripIncludingPolesAndDateline() {
        for latitude in [-90.0, -47, 0, 47, 90] {
            for longitude in [-180.0, -100, 0, 100, 180] {
                let coordinate = MapAnchor(latitude: latitude, longitude: longitude, altitudeMeters: 0)
                let restored = GlobeGeometry.coordinate(GlobeGeometry.direction(coordinate))
                XCTAssertEqual(restored.latitude, latitude, accuracy: 1e-8)
                XCTAssertEqual(restored.longitude, longitude, accuracy: 1e-8)
            }
        }
    }
}
