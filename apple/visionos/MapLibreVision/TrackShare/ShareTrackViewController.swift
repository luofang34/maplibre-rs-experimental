import UIKit
import UniformTypeIdentifiers

final class ShareTrackViewController: UIViewController {
    private let message = UILabel()
    private let done = UIButton(type: .system)

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .systemBackground
        let title = UILabel()
        title.text = "MapLibre Vision"
        title.font = .preferredFont(forTextStyle: .title2)
        message.text = "Receiving flight track…"
        message.numberOfLines = 0
        done.setTitle("Done", for: .normal)
        done.addTarget(self, action: #selector(finish), for: .touchUpInside)
        let stack = UIStackView(arrangedSubviews: [title, message, done])
        stack.axis = .vertical
        stack.spacing = 24
        stack.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.leadingAnchor, constant: 28),
            stack.trailingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.trailingAnchor, constant: -28),
            stack.centerYAnchor.constraint(equalTo: view.centerYAnchor)])
        receive()
    }

    private func receive() {
        guard let item = extensionContext?.inputItems.first as? NSExtensionItem,
              let provider = item.attachments?.first else { message.text = "No track file was shared."; return }
        let types = ["com.google.earth.kml", "org.topografix.gpx", "com.sokolysystems.flight-track", UTType.json.identifier, UTType.data.identifier]
        guard let type = types.first(where: provider.hasItemConformingToTypeIdentifier) else {
            message.text = "Share a GPX, KML, or flight JSON file from Files or another app."; return
        }
        done.isEnabled = false
        provider.loadFileRepresentation(forTypeIdentifier: type) { [weak self] url, error in
            let result: String
            do {
                guard let url else { throw error ?? FlightInbox.Failure.unsupported }
                // The provider owns the temporary URL only until this callback returns.
                let name = try FlightInbox.fileName(source: url, suggested: provider.suggestedName, contentType: type)
                try FlightInbox().save(url, name: name)
                result = "Track saved. Open MapLibre Vision to name and review it before adding it to your library."
            } catch { result = error.localizedDescription }
            DispatchQueue.main.async { self?.message.text = result; self?.done.isEnabled = true }
        }
    }

    @objc private func finish() { extensionContext?.completeRequest(returningItems: nil) }
}
