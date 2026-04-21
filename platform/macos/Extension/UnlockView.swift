import SwiftUI

/// Unlock prompt shown when the daemon has no cached user. Lets the user
/// pick a registered username, type the master password, and drives the
/// daemon's `unlock` RPC. On success, calls `onUnlocked(username)` with the
/// chosen user so the caller can continue into the credential list.
///
/// The daemon holds the derived key for `IDLE_TIMEOUT` (5 min) across any
/// process that connects to the socket, so subsequent AutoFill requests in
/// that window skip this screen entirely.
struct UnlockView: View {
    let client: ValetClient
    let onUnlocked: (String) -> Void
    let onCancel: () -> Void

    @State private var users: [String] = []
    @State private var selected: String = ""
    @State private var password: String = ""
    @State private var busy = false
    @State private var loading = true
    @State private var error: String?

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Spacer()
                Button("Cancel", action: onCancel)
                    .keyboardShortcut(.cancelAction)
            }
            .padding(8)

            content
                .padding(.horizontal, 16)
                .padding(.bottom, 16)
        }
        .frame(minWidth: 320, minHeight: 240)
        .task { await loadUsers() }
    }

    @ViewBuilder
    private var content: some View {
        if loading {
            ProgressView()
        } else if users.isEmpty {
            Text("No registered users.").foregroundColor(.secondary)
        } else {
            VStack(alignment: .leading, spacing: 12) {
                Text("Unlock Valet").font(.headline)

                if users.count > 1 {
                    Picker("User", selection: $selected) {
                        ForEach(users, id: \.self) { user in
                            Text(user).tag(user)
                        }
                    }
                } else {
                    HStack {
                        Text("User:").foregroundColor(.secondary)
                        Text(selected)
                    }
                }

                SecureField("Master password", text: $password)
                    .textFieldStyle(.roundedBorder)
                    .onSubmit { Task { await submit() } }

                if let error = error {
                    Text(error).foregroundColor(.red).font(.caption)
                }

                HStack {
                    Spacer()
                    Button("Unlock") {
                        Task { await submit() }
                    }
                    .keyboardShortcut(.defaultAction)
                    .disabled(busy || password.isEmpty || selected.isEmpty)
                }
            }
        }
    }

    private func loadUsers() async {
        defer { loading = false }
        do {
            let all = try await client.listUsers()
            users = all
            if let first = all.first { selected = first }
        } catch {
            self.error = "\(error)"
        }
    }

    private func submit() async {
        guard !busy, !selected.isEmpty, !password.isEmpty else { return }
        busy = true
        error = nil
        defer { busy = false }
        do {
            try await client.unlock(username: selected, password: password)
            password = ""
            onUnlocked(selected)
        } catch {
            self.error = "\(error)"
        }
    }
}
