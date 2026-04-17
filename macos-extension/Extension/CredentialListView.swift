import SwiftUI
import os

private let log = Logger(subsystem: "com.nixpulvis.valet.autofill", category: "extension")

struct CredentialListView: View {
    let client: ValetClient
    let serviceIdentifiers: [String]
    let onSelect: (RecordView) -> Void
    let onCancel: () -> Void

    @State private var records: [RecordView] = []
    @State private var loading = true
    @State private var error: String?

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            content
        }
        .frame(minWidth: 320, minHeight: 240)
        .task {
            await load()
        }
    }

    private var header: some View {
        HStack {
            Text("Valet")
                .font(.headline)
            Spacer()
            Button("Cancel", action: onCancel)
                .keyboardShortcut(.cancelAction)
        }
        .padding()
    }

    @ViewBuilder
    private var content: some View {
        if loading {
            ProgressView().padding()
        } else if let error = error {
            Text(error)
                .foregroundColor(.red)
                .padding()
        } else if records.isEmpty {
            Text("No matching credentials")
                .foregroundColor(.secondary)
                .padding()
        } else {
            // A SwiftUI Button inside a List swallows taps when hosted inside
            // an ASCredentialProviderViewController — the row's selection gesture
            // wins and the button action never fires, leaving the host app stuck
            // waiting on the AutoFill callback. Use a row-level tap gesture.
            List(records) { record in
                VStack(alignment: .leading, spacing: 2) {
                    Text(record.label)
                        .font(.body)
                    if !record.username.isEmpty {
                        Text(record.username)
                            .font(.caption)
                            .foregroundColor(.secondary)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .contentShape(Rectangle())
                .onTapGesture {
                    log.notice("tap: \(record.label, privacy: .public)")
                    onSelect(record)
                }
            }
        }
    }

    private func load() async {
        defer { loading = false }
        do {
            let result = try await client.list(serviceIdentifiers: serviceIdentifiers)
            log.notice("list loaded: count=\(result.count) ids=\(self.serviceIdentifiers, privacy: .public)")
            records = result
        } catch {
            log.error("list failed: \(String(describing: error), privacy: .public)")
            self.error = "\(error)"
        }
    }
}
