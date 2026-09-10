import XCTest
import simd
@testable import MapInteraction

final class FlightHUDTests: XCTestCase {
    private func point(attitude: Bool = true) -> FlightTrack.Observation {
        .init(time: 0, latitude: 40, longitude: -74, altitudeMSL: 1000,
            altitudeGNSS: nil, geoidSeparation: nil, groundSpeed: 60, track: 90,
            roll: attitude ? 30 : nil, pitch: attitude ? 5 : nil, heading: attitude ? 80 : nil)
    }

    func testHeadRotationCannotChangeFlightReferencedWings() throws {
        let point = point(), geometry = FlightHUDGeometry(point: point, verticalSpeed: -3)
        let wing = geometry.marker(.prograde)[32]
        var placement = MapPlacement(viewpoint: .above(point.coordinate, height: 1000))
        var camera = FlightCamera()
        camera.update(point, placement: &placement, head: [0, 1.6, 0], forward: [0, 0, -1])
        XCTAssertLessThan(simd_distance(placement.current.rotation.act(try XCTUnwrap(geometry.bodyForward)), [0, 0, -1]), 1e-12)
        XCTAssertLessThan(simd_distance(placement.current.rotation.act(geometry.bodyRight), [1, 0, 0]), 1e-12)
        let roomWing = placement.current.rotation.act(wing.b - wing.a)
        let headRoll = simd_quatd(angle: .pi / 4, axis: [0, 0, 1])
        let viewed = headRoll.inverse.act(roomWing)
        XCTAssertLessThan(simd_distance(headRoll.act(viewed), roomWing), 1e-12)
        XCTAssertGreaterThan(simd_distance(simd_normalize(viewed), simd_normalize(roomWing)), 0.5)
        XCTAssertEqual(simd_length(try XCTUnwrap(geometry.velocity)), 1, accuracy: 1e-12)
    }

    func testRetrogradeIsOppositeVelocityAndHasCrossedRing() throws {
        let geometry = FlightHUDGeometry(point: point(), verticalSpeed: -3)
        let prograde = geometry.marker(.prograde), retrograde = geometry.marker(.retrograde)
        XCTAssertEqual(prograde.count, 35)
        XCTAssertEqual(retrograde.count, 37)
        let center = simd_normalize(prograde.prefix(32).reduce(SIMD3<Double>.zero) { $0 + $1.a })
        let behind = simd_normalize(retrograde.prefix(32).reduce(SIMD3<Double>.zero) { $0 + $1.a })
        XCTAssertEqual(simd_dot(center, behind), -1, accuracy: 1e-12)
        XCTAssertEqual(simd_dot(center, try XCTUnwrap(geometry.velocity)), 1, accuracy: 1e-12)
    }

    func testMissingAttitudeDoesNotInventBoresightOrPitchLadder() {
        let geometry = FlightHUDGeometry(point: point(attitude: false), verticalSpeed: -3)
        XCTAssertTrue(geometry.references().isEmpty)
        XCTAssertFalse(geometry.marker(.prograde).isEmpty)
        XCTAssertTrue(FlightHUDGeometry(point: point(), verticalSpeed: nil).marker(.prograde).isEmpty)
        var stopped = point()
        stopped.velocityAvailable = false
        XCTAssertTrue(FlightHUDGeometry(point: stopped, verticalSpeed: 0).marker(.retrograde).isEmpty)
    }

}
