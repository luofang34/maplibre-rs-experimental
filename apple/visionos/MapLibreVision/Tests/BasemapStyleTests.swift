import XCTest

final class BasemapStyleTests: XCTestCase {
    func testGroundClassesAndTransportSurfacesKeepDistinctConsistentColors() throws {
        let url = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("MapLibreVision/style.json")
        let style = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any])
        let layers = try XCTUnwrap(style["layers"] as? [[String: Any]])
        func paint(_ id: String, _ property: String) throws -> String {
            let layer = try XCTUnwrap(layers.first { $0["id"] as? String == id })
            return try XCTUnwrap((layer["paint"] as? [String: Any])?[property] as? String)
        }
        let water = try paint("water", "fill-color")
        for id in ["landcover_grass", "landcover_farmland", "landcover_wood", "landcover_rock", "building"] {
            XCTAssertNotEqual(try paint(id, "fill-color"), water)
        }
        for layer in 0...5 {
            XCTAssertEqual(try paint("bridge_\(layer)_highway_major_inner", "line-color"),
                           try paint("highway_major_inner", "line-color"))
        }
        XCTAssertEqual((style["terrain"] as? [String: Any])?["exaggeration"] as? Int, 1)
    }
}
