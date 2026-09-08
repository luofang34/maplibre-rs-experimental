import Foundation
import simd

struct MapSelection: Decodable {
    var text: String
    var source_layer: String
    var coordinates: [Double]

    var title: String { text.isEmpty ? source_layer.replacingOccurrences(of: "_", with: " ") : text }
}

extension MapRenderer {
    func selectLabel(_ ray: (origin: SIMD3<Double>, direction: SIMD3<Double>),
                     worldFromEye: simd_float4x4, projection: simd_float4x4,
                     map: OpaquePointer) -> MapSelection? {
        let direction = simd_inverse(worldFromEye) * SIMD4<Float>(SIMD3<Float>(ray.direction), 0)
        let clip = projection * direction
        guard clip.w > 0 else { return nil }
        let x = Double((clip.x / clip.w + 1) * 0.5) * Double(maplibre_visionos_width(map))
        let y = Double((1 - clip.y / clip.w) * 0.5) * Double(maplibre_visionos_height(map))
        var bytes = [CChar](repeating: 0, count: 4096)
        var required = maplibre_visionos_query_symbols(map, x, y, &bytes, bytes.count)
        if required > bytes.count, required <= 1_048_576 {
            bytes = [CChar](repeating: 0, count: required)
            required = maplibre_visionos_query_symbols(map, x, y, &bytes, bytes.count)
        }
        guard required > 1, required <= bytes.count else { return nil }
        let data = bytes.withUnsafeBytes { Data($0.prefix(required - 1)) }
        return (try? JSONDecoder().decode([MapSelection].self, from: data))?.first
    }
}
