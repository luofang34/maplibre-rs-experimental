import XCTest
@testable import MapInteraction
@testable import FlightExchange

final class FlightLibraryTests: XCTestCase {
    private var resource: URL {
        URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("MapLibreVision/Resources/liberty.kml")
    }

    func testActualFlightAwareKMLPreservesCoverageAndMissingAirData() throws {
        let track = try FlightImport.decode(Data(contentsOf: resource), name: "Liberty.kml")
        XCTAssertEqual(track.title, "Liberty")
        XCTAssertEqual(track.observations.count, 184)
        XCTAssertEqual(track.duration, 3057)
        let gap = try XCTUnwrap(zip(track.observations, track.observations.dropFirst()).first { $1.time - $0.time == 57 })
        XCTAssertNil(track.sample(at: (gap.0.time + gap.1.time) / 2))
        XCTAssertTrue(track.observations.allSatisfy { $0.indicatedAirspeed == nil && !$0.hasAttitude && $0.heading == nil })
        XCTAssertGreaterThan(track.observations.filter(\.hasVelocity).count, 150)
    }

    func testDemoRemovalSurvivesRelaunchAndCanBeRestored() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let bundleURL = root.appendingPathComponent("Flights.bundle")
        try FileManager.default.createDirectory(at: bundleURL, withIntermediateDirectories: true)
        try Data("<?xml version='1.0'?><plist version='1.0'><dict><key>CFBundleIdentifier</key><string>flight.test</string></dict></plist>".utf8).write(to: bundleURL.appendingPathComponent("Info.plist"))
        try FileManager.default.copyItem(at: resource, to: bundleURL.appendingPathComponent("liberty.kml"))
        let bundle = try XCTUnwrap(Bundle(url: bundleURL)), directory = root.appendingPathComponent("library")
        let library = FlightLibrary(directory: directory, bundle: bundle)
        let entries = try await library.entries(), entry = try XCTUnwrap(entries.first)
        XCTAssertEqual(entry.title, "Liberty")
        try await library.remove(entry)
        let restored = FlightLibrary(directory: directory, bundle: bundle)
        let empty = try await restored.entries()
        XCTAssertTrue(empty.isEmpty)
        try await restored.restoreDemos()
        let visible = try await restored.entries()
        XCTAssertEqual(visible.first?.title, "Liberty")
    }

    func testShareInboxCopiesActualKMLAndBoundsFiles() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let inbox = try FlightInbox(directory: root)
        try inbox.save(resource, name: "Liberty.kml")
        let url = try XCTUnwrap(inbox.pending().first)
        XCTAssertEqual(try Data(contentsOf: url), try Data(contentsOf: resource))
        XCTAssertEqual(try FlightImport.decode(FlightImport.read(url), name: url.lastPathComponent).observations.count, 184)
        XCTAssertThrowsError(try inbox.save(resource, name: "unsupported.txt"))
        for _ in 1..<10 { try inbox.save(resource, name: "Liberty.kml") }
        XCTAssertThrowsError(try inbox.save(resource, name: "overflow.kml"))
        try inbox.remove(url)
        XCTAssertEqual(try inbox.pending().count, 9)
    }

    func testProviderWithoutFilenameExtensionUsesDeclaredTrackType() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let temporary = root.appendingPathComponent("provider-data")
        try FileManager.default.copyItem(at: resource, to: temporary)
        let name = try FlightInbox.fileName(source: temporary, suggested: "Liberty", contentType: "com.google.earth.kml")
        XCTAssertEqual(name, "Liberty.kml")
        let inbox = try FlightInbox(directory: root.appendingPathComponent("inbox"))
        try inbox.save(temporary, name: name)
        let saved = try XCTUnwrap(inbox.pending().first)
        XCTAssertEqual(try FlightImport.decode(FlightImport.read(saved), name: saved.lastPathComponent).observations.count, 184)
        XCTAssertThrowsError(try FlightInbox.fileName(source: temporary, suggested: nil, contentType: "public.data"))
    }
}
