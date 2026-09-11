import XCTest
import simd
@testable import MapInteraction

final class FlightReplayTests: XCTestCase {
    func testMapOnlyModeDoesNotEnableFlightAndCancelsReturnToOwnship() throws {
        let replay = FlightReplay()
        replay.replace(with: try track())
        XCTAssertFalse(replay.frame().enabled)
        replay.setEnabled(true, at: 0)
        replay.setView(.fpv, at: 0)
        replay.toggle(at: 0)
        replay.boarded(revision: replay.frame(at: 0).cameraRevision, at: 0)
        replay.navigate(at: 2)
        XCTAssertNotNil(replay.frame(at: 2).returnSeconds)
        replay.setEnabled(false, at: 3)
        replay.advanceReturn(at: 100)
        let frame = replay.frame(at: 100)
        XCTAssertFalse(frame.enabled)
        XCTAssertFalse(frame.following)
        XCTAssertFalse(frame.playing)
        XCTAssertNil(frame.returnSeconds)
        XCTAssertNotNil(frame.track)
        replay.setEnabled(true, at: 100)
        XCTAssertEqual(replay.frame(at: 100).elapsed, frame.elapsed)
        XCTAssertFalse(replay.frame(at: 100).following)
    }

    private func track(_ name: String = "innsbruck-approach") throws -> FlightTrack {
        let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
        return try FlightTrack.decode(Data(contentsOf: root.appendingPathComponent("MapLibreVision/Resources/\(name).json")))
    }

    func testDetachPausesAndReturnWaitsUntilCameraArrives() throws {
        let replay = FlightReplay()
        replay.replace(with: try track())
        replay.setView(.fpv, at: 100)
        replay.toggle(at: 100)
        let revision = replay.frame(at: 100).cameraRevision
        XCTAssertFalse(replay.frame(at: 105).playing)
        replay.boarded(revision: revision &- 1, at: 105)
        XCTAssertFalse(replay.frame(at: 106).playing)
        replay.boarded(revision: revision, at: 110)
        XCTAssertEqual(replay.frame(at: 120).elapsed, 10)
        replay.setView(.free, at: 120)
        XCTAssertEqual(replay.frame(at: 500).elapsed, 10)
        XCTAssertFalse(replay.frame(at: 500).following)
        replay.setView(.chase, at: 500)
        replay.boarded(revision: replay.frame(at: 500).cameraRevision, at: 502)
        XCTAssertEqual(replay.frame(at: 504).elapsed, 12)
        replay.replace(with: try track("mach-loop"))
        let replacement = replay.frame(at: 1000)
        XCTAssertFalse(replacement.playing)
        XCTAssertEqual(replacement.view, .free)
        XCTAssertEqual(replacement.elapsed, 0)
        XCTAssertEqual(replacement.track?.title, "Conquering the Mach Loop")
    }

    func testExplicitPauseInFreeViewCancelsAutomaticResume() throws {
        let replay = FlightReplay()
        replay.replace(with: try track())
        replay.toggle(at: 10)
        replay.setView(.fpv, at: 20)
        replay.boarded(revision: replay.frame(at: 20).cameraRevision, at: 22)
        replay.setView(.free, at: 30)
        replay.toggle(at: 40)
        replay.toggle(at: 45)
        replay.setView(.fpv, at: 50)
        replay.boarded(revision: replay.frame(at: 50).cameraRevision, at: 52)
        XCTAssertFalse(replay.frame(at: 55).playing)
    }

    func testHeadingRemainsIndependentOfMissingAttitude() throws {
        var point = try XCTUnwrap(track().sample(at: 10))
        point.heading = 90
        point.pitch = nil
        var placement = MapPlacement(viewpoint: .above(.innsbruck, height: 4000))
        var camera = FlightCamera()
        camera.update(point, placement: &placement, head: .zero, forward: [0, 0, -1])
        let east = placement.current.rotation.act(SIMD3<Double>(1, 0, 0))
        XCTAssertLessThan(simd_distance(east, [0, 0, -1]), 0.000001)
        XCTAssertFalse(point.hasAttitude)
    }

    func testPositiveRightBankMakesWorldHorizonRiseToTheRight() throws {
        let flight = try track()
        var point = try XCTUnwrap(flight.sample(at: 10))
        point.pitch = 0
        point.heading = 0
        let json = try JSONEncoder().encode(point)
        var fields = try XCTUnwrap(JSONSerialization.jsonObject(with: json) as? [String: Any])
        fields["roll"] = 30.0
        point = try JSONDecoder().decode(FlightTrack.Observation.self, from: JSONSerialization.data(withJSONObject: fields))
        var placement = MapPlacement(viewpoint: .above(.innsbruck, height: 4000))
        var camera = FlightCamera()
        camera.update(point, placement: &placement, head: .zero, forward: [0, 0, -1])
        let east = placement.current.rotation.act(SIMD3<Double>(1, 0, 0))
        XCTAssertEqual(east.y, 0.5, accuracy: 0.000001)
        XCTAssertEqual(east.x, cos(.pi / 6), accuracy: 0.000001)
    }

    func testFreeCameraDetachHasNoPoseJumpAndReturnStartsAtVisiblePose() throws {
        let point = try XCTUnwrap(track().sample(at: 10))
        var placement = MapPlacement(viewpoint: .above(.innsbruck, height: 4000))
        placement.focusElevation = 700
        var camera = FlightCamera()
        camera.update(point, placement: &placement, head: [0, 1.6, 0], forward: [0, 0, -1], revision: 1, at: 0)
        let aircraft = placement.roomPoint(for: point.coordinate)
        let nearby = MapAnchor(latitude: point.latitude + 0.02, longitude: point.longitude + 0.03, altitudeMeters: 2000)
        let visible = placement.roomPoint(for: nearby)
        camera.detach(placement: &placement)
        XCTAssertEqual(placement.cameraPolicy, .freeOrbit)
        XCTAssertLessThan(simd_distance(placement.roomPoint(for: nearby), visible), 0.00001)
        XCTAssertLessThan(simd_distance(placement.pose(for: placement.viewpoint).translation, placement.current.translation), 0.001)
        camera.update(point, placement: &placement, head: [0, 1.6, 0], forward: [0, 0, -1], view: .chase, revision: 2, at: 10)
        XCTAssertTrue(camera.isBoarding)
        XCTAssertLessThan(simd_distance(placement.roomPoint(for: nearby), visible), 0.001)
        camera.update(point, placement: &placement, head: [0, 1.6, 0], forward: [0, 0, -1], view: .chase, revision: 2, at: 12)
        XCTAssertFalse(camera.isBoarding)
        XCTAssertGreaterThan(simd_distance(placement.roomPoint(for: point.coordinate), aircraft), 3000)
    }

    func testSeekDuringBoardingDoesNotAnimateInventedFlight() throws {
        let flight = try track(), point = try XCTUnwrap(flight.sample(at: 10))
        var placement = MapPlacement(viewpoint: .above(.innsbruck, height: 4000))
        var camera = FlightCamera()
        camera.update(point, placement: &placement, head: .zero, forward: [0, 0, -1], revision: 1, at: 0)
        camera.update(point, placement: &placement, head: .zero, forward: [0, 0, -1], view: .chase, revision: 2, at: 1)
        XCTAssertTrue(camera.isBoarding)
        camera.update(point, placement: &placement, head: .zero, forward: [0, 0, -1], view: .chase, revision: 2, generation: 1, at: 1.1)
        XCTAssertFalse(camera.isBoarding)
    }

    func testMachLoopIsExplicitlySimulationAndDoesNotInventIAS() throws {
        let flight = try track("mach-loop")
        XCTAssertTrue(flight.isSimulation)
        XCTAssertEqual(flight.title, "Conquering the Mach Loop")
        XCTAssertTrue(flight.source.coverage.contains("not a recorded flight"))
        XCTAssertGreaterThan(flight.duration, 300)
        XCTAssertLessThan(flight.duration, 500)
        XCTAssertTrue(flight.observations.allSatisfy { $0.hasAttitude && $0.indicatedAirspeed == nil })
        XCTAssertNotNil(flight.sample(at: 170))
    }
}
