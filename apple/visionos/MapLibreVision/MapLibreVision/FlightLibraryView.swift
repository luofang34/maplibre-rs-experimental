import SwiftUI

struct FlightLibraryView: View {
    @EnvironmentObject private var session: GlobeSession
    @Environment(\.dismiss) private var dismiss
    @State private var deleting: Set<String> = []
    let importTrack: () -> Void

    var body: some View {
        NavigationStack {
            List {
                ForEach(session.tracks) { entry in
                    HStack {
                        Button {
                            Task { await session.select(entry); dismiss() }
                        } label: {
                            HStack {
                                VStack(alignment: .leading) {
                                    Text(entry.title)
                                    Text(entry.simulated ? "Simulation" : "Recorded flight")
                                        .font(.caption).foregroundStyle(.secondary)
                                }
                                Spacer()
                                if session.selectedTrackID == entry.id {
                                    Image(systemName: "checkmark").accessibilityLabel("Selected")
                                }
                            }.contentShape(Rectangle())
                        }.buttonStyle(.plain)
                        Button(role: .destructive) { remove(entry) } label: {
                            Image(systemName: "trash")
                        }
                        .buttonStyle(.borderless)
                        .accessibilityLabel("Delete \(entry.title)")
                    }
                    .disabled(deleting.contains(entry.id))
                    .contextMenu { Button("Delete", role: .destructive) { remove(entry) } }
                    .swipeActions { Button("Delete", role: .destructive) { remove(entry) } }
                }
                if session.tracks.isEmpty {
                    ContentUnavailableView("No flights", systemImage: "airplane",
                                           description: Text("Import a recording or restore the demo flights."))
                }
                Section {
                    Button("Import track…", systemImage: "square.and.arrow.down", action: importTrack)
                    Button("Restore demo flights") { Task { await session.restoreDemos() } }
                }
            }
            .navigationTitle("Flight library")
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } } }
        }.frame(width: 460, height: 520)
    }

    private func remove(_ entry: FlightLibrary.Entry) {
        guard deleting.insert(entry.id).inserted else { return }
        Task {
            await session.remove(entry)
            deleting.remove(entry.id)
        }
    }
}
