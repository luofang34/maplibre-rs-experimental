import Foundation
#if canImport(FoundationXML)
import FoundationXML
#endif

final class FlightXML: NSObject, XMLParserDelegate {
    private let format: String
    private var path: [String] = []
    private var text = ""
    private var points: [FlightImport.Point] = []
    private var fields: [String: String] = [:]
    private var segmentStart = true
    private var times: [Double] = []
    private var coordinates: [[Double]] = []
    private var absolute = false
    private var inheritedAbsolute = false
    private var failure: Error?
    private let dates = ISO8601DateFormatter()
    private let fractionalDates = ISO8601DateFormatter()

    private init(format: String) {
        self.format = format
        super.init()
        fractionalDates.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    }

    static func decode(_ data: Data, format: String) throws -> [FlightImport.Point] {
        guard let xml = String(data: data, encoding: .utf8),
              !xml.localizedCaseInsensitiveContains("<!DOCTYPE"), !xml.localizedCaseInsensitiveContains("<!ENTITY") else {
            throw FlightImport.Failure.message("Tracks must be UTF-8 XML without document types or entity declarations.")
        }
        let delegate = FlightXML(format: format)
        let parser = XMLParser(data: data)
        parser.shouldResolveExternalEntities = false
        parser.shouldProcessNamespaces = true
        parser.delegate = delegate
        guard parser.parse(), delegate.failure == nil else {
            throw delegate.failure ?? parser.parserError ?? FlightImport.Failure.message("The track XML is incomplete.")
        }
        return delegate.points
    }

    func parser(_ parser: XMLParser, didStartElement localName: String, namespaceURI: String?,
                qualifiedName qName: String?, attributes: [String: String]) {
        let element = namespaceURI == "http://www.google.com/kml/ext/2.2" ? "gx:\(localName)" : localName
        path.append(element)
        text = ""
        guard path.count <= 48 else { fail("Track XML is nested too deeply.", parser); return }
        if element == "trkseg" { segmentStart = true }
        if element == "trkpt" { fields = attributes }
        if element == "gx:Track" { times = []; coordinates = []; absolute = inheritedAbsolute }
        if element == "gx:MultiTrack" { inheritedAbsolute = false }
    }

    func parser(_ parser: XMLParser, foundCharacters string: String) {
        guard text.utf8.count + string.utf8.count <= 4096 else { fail("A track field exceeds 4 KB.", parser); return }
        text += string
    }

    func parser(_ parser: XMLParser, didEndElement localName: String, namespaceURI: String?, qualifiedName qName: String?) {
        let element = namespaceURI == "http://www.google.com/kml/ext/2.2" ? "gx:\(localName)" : localName
        defer { if !path.isEmpty { path.removeLast() }; text = "" }
        let value = text.trimmingCharacters(in: .whitespacesAndNewlines)
        if format == "gpx" {
            if path.dropLast().last == "trkpt" { fields[element] = value }
            if element == "trkpt", path.suffix(3).elementsEqual(["trk", "trkseg", "trkpt"]) {
                guard let lat = fields["lat"].flatMap(Double.init), let lon = fields["lon"].flatMap(Double.init),
                      let alt = fields["ele"].flatMap(Double.init), let date = fields["time"].flatMap(timestamp) else {
                    fail("Each GPX track point needs latitude, longitude, ele and an ISO-8601 time.", parser); return
                }
                append(.init(date: date, latitude: lat, longitude: lon, altitude: alt,
                             geoid: fields["geoidheight"].flatMap(Double.init), startsSegment: segmentStart), parser)
                segmentStart = false
            }
        } else { endKML(element, value, parser) }
    }

    private func endKML(_ element: String, _ value: String, _ parser: XMLParser) {
        if element == "altitudeMode" {
            if path.contains("gx:Track") { absolute = value == "absolute" }
            else if path.contains("gx:MultiTrack") { inheritedAbsolute = value == "absolute" }
        }
        guard path.contains("gx:Track") else { return }
        if element == "when" {
            guard let time = timestamp(value) else { fail("Invalid KML timestamp.", parser); return }
            times.append(time)
        }
        if element == "gx:coord" {
            let components = value.split(whereSeparator: \.isWhitespace).compactMap { Double($0) }
            guard components.count == 3 else { fail("KML gx:coord needs longitude, latitude and altitude.", parser); return }
            coordinates.append(components)
        }
        guard times.count <= FlightImport.pointLimit, coordinates.count <= FlightImport.pointLimit else {
            fail("A track can contain at most 20,000 positions.", parser); return
        }
        if element == "gx:Track" {
            guard absolute else { fail("KML tracks need altitudeMode absolute; ground-relative heights cannot be treated as MSL.", parser); return }
            guard times.count == coordinates.count else { fail("KML timestamps and coordinates must have matching counts.", parser); return }
            for (i, pair) in zip(times, coordinates).enumerated() {
                append(.init(date: pair.0, latitude: pair.1[1], longitude: pair.1[0], altitude: pair.1[2], startsSegment: i == 0), parser)
            }
        }
    }

    private func timestamp(_ value: String) -> Double? {
        (fractionalDates.date(from: value) ?? dates.date(from: value))?.timeIntervalSince1970
    }

    private func append(_ point: FlightImport.Point, _ parser: XMLParser) {
        guard points.count < FlightImport.pointLimit else { fail("A track can contain at most 20,000 positions.", parser); return }
        points.append(point)
    }

    private func fail(_ message: String, _ parser: XMLParser) {
        failure = FlightImport.Failure.message(message)
        parser.abortParsing()
    }
}
