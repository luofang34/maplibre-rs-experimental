import Foundation

struct FlightRoute {
    struct Segment {
        let start: MapAnchor
        let end: MapAnchor
        let time: Double
    }
    let segments: [Segment]

    init(track: FlightTrack, limit: Int = 1500) {
        guard limit > 0, track.observations.count > 1 else { segments = []; return }
        let points = track.observations
        let step = max(1, Int(ceil(Double(points.count - 1) / Double(limit))))
        var result: [Segment] = []
        var anchor = 0
        for index in 1..<points.count {
            let gap = points[index].startsSegment == true || points[index].time - points[index - 1].time > 20
            if gap {
                if index - 1 > anchor, result.count < limit {
                    result.append(.init(start: points[anchor].coordinate, end: points[index - 1].coordinate, time: points[index - 1].time))
                }
                anchor = index
            } else if index - anchor >= step || index == points.count - 1 {
                if result.count < limit {
                    result.append(.init(start: points[anchor].coordinate, end: points[index].coordinate, time: points[index].time))
                }
                anchor = index
            }
        }
        segments = result
    }
}
