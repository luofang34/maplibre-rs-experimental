import XCTest
@testable import MapInteraction

final class FlightImportTests: XCTestCase {
    private func gpx(segments: Int = 1, geoid: Bool = false) -> Data {
        let correction = geoid ? "<geoidheight>48</geoidheight>" : ""
        let xml = (0..<segments).map { segment in
            "<trkseg>" + (0..<3).map { index in
                "<trkpt lat='47.26' lon='\(11.3 + Double(segment * 3 + index) * 0.001)'><ele>1000</ele>\(correction)<time>2026-09-01T12:00:\(String(format: "%02d", segment * 3 + index))Z</time></trkpt>"
            }.joined() + "</trkseg>"
        }.joined()
        return Data("<gpx xmlns='http://www.topografix.com/GPX/1/1'><trk>\(xml)</trk></gpx>".utf8)
    }

    func testGPXDerivesGroundVelocityButDoesNotInventAirData() throws {
        let track = try FlightImport.decode(gpx(), name: "Valley.gpx")
        XCTAssertEqual(track.title, "Valley")
        XCTAssertEqual(track.observations.count, 3)
        XCTAssertGreaterThan(track.observations[0].groundSpeed, 70)
        XCTAssertLessThan(track.observations[0].groundSpeed, 80)
        XCTAssertEqual(track.observations[0].track, 90, accuracy: 0.01)
        XCTAssertEqual(track.observations[0].altitudeMSL, 1000)
        XCTAssertNil(track.observations[0].indicatedAirspeed)
        XCTAssertFalse(track.observations[0].hasAttitude)
        XCTAssertNil(track.observations[0].altitudeGNSS)
    }

    func testEllipsoidRequiresGeoidAndConversionSurvivesJSONRoundTrip() throws {
        XCTAssertThrowsError(try FlightImport.decode(gpx(), name: "x.gpx", elevation: .ellipsoid))
        let track = try FlightImport.decode(gpx(geoid: true), name: "x.gpx", elevation: .ellipsoid)
        XCTAssertEqual(track.observations[0].altitudeMSL, 952)
        XCTAssertEqual(track.observations[0].altitudeGNSS, 1000)
        let restored = try FlightImport.decode(JSONEncoder().encode(track), name: "x.flighttrack")
        XCTAssertEqual(restored.observations[0].geoidSeparation, 48)
    }

    func testSegmentsCannotBecomeInterpolatedFlightsOrRouteConnections() throws {
        let track = try FlightImport.decode(gpx(segments: 2), name: "x.gpx")
        XCTAssertNil(track.sample(at: 2.5))
        XCTAssertNotNil(track.sample(at: 3))
        let route = FlightRoute(track: track, limit: 2)
        XCTAssertEqual(route.segments.count, 2)
        XCTAssertEqual(route.segments[0].end.longitude, track.observations[2].longitude)
        XCTAssertEqual(route.segments[1].start.longitude, track.observations[3].longitude)
    }

    func testKMLRequiresTimestampsAndAbsoluteAltitude() throws {
        let xml = """
        <kml xmlns='http://www.opengis.net/kml/2.2' xmlns:gx='http://www.google.com/kml/ext/2.2'>
        <Placemark><gx:Track><altitudeMode>absolute</altitudeMode>
        <when>2026-09-01T12:00:00Z</when><when>2026-09-01T12:00:01Z</when>
        <gx:coord>11.3 47.26 1000</gx:coord><gx:coord>11.301 47.26 1001</gx:coord>
        </gx:Track></Placemark></kml>
        """
        XCTAssertEqual(try FlightImport.decode(Data(xml.utf8), name: "flight.kml").observations.count, 2)
        let renamed = xml.replacingOccurrences(of: "gx:", with: "track:").replacingOccurrences(of: "xmlns:gx", with: "xmlns:track")
        XCTAssertEqual(try FlightImport.decode(Data(renamed.utf8), name: "renamed.kml").observations.count, 2)
        XCTAssertThrowsError(try FlightImport.decode(Data(xml.replacingOccurrences(of: "absolute", with: "relativeToGround").utf8), name: "x.kml"))
        XCTAssertThrowsError(try FlightImport.decode(Data(xml.replacingOccurrences(of: "<when>2026-09-01T12:00:01Z</when>", with: "").utf8), name: "x.kml"))
    }

    func testMalformedAndUnboundedImportsAreRejected() throws {
        XCTAssertThrowsError(try FlightImport.decode(Data(repeating: 0, count: FlightImport.byteLimit + 1), name: "x.gpx"))
        XCTAssertThrowsError(try FlightImport.decode(Data("<!DOCTYPE gpx [<!ENTITY source SYSTEM 'file:///etc/passwd'>]><gpx>&source;</gpx>".utf8), name: "x.gpx"))
        XCTAssertThrowsError(try FlightImport.decode(Data("<gpx><trk><trkseg><trkpt lat='0' lon='0'/></trkseg></trk></gpx>".utf8), name: "x.gpx"))
        let xml = try XCTUnwrap(String(data: gpx(), encoding: .utf8))
        XCTAssertThrowsError(try FlightImport.decode(Data(xml.replacingOccurrences(of: "12:00:01Z", with: "12:00:00Z").utf8), name: "x.gpx"))
        XCTAssertThrowsError(try FlightImport.decode(Data(xml.replacingOccurrences(of: "lat='47.26'", with: "lat='NaN'").utf8), name: "x.gpx"))
    }

    func testLibraryPersistsImportsWithoutChangingTheSourceAndSeparatesDatums() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let library = FlightLibrary(directory: directory)
        let data = gpx(geoid: true)
        let (entry, first) = try await library.importTrack(data, name: "Flight.gpx", elevation: .msl)
        let (duplicate, _) = try await library.importTrack(data, name: "Flight.gpx", elevation: .msl)
        XCTAssertEqual(entry, duplicate)
        let loaded = try await library.load(entry)
        XCTAssertEqual(loaded.source.sha256, first.source.sha256)
        let (corrected, _) = try await library.importTrack(data, name: "Flight.gpx", elevation: .ellipsoid)
        XCTAssertNotEqual(corrected.id, entry.id)
        let correctedTrack = try await library.load(corrected)
        XCTAssertEqual(correctedTrack.observations[0].altitudeMSL, 952)
        try await library.remove(entry)
        let entries = try await library.entries()
        XCTAssertFalse(entries.contains(entry))
        XCTAssertTrue(entries.contains(corrected))
    }
}
