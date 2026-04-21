import SwiftUI

struct CredentialListView: View {
    let client: ValetClient
    let username: String
    let domains: [String]
    let onSelect: (RecordIndexEntry) -> Void
    let onCancel: () -> Void
    /// Fires once, after `load()` finishes (success or failure). Lets the
    /// caller schedule work that shouldn't compete with the list's
    /// findRecords RPCs for the client mutex.
    var onLoaded: (() -> Void)? = nil

    @State private var records: [RecordIndexEntry] = []
    @State private var loading = true
    @State private var error: String?

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Spacer()
                Button("Close", action: onCancel)
                    .keyboardShortcut(.cancelAction)
            }
            .padding(8)
            content
        }
        .frame(minWidth: 320, minHeight: 240)
        .task { await load() }
    }

    @ViewBuilder
    private var content: some View {
        if loading {
            ProgressView().padding()
        } else if let error = error {
            Text(error).foregroundColor(.red).padding()
        } else if records.isEmpty {
            Text("No matching credentials").foregroundColor(.secondary).padding()
        } else {
            // A SwiftUI Button inside a List swallows taps when hosted inside
            // ASCredentialProviderViewController; use a row-level tap gesture.
            List(records) { record in
                VStack(alignment: .leading, spacing: 2) {
                    Text(record.label).font(.body)
                    if let username = record.username, !username.isEmpty {
                        Text(username)
                            .font(.caption)
                            .foregroundColor(.secondary)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .contentShape(Rectangle())
                .onTapGesture { onSelect(record) }
            }
        }
    }

    private func load() async {
        defer {
            loading = false
            onLoaded?()
        }
        do {
            // findRecords is per-lot, so we loop over each domain Safari
            // handed us, hit DEFAULT_LOT once per domain, and dedupe by
            // uuid. Mirrors the browser extension's shape.
            //
            // TODO: when the daemon grows a cross-lot suffix query (see
            // the TODO on Request::FindRecords), drop this loop and pass
            // domains directly to a single RPC.
            var seen = Set<String>()
            var merged: [RecordIndexEntry] = []
            for domain in domains {
                let batch = try await client.findRecords(
                    username: username, lot: "main", domain: domain
                )
                for entry in batch where seen.insert(entry.id).inserted {
                    merged.append(entry)
                }
            }
            records = merged
        } catch {
            self.error = "\(error)"
        }
    }
}
