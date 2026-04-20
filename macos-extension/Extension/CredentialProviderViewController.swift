import AuthenticationServices
import SwiftUI
import os

private let log = Logger(subsystem: "com.nixpulvis.valet.autofill", category: "extension")

final class CredentialProviderViewController: ASCredentialProviderViewController {
    override func prepareCredentialList(
        for serviceIdentifiers: [ASCredentialServiceIdentifier]
    ) {
        presentList(queries: serviceIdentifiers.map { queryFor($0.identifier) })
    }

    override func provideCredentialWithoutUserInteraction(
        for credentialIdentity: ASPasswordCredentialIdentity
    ) {
        let recordID = credentialIdentity.recordIdentifier ?? ""
        Task { [weak self] in
            guard let self = self else { return }
            do {
                let client = try ValetClient.default()
                let record = try await client.fetch(uuid: recordID)
                self.extensionContext.completeRequest(
                    withSelectedCredential: ASPasswordCredential(
                        user: record.extras["username"] ?? "",
                        password: record.password
                    )
                )
            } catch {
                log.error("provideCredential failed: \(String(describing: error), privacy: .public)")
                self.fallbackToUI()
            }
        }
    }

    override func prepareInterfaceToProvideCredential(
        for credentialIdentity: ASPasswordCredentialIdentity
    ) {
        presentList(queries: [queryFor(credentialIdentity.serviceIdentifier.identifier)])
    }

    private func presentList(queries: [String]) {
        do {
            let client = try ValetClient.default()
            let view = CredentialListView(
                client: client,
                queries: queries,
                onSelect: { [weak self] record in self?.complete(with: record) },
                onCancel: { [weak self] in self?.cancel() }
            )
            let hosting = NSHostingController(rootView: view)
            addChild(hosting)
            hosting.view.translatesAutoresizingMaskIntoConstraints = false
            self.view.addSubview(hosting.view)
            NSLayoutConstraint.activate([
                hosting.view.leadingAnchor.constraint(equalTo: self.view.leadingAnchor),
                hosting.view.trailingAnchor.constraint(equalTo: self.view.trailingAnchor),
                hosting.view.topAnchor.constraint(equalTo: self.view.topAnchor),
                hosting.view.bottomAnchor.constraint(equalTo: self.view.bottomAnchor),
            ])
            // Without this, macOS presents the popover at 1x0 and nothing is visible.
            preferredContentSize = NSSize(width: 400, height: 320)
        } catch {
            log.error("presentList failed: \(String(describing: error), privacy: .public)")
            extensionContext.cancelRequest(
                withError: NSError(
                    domain: ASExtensionErrorDomain,
                    code: ASExtensionError.failed.rawValue
                )
            )
        }
    }

    private func complete(with entry: RecordIndexEntry) {
        Task { [weak self] in
            guard let self = self else { return }
            do {
                let record = try await ValetClient.default().fetch(uuid: entry.id)
                self.extensionContext.completeRequest(
                    withSelectedCredential: ASPasswordCredential(
                        user: record.extras["username"] ?? "",
                        password: record.password
                    )
                )
            } catch {
                log.error("fetch failed: \(String(describing: error), privacy: .public)")
                self.fallbackToUI()
            }
        }
    }

    /// Turn an `ASCredentialServiceIdentifier.identifier` (a bare host like
    /// `github.com` or sometimes a full URL) into a broad Valet query that
    /// matches that host in any lot. Regex metacharacters in the host pass
    /// through unescaped on purpose; a `.` matching any char over-matches but
    /// is fine for autofill suggestions.
    ///
    /// TODO: add lot support (target a specific lot instead of `~::`), and
    /// drop the regex in favor of an explicit match against
    /// `extra["url"]` or the domain id of `label.name()`.
    private func queryFor(_ identifier: String) -> String {
        let host = URL(string: identifier)?.host ?? identifier
        return "~::~\(host)"
    }

    private func cancel() {
        extensionContext.cancelRequest(
            withError: NSError(
                domain: ASExtensionErrorDomain,
                code: ASExtensionError.userCanceled.rawValue
            )
        )
    }

    private func fallbackToUI() {
        extensionContext.cancelRequest(
            withError: NSError(
                domain: ASExtensionErrorDomain,
                code: ASExtensionError.userInteractionRequired.rawValue
            )
        )
    }
}
