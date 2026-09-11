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

    func testVirtualPanelIsCollimatedAndGlanceHasHysteresis() {
        let panel = FlightViewBasis.instrument(rotation: simd_quatd(angle: 0, axis: [0, 1, 0]),
                                              right: [1, 0, 0], up: [0, 1, 0], forward: [0, 0, -1])
        func head(_ yaw: Float) -> simd_float4x4 {
            var result = simd_float4x4(simd_quatf(angle: yaw * .pi / 180, axis: [0, 1, 0]))
            result.columns.3 = [1, 2, 3, 1]
            return result
        }
        XCTAssertFalse(FlightViewBasis.useGlance(wasGlancing: false, head: head(32), instrument: panel))
        XCTAssertTrue(FlightViewBasis.useGlance(wasGlancing: true, head: head(32), instrument: panel))
        XCTAssertTrue(FlightViewBasis.useGlance(wasGlancing: false, head: head(40), instrument: panel))
        XCTAssertFalse(FlightViewBasis.useGlance(wasGlancing: true, head: head(20), instrument: panel))
        let ray = panel * SIMD4<Float>(0, 0, -2, 0)
        XCTAssertEqual(ray.w, 0)
        var translated = head(0)
        translated.columns.3 = [200, -20, 100, 1]
        XCTAssertEqual(simd_inverse(translated) * ray, simd_inverse(head(0)) * ray)
    }
}
