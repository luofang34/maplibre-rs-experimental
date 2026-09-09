import XCTest
import simd
@testable import MapInteraction

final class DeskGlobeTests: XCTestCase {
    func testSphereHasOutwardTrianglesAndClosedPoles() {
        let mesh = GlobeGeometry.sphere(columns: 24, rows: 16)
        XCTAssertEqual(mesh.positions.count, 25 * 17)
        XCTAssertEqual(mesh.indices.count, 24 * 15 * 6)
        for i in stride(from: 0, to: mesh.indices.count, by: 3) {
            let a = mesh.positions[Int(mesh.indices[i])]
            let b = mesh.positions[Int(mesh.indices[i + 1])]
            let c = mesh.positions[Int(mesh.indices[i + 2])]
            XCTAssertGreaterThan(simd_dot(simd_cross(b - a, c - a), a + b + c), 0)
        }
        XCTAssertEqual(mesh.uv[0].y, 1, accuracy: 0.00001)
        XCTAssertEqual(mesh.uv[16 * 25].y, 0, accuracy: 0.00001)
        for i in 0...24 {
            XCTAssertEqual(mesh.positions[i].y, 1, accuracy: 0.00001)
            XCTAssertEqual(mesh.positions[16 * 25 + i].y, -1, accuracy: 0.00001)
        }
        XCTAssertTrue(mesh.uv.allSatisfy { $0.x.isFinite && $0.y.isFinite && (-0.00001...1.00001).contains($0.y) })
        for row in 0...16 {
            XCTAssertLessThan(simd_distance(mesh.positions[row * 25], mesh.positions[row * 25 + 24]), 1e-6)
        }
    }

    func testRestorationBoundsObjectSizeAndPreservesOrientation() throws {
        var state = DeskGlobeState()
        let rotation = simd_quatf(angle: 1.2, axis: simd_normalize(SIMD3<Float>(1, 2, 3)))
        state.record(rotation: rotation, radius: 4)
        state.playbackTime = 70
        let saved = try JSONDecoder().decode(DeskGlobeState.self, from: JSONEncoder().encode(state))
        XCTAssertEqual(saved.boundedRadius, 0.22)
        XCTAssertEqual(saved.playbackTime, 70)
        XCTAssertLessThan(simd_length(saved.rotation.vector - rotation.vector), 1e-6)
        state.orientation = [0, 0, 0, 0]
        state.radius = .nan
        XCTAssertEqual(state.boundedRadius, 0.17)
        XCTAssertTrue(state.rotation.vector.x.isFinite)
        state.orientation = Array(repeating: .greatestFiniteMagnitude, count: 4)
        XCTAssertTrue(state.rotation.vector.x.isFinite)
        XCTAssertGreaterThan(simd_length(state.rotation.vector), 0.99)
        let focus = SIMD3<Float>(GlobeGeometry.direction(.init(latitude: 47.26, longitude: 11.34, altitudeMeters: 0)))
        XCTAssertLessThan(simd_distance(DeskGlobeState.initialRotation.act(focus), [0, 0, 1]), 1e-6)
    }

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
