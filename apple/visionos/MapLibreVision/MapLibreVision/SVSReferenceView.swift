import PDFKit
import SwiftUI

struct SVSReferenceView: View {
    @Environment(\.dismiss) private var dismiss
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text("FAA AC 20-185A").font(.headline)
                Spacer()
                Button("Done") { dismiss() }
            }
            Text("Display design reference. This replay is not an approved SVS or flight instrument.")
                .font(.caption).foregroundStyle(.secondary)
            ReferencePDF()
        }.padding(20).frame(width: 660, height: 760)
    }
}

private struct ReferencePDF: UIViewRepresentable {
    func makeUIView(context: Context) -> PDFView {
        let view = PDFView()
        view.autoScales = true
        if let url = Bundle.main.url(forResource: "AC_20-185A", withExtension: "pdf") {
            view.document = PDFDocument(url: url)
            if let page = view.document?.page(at: 9) { view.go(to: page) }
        }
        return view
    }
    func updateUIView(_ uiView: PDFView, context: Context) {}
}
