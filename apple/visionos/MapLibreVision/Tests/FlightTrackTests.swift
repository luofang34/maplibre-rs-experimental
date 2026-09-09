import XCTest
import simd
@testable import MapInteraction

final class FlightTrackTests: XCTestCase {
    private func recordingData() throws -> Data {
        let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
        return try Data(contentsOf: root.appendingPathComponent("MapLibreVision/Resources/innsbruck-approach.json"))
    }

    func testBundledRecordingHasMeasuredAltitudeAndBoundedCoverage() throws {
        let track = try FlightTrack.decode(recordingData())
        XCTAssertEqual(track.callsign, "AUA10A")
        XCTAssertEqual(track.registration, "OE-LWD")
        XCTAssertEqual(track.observations.count, 55)
        XCTAssertEqual(track.duration, 405.47, accuracy: 0.001)
        XCTAssertTrue(track.source.coverage.contains("before the runway"))
        for point in track.observations {
            XCTAssertEqual(point.altitudeGNSS - point.geoidSeparation, point.altitudeMSL, accuracy: 0.002)
            XCTAssertTrue((40...60).contains(point.geoidSeparation))
        }
        let end = try XCTUnwrap(track.observations.last)
        XCTAssertEqual(track.sample(at: track.duration + 5000)?.latitude, end.latitude)
        XCTAssertNil(track.sample(at: .nan))
    }

    func testInterpolationPreservesEndpointsAndDoesNotCrossReceiverOutage() throws {
        let track = try FlightTrack.decode(recordingData())
        let a = track.observations[0], b = track.observations[1]
        let midpoint = try XCTUnwrap(track.sample(at: (a.time + b.time) / 2))
        XCTAssertEqual(midpoint.altitudeMSL, (a.altitudeMSL + b.altitudeMSL) / 2, accuracy: 0.001)
        XCTAssertEqual(track.sample(at: b.time)?.longitude, b.longitude)
        var json = try XCTUnwrap(JSONSerialization.jsonObject(with: recordingData()) as? [String: Any])
        var points = try XCTUnwrap(json["observations"] as? [[String: Any]])
        points = [points[0], points[1]]
        points[1]["time"] = 45.0
        json["observations"] = points
        let outage = try FlightTrack.decode(JSONSerialization.data(withJSONObject: json))
        XCTAssertNil(outage.sample(at: 22))
        XCTAssertNotNil(outage.sample(at: 45))
        points[1]["time"] = 0.0
        json["observations"] = points
        XCTAssertThrowsError(try FlightTrack.decode(JSONSerialization.data(withJSONObject: json)))
    }

    func testPlaybackRateChangesDoNotJumpOrRunBeyondRecording() {
        var clock = FlightPlayback()
        clock.play(at: 100, duration: 400)
        XCTAssertEqual(clock.time(at: 110, duration: 400), 10)
        clock.setRate(4, at: 110, duration: 400)
        XCTAssertEqual(clock.time(at: 110, duration: 400), 10)
        XCTAssertEqual(clock.time(at: 120, duration: 400), 50)
        clock.pause(at: 120, duration: 400)
        XCTAssertEqual(clock.time(at: 500, duration: 400), 50)
        clock.play(at: 500, duration: 400)
        XCTAssertEqual(clock.time(at: 600, duration: 400), 400)
        clock.play(at: 700, duration: 400)
        XCTAssertEqual(clock.time(at: 700, duration: 400), 0)
    }

    func testFollowCameraPreservesRecordedPositionAndHeadMotionIsIndependent() throws {
        let observation = try XCTUnwrap(FlightTrack.decode(recordingData()).sample(at: 20))
        var placement = MapPlacement(viewpoint: .above(.innsbruck, height: 4000))
        placement.focusElevation = 700
        var camera = FlightCamera()
        camera.update(observation, placement: &placement, head: [0, 1.6, 0], forward: [0, 0, -1])
        let position = placement.roomPoint(for: observation.coordinate)
        XCTAssertEqual(position.x, 0, accuracy: 0.001)
        XCTAssertEqual(position.y, 1.6 - 600, accuracy: 0.001)
        XCTAssertEqual(position.z, -3500, accuracy: 0.001)
        camera.update(observation, placement: &placement, head: [0.2, 1.8, 0], forward: [1, 0, 0])
        XCTAssertLessThan(simd_distance(position, placement.roomPoint(for: observation.coordinate)), 0.001)
        let levelUp = placement.current.rotation.act(SIMD3<Double>(0, 0, 1))
        XCTAssertLessThan(simd_distance(levelUp, [0, 1, 0]), 0.0001)
    }
}
