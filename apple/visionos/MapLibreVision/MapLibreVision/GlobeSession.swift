import SwiftUI
import simd

@MainActor
final class GlobeSession: ObservableObject {
    static let homeID = "desk-globe"
    static let controlsID = "map-controls"
    private static let storageKey = "desk-globe-state"

    @Published var immersed = false
    @Published var opening = false
    @Published var status = ""
    var desk: DeskGlobeState
    let replay: FlightReplay

    init() {
        let restored = UserDefaults.standard.data(forKey: Self.storageKey)
            .flatMap { try? JSONDecoder().decode(DeskGlobeState.self, from: $0) } ?? .init()
        desk = restored
        replay = FlightReplay(restored: restored)
    }

    func save() {
        let frame = replay.frame()
        desk.playbackTime = frame.elapsed
        desk.playbackRate = frame.rate
        do { UserDefaults.standard.set(try JSONEncoder().encode(desk), forKey: Self.storageKey) }
        catch { status = "Could not save the globe state: \(error.localizedDescription)" }
    }

    func record(rotation: simd_quatf, radius: Float) {
        desk.record(rotation: rotation, radius: radius)
        save()
    }
}
