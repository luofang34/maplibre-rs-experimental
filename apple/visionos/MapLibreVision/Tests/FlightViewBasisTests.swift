import XCTest
import simd
@testable import MapInteraction

final class FlightViewBasisTests: XCTestCase {
    func testReadoutBasisIgnoresHeadRoll() {
        let yawPitch = simd_quatf(angle: 0.7, axis: [0, 1, 0]) * simd_quatf(angle: 0.3, axis: [1, 0, 0])
        let reference = FlightViewBasis.leveled(head: .init(yawPitch), up: [0, 1, 0])
        for roll: Float in [-1, -0.4, 0.5, 1.2] {
            let head = simd_float4x4(yawPitch * simd_quatf(angle: roll, axis: [0, 0, 1]))
            let actual = FlightViewBasis.leveled(head: head, up: [0, 1, 0])
            XCTAssertLessThan(simd_distance(reference.columns.0, actual.columns.0), 1e-5)
            XCTAssertLessThan(simd_distance(reference.columns.1, actual.columns.1), 1e-5)
            XCTAssertLessThan(simd_distance(head.columns.2, actual.columns.2), 1e-5)
        }
    }

    func testOwnshipRemainsInGeographicPlaneAtPole() {
        let point = FlightTrack.Observation(time: 0, latitude: 89.99, longitude: 20, altitudeMSL: 1000,
            altitudeGNSS: nil, geoidSeparation: nil, groundSpeed: 60, track: 90, roll: nil, pitch: nil, heading: nil)
        let placement = MapPlacement(viewpoint: .above(point.coordinate, height: 1_000_000))
        let vertices = FlightOwnshipGeometry.vertices(observation: point, placement: placement, radius: 0.01)
        XCTAssertEqual(vertices.count, 6)
        let normal = placement.current.rotation.act([0, 0, 1])
        let origin = placement.roomPoint(for: point.coordinate)
        for vertex in vertices {
            XCTAssertLessThan(abs(simd_dot(vertex - origin, normal)), 1e-8)
            XCTAssertTrue(vertex.x.isFinite && vertex.y.isFinite && vertex.z.isFinite)
        }
    }
}
