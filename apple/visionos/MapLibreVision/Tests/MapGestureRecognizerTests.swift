import XCTest
import simd
@testable import MapInteraction

final class MapGestureRecognizerTests: XCTestCase {
    typealias Recognizer = MapGestureRecognizer<Int>
    private func hand(_ id: Int, _ x: Double, _ y: Double = 0, _ z: Double = -0.5) -> Recognizer.Sample {
        .init(id: id, position: SIMD3<Double>(x, y, z), rayOrigin: .zero,
              rayDirection: simd_normalize(SIMD3<Double>(x, y, -1)))
    }

    func testAddingOrRemovingAHandCannotLeakSingleHandMotion() {
        var engine = Recognizer()
        _ = engine.handle([hand(1, -0.15)])
        let joined = engine.handle([hand(1, -0.2), hand(2, 0.2)])
        XCTAssertTrue(joined.moves.isEmpty)
        let left = engine.handle([hand(1, -0.25), .init(id: 2)])
        XCTAssertTrue(left.moves.isEmpty)
        let resumed = engine.handle([hand(1, -0.26)])
        XCTAssertEqual(resumed.moves.count, 1)
        XCTAssertTrue(resumed.moves[0].beginsGesture)
        XCTAssertEqual(resumed.moves[0].travel.x, -0.01, accuracy: 1e-9)
    }

    func testStationaryHandsAndMovingHeadCannotMoveMap() {
        var engine = Recognizer()
        _ = engine.handle([hand(1, 0.1)])
        _ = engine.handle([hand(1, 0.15)])
        engine.head = SIMD3<Double>(0.3, 0.2, 0.1)
        let delta = engine.handle([hand(1, 0.15)])
        XCTAssertEqual(delta.moves[0].rayFrom, delta.moves[0].rayTo)
        XCTAssertEqual(delta.moves[0].travel, .zero)
    }

    func testTrackingNoiseDoesNotAccumulateIntoZoomOrRotation() {
        var engine = Recognizer()
        _ = engine.handle([hand(1, -0.15), hand(2, 0.15)])
        for i in 0..<1000 {
            let jitter = i.isMultiple(of: 2) ? 0.001 : -0.001
            let delta = engine.handle([hand(1, -0.15 + jitter), hand(2, 0.15 - jitter, jitter)])
            XCTAssertEqual(delta.logScale, 0)
            XCTAssertEqual(delta.turn, 0)
            XCTAssertEqual(delta.translation, .zero)
            XCTAssertTrue(delta.moves.isEmpty)
        }
    }

    func testSmallTwistInitiatesRotationWithoutZoomOrPlacement() {
        var engine = Recognizer()
        _ = engine.handle([hand(1, -0.15), hand(2, 0.15)])
        let angle = 4.0 * Double.pi / 180
        let delta = engine.handle([hand(1, -0.15 * cos(angle), -0.15 * sin(angle)),
                                   hand(2, 0.15 * cos(angle), 0.15 * sin(angle))])
        XCTAssertEqual(delta.turn, angle, accuracy: 1e-9)
        XCTAssertTrue(delta.beginsOrbit)
        XCTAssertEqual(delta.logScale, 0)
        XCTAssertEqual(delta.translation, .zero)
    }

    func testCommonImmersiveHandMotionOrbitsAndPitchesWithoutZoom() {
        var engine = Recognizer()
        engine.setGlobe(false)
        _ = engine.handle([hand(1, -0.15), hand(2, 0.15)])
        let delta = engine.handle([hand(1, -0.12, 0.03), hand(2, 0.18, 0.03)])
        XCTAssertTrue(delta.beginsOrbit)
        XCTAssertEqual(delta.turn, 0.075, accuracy: 1e-9)
        XCTAssertEqual(delta.pitch, 0.075, accuracy: 1e-9)
        XCTAssertEqual(delta.logScale, 0)
        XCTAssertEqual(delta.translation, .zero)
    }

    func testZoomCannotTurnOrCarryEvenWithAsymmetricHandMotion() {
        var engine = Recognizer()
        _ = engine.handle([hand(1, -0.15), hand(2, 0.15)])
        _ = engine.handle([hand(1, -0.2), hand(2, 0.2)])
        let delta = engine.handle([hand(1, -0.23, 0.01), hand(2, 0.26, 0.07)])
        XCTAssertGreaterThan(delta.logScale, 0)
        XCTAssertEqual(delta.turn, 0)
        XCTAssertEqual(delta.translation, .zero)
        XCTAssertTrue(delta.moves.isEmpty)
    }

    func testPlacementCannotZoomOrTurn() {
        var engine = Recognizer()
        _ = engine.handle([hand(1, -0.15), hand(2, 0.15)])
        _ = engine.handle([hand(1, -0.15, 0.08), hand(2, 0.15, 0.08)])
        let delta = engine.handle([hand(1, -0.05, 0.10), hand(2, 0.25, 0.10)])
        XCTAssertEqual(delta.translation.x, 0.1, accuracy: 1e-9)
        XCTAssertEqual(delta.logScale, 0)
        XCTAssertEqual(delta.turn, 0)
    }

    func testEnteringGroundRequiresReleaseBeforeMotion() {
        var engine = Recognizer()
        _ = engine.handle([hand(1, 0)])
        engine.setGlobe(false)
        XCTAssertTrue(engine.handle([hand(1, 0.2)]).moves.isEmpty)
        _ = engine.handle([.init(id: 1)])
        _ = engine.handle([hand(1, 0.2)])
        XCTAssertEqual(engine.handle([hand(1, 0.3)]).moves.count, 1)
    }

    func testPairedPlacementTracksDepthAndDoesNotBecomeZoom() {
        var engine = Recognizer()
        _ = engine.handle([hand(1, -0.15), hand(2, 0.15)])
        _ = engine.handle([hand(1, -0.15, 0, -0.45), hand(2, 0.15, 0, -0.45)])
        let delta = engine.handle([hand(1, -0.20, 0, -0.40), hand(2, 0.20, 0, -0.40)])
        XCTAssertEqual(delta.translation.z, 0.05, accuracy: 1e-9)
        XCTAssertEqual(delta.logScale, 0)
    }
    func testShortStationaryPinchSelectsWithoutDragging() {
        var engine = Recognizer()
        _ = engine.handle([hand(1, 0.1)])
        XCTAssertTrue(engine.handle([hand(1, 0.101)]).moves.isEmpty)
        let released = engine.handle([.init(id: 1, timestamp: 0.2)])
        XCTAssertNotNil(released.selection)
        XCTAssertTrue(released.moves.isEmpty)
    }

    func testDragPairCancellationAndLongHoldCannotSelect() {
        for mode in 0..<4 {
            var engine = Recognizer()
            _ = engine.handle([hand(1, 0.1)])
            if mode == 0 { _ = engine.handle([hand(1, 0.12)]) }
            if mode == 1 {
                _ = engine.handle([hand(2, -0.1)])
                _ = engine.handle([.init(id: 2)])
            }
            let result = engine.handle([.init(id: 1, timestamp: mode == 3 ? 1.0 : 0.2, cancelled: mode == 2)])
            XCTAssertNil(result.selection)
        }
    }

    func testNaturalUnevenCarryWinsBeforeSmallAccidentalTwist() {
        var engine = Recognizer()
        _ = engine.handle([hand(1, -0.15), hand(2, 0.15)])
        _ = engine.handle([hand(1, -0.133, 0.006), hand(2, 0.164, -0.002)])
        let delta = engine.handle([hand(1, -0.123, 0.012), hand(2, 0.174, 0.004)])
        XCTAssertEqual(delta.translation.x, 0.01, accuracy: 1e-9)
        XCTAssertEqual(delta.turn, 0)
        XCTAssertEqual(delta.logScale, 0)
    }

    func testZoomContinuesAcrossImmersionBoundaryWithoutRelease() {
        var engine = Recognizer()
        _ = engine.handle([hand(1, -0.15), hand(2, 0.15)])
        _ = engine.handle([hand(1, -0.20), hand(2, 0.20)])
        XCTAssertTrue(engine.handle([hand(1, -0.21), hand(2, 0.21)]).beginsZoom)
        XCTAssertFalse(engine.setGlobe(false))
        let delta = engine.handle([hand(1, -0.22), hand(2, 0.22)])
        XCTAssertGreaterThan(delta.logScale, 0)
        XCTAssertFalse(delta.beginsZoom)
    }

    func testPointerGainIsIdenticalAcrossModesAndRestingHandDepths() throws {
        for globe in [true, false] {
            for depth in [0.15, 0.3, 0.6] {
                var engine = Recognizer()
                engine.setGlobe(globe)
                _ = engine.handle([hand(1, 0, 0, -depth)])
                let delta = engine.handle([hand(1, 0.03, 0, -depth)])
                let ray = try XCTUnwrap(delta.moves.first?.rayTo)
                XCTAssertEqual(atan2(ray.x, -ray.z), atan(0.03 / 0.6), accuracy: 1e-9)
            }
        }
    }

    func testMovingHandAlongSelectedRayDoesNotPanOrZoom() {
        var engine = Recognizer()
        _ = engine.handle([hand(1, 0)])
        let delta = engine.handle([hand(1, 0, 0, -0.4)])
        XCTAssertEqual(delta.moves.first?.rayFrom, delta.moves.first?.rayTo)
        XCTAssertEqual(delta.logScale, 0)
    }

}

extension MapGestureRecognizerTests {
    func testCarryCapturesReferenceOnceAndDoesNotFollowHeadMotion() throws {
        var engine = Recognizer()
        _ = engine.handle([hand(1, -0.15), hand(2, 0.15)])
        _ = engine.handle([hand(1, -0.10), hand(2, 0.20)])
        let first = engine.handle([hand(1, -0.08), hand(2, 0.22)])
        let reference = try XCTUnwrap(first.carryReference)
        XCTAssertEqual(reference.origin, .zero)
        XCTAssertEqual(reference.handDepth, 0.6, accuracy: 1e-9)
        engine.head = SIMD3<Double>(0.4, 0.2, 0.3)
        let still = engine.handle([hand(1, -0.08), hand(2, 0.22)])
        XCTAssertEqual(still.translation, .zero)
        XCTAssertNil(still.carryReference)
    }
}
