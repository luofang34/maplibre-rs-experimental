import XCTest
@testable import MapInteraction

final class FlightLibrarySelectionTests: XCTestCase {
    @MainActor
    func testDeletingAnotherFlightPreservesPlaybackAndSelection() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let library = FlightLibrary(directory: root)
        let url = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("MapLibreVision/Resources/liberty.kml")
        let bytes = try Data(contentsOf: url)
        let first = try await library.importTrack(bytes, name: "Liberty.kml", elevation: .msl, title: "First").0
        let otherBytes = Data(String(decoding: bytes, as: UTF8.self).replacingOccurrences(of: "N9758H", with: "N9759H").utf8)
        let other = try await library.importTrack(otherBytes, name: "Liberty.kml", elevation: .msl, title: "Other").0
        let suite = UUID().uuidString
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let session = GlobeSession(library: library, defaults: defaults)
        await session.loadLibrary()
        await session.select(first)
        session.replay.seek(90)
        session.replay.toggle()
        let before = session.replay.frame()
        await session.remove(other)
        let after = session.replay.frame()
        XCTAssertEqual(session.selectedTrackID, first.id)
        XCTAssertEqual(after.generation, before.generation)
        XCTAssertEqual(after.cameraRevision, before.cameraRevision)
        XCTAssertTrue(after.playing)
        XCTAssertGreaterThanOrEqual(after.elapsed, before.elapsed)
        XCTAssertFalse(session.tracks.contains(other))
        let stored = try await library.entries()
        XCTAssertFalse(stored.contains(other))
        await session.remove(first)
        XCTAssertNil(session.replay.frame().track)
        XCTAssertFalse(session.replay.frame().playing)
        XCTAssertEqual(session.selectedTrackID, "")
    }
}
