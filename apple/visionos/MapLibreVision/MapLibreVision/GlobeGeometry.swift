import simd

enum GlobeGeometry {
    static func direction(_ coordinate: MapAnchor) -> SIMD3<Double> {
        let latitude = coordinate.latitude * .pi / 180
        let longitude = coordinate.longitude * .pi / 180
        return SIMD3(cos(latitude) * sin(longitude), sin(latitude), cos(latitude) * cos(longitude))
    }

    static func coordinate(_ direction: SIMD3<Double>) -> MapAnchor {
        let point = simd_normalize(direction)
        return .init(latitude: asin(min(max(point.y, -1), 1)) * 180 / .pi,
                     longitude: atan2(point.x, point.z) * 180 / .pi, altitudeMeters: 0)
    }

    struct Mesh {
        var positions: [SIMD3<Float>]
        var normals: [SIMD3<Float>]
        var uv: [SIMD2<Float>]
        var indices: [UInt32]
    }

    static func sphere(columns: Int = 192, rows: Int = 128) -> Mesh {
        var mesh = Mesh(positions: [], normals: [], uv: [], indices: [])
        for row in 0...rows {
            let latitude = 90 - Double(row) / Double(rows) * 180
            let mercator = min(max(latitude, -85.05112878), 85.05112878) * .pi / 180
            let v = (1 - log(tan(.pi / 4 + mercator / 2)) / .pi) / 2
            for column in 0...columns {
                let u = Double(column) / Double(columns)
                let point = SIMD3<Float>(direction(.init(latitude: latitude, longitude: u * 360 - 180, altitudeMeters: 0)))
                mesh.positions.append(point)
                mesh.normals.append(point)
                mesh.uv.append(SIMD2(Float(u), Float(1 - v)))
            }
        }
        for row in 0..<rows {
            for column in 0..<columns {
                let a = UInt32(row * (columns + 1) + column), b = a + UInt32(columns + 1)
                if row > 0 { mesh.indices += [a, b, a + 1] }
                if row < rows - 1 { mesh.indices += [a + 1, b, b + 1] }
            }
        }
        return mesh
    }
}
