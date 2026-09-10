import RealityKit
import SwiftUI

struct DeskGlobeView: View {
    @EnvironmentObject private var session: GlobeSession
    @Environment(\.scenePhase) private var scenePhase
    @Environment(\.openImmersiveSpace) private var openImmersiveSpace
    @Environment(\.openWindow) private var openWindow
    @State private var subscriptions: [EventSubscription] = []
    @State private var error = ""

    var body: some View {
        RealityView { content, attachments in
            do {
                let globe = try await makeGlobe()
                globe.position = [0, 0.065, 0]
                let root = Entity()
                root.isEnabled = !session.immersed
                root.addChild(globe)
                content.add(root)
                let stand = ModelEntity(mesh: .generateCylinder(height: 0.018, radius: 0.09),
                    materials: [SimpleMaterial(color: .gray, roughness: 0.4, isMetallic: true)])
                stand.position = [0, -0.316, 0]
                root.addChild(stand)
                let stem = ModelEntity(mesh: .generateCylinder(height: 1, radius: 0.008),
                    materials: [SimpleMaterial(color: .gray, roughness: 0.4, isMetallic: true)])
                stem.name = "stem"
                root.addChild(stem)
                bound(globe)
                if let controls = attachments.entity(for: "controls") {
                    controls.position = [0, -0.18, 0.265]
                    root.addChild(controls)
                }
                subscriptions = [
                    content.subscribe(to: ManipulationEvents.DidUpdateTransform.self, on: globe) { _ in
                        bound(globe)
                    },
                    content.subscribe(to: ManipulationEvents.WillEnd.self, on: globe) { _ in
                        bound(globe)
                        session.record(rotation: globe.orientation, radius: globe.scale.x)
                    },
                    content.subscribe(to: SceneEvents.Update.self) { _ in
                        if !session.immersed { updateAircraft(on: globe) }
                    }
                ]
            } catch { self.error = "The desk globe could not load: \(error.localizedDescription)" }
        } update: { content, _ in
            for entity in content.entities { entity.isEnabled = !session.immersed }
        } attachments: {
            Attachment(id: "controls") { controls }
        }
        .persistentSystemOverlays(session.immersed ? .hidden : .automatic)
        .overlay { if !error.isEmpty { Text(error).padding().glassBackgroundEffect() } }
        .onChange(of: scenePhase) { _, phase in
            if phase == .background, !session.immersed { session.replay.pause() }
            if phase != .active { session.save() }
        }
        .onDisappear { subscriptions.removeAll(); session.save() }
        .modifier(FlightImportPresentation(immersiveControls: false))
        .task {
            await session.loadLibrary()
            if let index = ProcessInfo.processInfo.arguments.firstIndex(of: "--import-track"),
               index + 1 < ProcessInfo.processInfo.arguments.count {
                await session.receive(URL(fileURLWithPath: ProcessInfo.processInfo.arguments[index + 1]))
            }
            if ProcessInfo.processInfo.arguments.contains("--replay") { await enterTerrain(follow: true) }
            else if ProcessInfo.processInfo.arguments.contains("--mode") {
                await enterTerrain(follow: false, height: MapModeStore.shared.initialHeight)
            }
        }
    }

    private var controls: some View {
        VStack(alignment: .leading, spacing: 12) {
            ReplayControls()
            HStack {
                Button(session.selectedTrackID == "innsbruck-approach" ? "Fly approach" : "Fly track", systemImage: "airplane") { Task { await enterTerrain(follow: true) } }
                    .disabled(session.opening || session.replay.track == nil)
                Button("Explore terrain", systemImage: "mountain.2") { Task { await enterTerrain(follow: false) } }
                    .disabled(session.opening)
            }
            Text("Turn the globe with a pinch. Spread to resize. Use the window handle to place it; snap and lock it to keep it on your desk.")
                .font(.caption).foregroundStyle(.secondary)
            if !session.status.isEmpty { Text(session.status).font(.caption).foregroundStyle(.red) }
        }.padding(18).frame(width: 440).glassBackgroundEffect()
    }

    private func makeGlobe() async throws -> ModelEntity {
        let geometry = GlobeGeometry.sphere()
        var descriptor = MeshDescriptor(name: "Earth")
        descriptor.positions = .init(geometry.positions)
        descriptor.normals = .init(geometry.normals)
        descriptor.textureCoordinates = .init(geometry.uv)
        descriptor.primitives = .triangles(geometry.indices)
        let texture = try await TextureResource(named: "desk-globe.png")
        var material = UnlitMaterial()
        material.color = .init(texture: .init(texture))
        let globe = ModelEntity(mesh: try MeshResource.generate(from: [descriptor]), materials: [material])
        globe.name = "Desk Earth"
        globe.scale = .init(repeating: session.desk.boundedRadius)
        globe.orientation = session.desk.rotation
        ManipulationComponent.configureEntity(globe, collisionShapes: [.generateSphere(radius: 1)])
        var manipulation = ManipulationComponent()
        manipulation.releaseBehavior = .stay
        manipulation.dynamics.translationBehavior = .none
        manipulation.dynamics.scalingBehavior = .unconstrained
        manipulation.dynamics.inertia = .zero
        globe.components.set(manipulation)
        let airport = ModelEntity(mesh: .generateSphere(radius: 0.012), materials: [UnlitMaterial(color: .cyan)])
        airport.name = "LOWI"
        airport.position = SIMD3<Float>(GlobeGeometry.direction(.init(latitude: 47.2602, longitude: 11.3439, altitudeMeters: 581))) * 1.005
        globe.addChild(airport)
        let aircraft = ModelEntity(mesh: .generateSphere(radius: 0.009), materials: [UnlitMaterial(color: .orange)])
        aircraft.name = "aircraft"
        globe.addChild(aircraft)
        updateAircraft(on: globe)
        return globe
    }

    private func bound(_ globe: Entity) {
        let radius = globe.scale.x.isFinite ? min(max(globe.scale.x, 0.10), 0.22) : 0.17
        globe.scale = .init(repeating: radius)
        globe.position = [0, 0.065, 0]
        if let stem = globe.parent?.findEntity(named: "stem") {
            let top = globe.position.y - radius, bottom: Float = -0.307
            stem.scale.y = top - bottom
            stem.position = [0, (top + bottom) / 2, 0]
        }
    }

    private func updateAircraft(on globe: Entity) {
        guard let aircraft = globe.findEntity(named: "aircraft") else { return }
        let sample = session.replay.frame().observation
        aircraft.isEnabled = sample != nil
        if let sample {
            let radius = Float(1 + sample.altitudeMSL / MapPlacement.earthRadiusMeters)
            aircraft.position = SIMD3<Float>(GlobeGeometry.direction(sample.coordinate)) * max(radius, 1.018)
        }
    }

    private func enterTerrain(follow: Bool, height: Double = MapPlacement.flyToHeight) async {
        guard !session.opening else { return }
        session.replay.follow(follow)
        if session.immersed { openWindow(id: GlobeSession.controlsID, value: GlobeSession.controlsID); return }
        session.opening = true
        defer { session.opening = false }
        session.status = ""
        let center = GlobeGeometry.coordinate(SIMD3<Double>(session.desk.rotation.inverse.act(SIMD3<Float>(0, 0, 1))))
        MapModeStore.shared.enter(at: follow ? session.replay.frame().observation?.coordinate ?? .innsbruck : center, height: height)
        switch await openImmersiveSpace(id: MapRenderer.spaceID) {
        case .opened:
            session.immersed = true
            openWindow(id: GlobeSession.controlsID, value: GlobeSession.controlsID)
            if follow, !session.replay.frame().playing { session.replay.toggle() }
        case .userCancelled: session.replay.follow(false)
        case .error: session.status = "The terrain view could not open. Please try again."
        @unknown default: session.status = "The terrain view could not open. Please try again."
        }
    }
}
