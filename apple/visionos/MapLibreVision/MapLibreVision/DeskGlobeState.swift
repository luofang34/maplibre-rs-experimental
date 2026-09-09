import Foundation
import simd

struct DeskGlobeState: Codable {
    var orientation: [Float] = []
    var radius: Float = 0.17
    var playbackTime = 0.0
    var playbackRate = 1.0

    var rotation: simd_quatf {
        guard orientation.count == 4, orientation.allSatisfy(\.isFinite) else { return Self.initialRotation }
        let vector = SIMD4<Float>(orientation[0], orientation[1], orientation[2], orientation[3])
        let length = simd_length(vector)
        guard length.isFinite, length > 0.001 else { return Self.initialRotation }
        return simd_normalize(simd_quatf(vector: vector))
    }

    var boundedRadius: Float { radius.isFinite ? min(max(radius, 0.10), 0.22) : 0.17 }

    static var initialRotation: simd_quatf {
        let longitude = simd_quatf(angle: Float(-11.34 * Double.pi / 180), axis: [0, 1, 0])
        let latitude = simd_quatf(angle: Float(47.26 * Double.pi / 180), axis: [1, 0, 0])
        return latitude * longitude
    }

    mutating func record(rotation: simd_quatf, radius: Float) {
        let q = simd_normalize(rotation).vector
        orientation = [q.x, q.y, q.z, q.w]
        self.radius = radius
        self.radius = boundedRadius
    }
}
