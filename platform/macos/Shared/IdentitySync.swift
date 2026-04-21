import AuthenticationServices
import Foundation
import os

private let log = Logger(subsystem: "com.nixpulvis.valet.autofill", category: "identity-sync")

/// List every record for `username` in valetd and replace the system
/// `ASCredentialIdentityStore` so Safari's inline AutoFill popup has
/// something to offer. Returns the identity count on success.
///
/// Both the host app (on launch) and the extension (after unlock) drive
/// this so the store tracks daemon state at both moments.
public func syncIdentities(client: ValetClient, username: String) async throws -> Int {
    let entries = try await client.list(username: username, queries: [])
    let identities = entries.compactMap { entry -> ASPasswordCredentialIdentity? in
        guard
            let urlString = entry.extras["url"],
            let host = URL(string: urlString)?.host
        else { return nil }
        return ASPasswordCredentialIdentity(
            serviceIdentifier: ASCredentialServiceIdentifier(identifier: host, type: .domain),
            user: entry.username ?? "",
            recordIdentifier: entry.id
        )
    }
    try await ASCredentialIdentityStore.shared.replaceCredentialIdentities(identities)
    log.notice("replaced \(identities.count) identities for \(username, privacy: .public)")
    return identities.count
}
