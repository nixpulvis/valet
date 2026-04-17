import SwiftUI

struct CredentialListView: View {
    let client: ValetClient
    let queries: [String]
    let onSelect: (RecordIndexEntry) -> Void
    let onCancel: () -> Void

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
                    if let username = record.extras["username"], !username.isEmpty {
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
        defer { loading = false }
        do {
            records = try await client.list(queries: queries)
        } catch {
            self.error = "\(error)"
        }
    }
}
