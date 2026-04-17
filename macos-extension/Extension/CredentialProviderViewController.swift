import AuthenticationServices
import SwiftUI
import os

private let log = Logger(subsystem: "com.nixpulvis.valet.autofill", category: "extension")

final class CredentialProviderViewController: ASCredentialProviderViewController {
    private var client: ValetClient?
    private var hosting: NSHostingController<CredentialListView>?

    override func loadView() {
        log.notice("loadView")
        super.loadView()
    }

    override func viewDidLoad() {
        log.notice("viewDidLoad bounds=\(NSStringFromRect(self.view.bounds), privacy: .public)")
        super.viewDidLoad()
    }

    override func viewWillAppear() {
        log.notice("viewWillAppear bounds=\(NSStringFromRect(self.view.bounds), privacy: .public)")
        super.viewWillAppear()
    }

    override func prepareCredentialList(
        for serviceIdentifiers: [ASCredentialServiceIdentifier]
    ) {
        let ids = serviceIdentifiers.map(\.identifier)
        log.notice("prepareCredentialList count=\(ids.count) ids=\(ids, privacy: .public)")
        // Pass-for-iOS pattern: seed ASCredentialIdentityStore with identities
        // keyed by Safari's OWN serviceIdentifiers. Inline matching is
        // equality-based on identifier, so saving verbatim guarantees a match
        // on the next visit to the same URL.
        Task { [weak self] in
            await self?.syncIdentityStore(serviceIdentifiers: serviceIdentifiers)
        }
        presentList(serviceIdentifiers: ids)
    }

    private func syncIdentityStore(serviceIdentifiers: [ASCredentialServiceIdentifier]) async {
        guard !serviceIdentifiers.isEmpty else { return }
        let store = ASCredentialIdentityStore.shared
        let state = await store.state()
        guard state.isEnabled else {
            log.notice("syncIdentityStore: store disabled, skipping")
            return
        }
        do {
            let client = try makeClient()
            let records = try await client.list(serviceIdentifiers: [])
            var identities: [ASPasswordCredentialIdentity] = []
            for identifier in serviceIdentifiers {
                for record in records {
                    identities.append(ASPasswordCredentialIdentity(
                        serviceIdentifier: identifier,
                        user: record.username,
                        recordIdentifier: record.label
                    ))
                }
            }
            try await store.saveCredentialIdentities(identities)
            log.notice("syncIdentityStore: saved \(identities.count) identities keyed by Safari's own serviceIdentifiers")
        } catch {
            log.error("syncIdentityStore failed: \(String(describing: error), privacy: .public)")
        }
    }

    override func provideCredentialWithoutUserInteraction(
        for credentialIdentity: ASPasswordCredentialIdentity
    ) {
        let recordID = credentialIdentity.recordIdentifier ?? ""
        log.notice("provideCredentialWithoutUserInteraction recordID=\(recordID, privacy: .public) user=\(credentialIdentity.user, privacy: .public)")
        // In the real daemon flow this path only succeeds when the vault is
        // already unlocked; a locked vault should fall through to the list UI
        // via .userInteractionRequired. The stub is always unlocked, so we
        // return the matching record's password directly.
        Task { [weak self] in
            guard let self = self else { return }
            do {
                let client = try self.makeClient()
                let records = try await client.list(serviceIdentifiers: [])
                guard let record = records.first(where: { $0.label == recordID }) else {
                    log.notice("no record matched recordID=\(recordID, privacy: .public) — falling back to UI")
                    self.extensionContext.cancelRequest(
                        withError: NSError(
                            domain: ASExtensionErrorDomain,
                            code: ASExtensionError.userInteractionRequired.rawValue
                        )
                    )
                    return
                }
                log.notice("completing silently label=\(record.label, privacy: .public) user=\(record.username, privacy: .public)")
                self.extensionContext.completeRequest(
                    withSelectedCredential: ASPasswordCredential(
                        user: record.username,
                        password: record.password
                    ),
                    completionHandler: { expired in
                        log.notice("silent completeRequest returned expired=\(expired, privacy: .public)")
                    }
                )
            } catch {
                log.error("provideCredential failed: \(String(describing: error), privacy: .public)")
                self.extensionContext.cancelRequest(
                    withError: NSError(
                        domain: ASExtensionErrorDomain,
                        code: ASExtensionError.userInteractionRequired.rawValue
                    )
                )
            }
        }
    }

    override func prepareInterfaceToProvideCredential(
        for credentialIdentity: ASPasswordCredentialIdentity
    ) {
        log.notice("prepareInterfaceToProvideCredential id=\(credentialIdentity.serviceIdentifier.identifier, privacy: .public)")
        presentList(serviceIdentifiers: [credentialIdentity.serviceIdentifier.identifier])
    }

    override func prepareInterfaceForExtensionConfiguration() {
        log.notice("prepareInterfaceForExtensionConfiguration")
        presentList(serviceIdentifiers: [])
    }

    private func presentList(serviceIdentifiers: [String]) {
        do {
            let client = try makeClient()
            self.client = client
            let view = CredentialListView(
                client: client,
                serviceIdentifiers: serviceIdentifiers,
                onSelect: { [weak self] record in
                    self?.complete(with: record)
                },
                onCancel: { [weak self] in
                    self?.cancel()
                }
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
            // The AutoFill popover sizes itself from preferredContentSize.
            // Without this, macOS presents us at 1×0 and nothing is visible.
            preferredContentSize = NSSize(width: 400, height: 320)
            self.hosting = hosting
        } catch {
            cancel(with: error)
        }
    }

    private func makeClient() throws -> ValetClient {
        #if VALET_IPC_STUB
        return try ValetClient.stub()
        #else
        // TODO(valet-daemon): hardcoded path until the daemon ships a real
        // launchd-managed socket and the extension gets a temporary-exception
        // entitlement for it.
        let path = NSString(string: "~/.local/share/valet/valet.sock")
            .expandingTildeInPath
        return try ValetClient.connect(socketPath: path)
        #endif
    }

    private func complete(with record: RecordView) {
        log.notice("complete: label=\(record.label, privacy: .public) user=\(record.username, privacy: .public)")
        extensionContext.completeRequest(
            withSelectedCredential: ASPasswordCredential(
                user: record.username,
                password: record.password
            ),
            completionHandler: { expired in
                log.notice("completeRequest returned expired=\(expired, privacy: .public)")
            }
        )
    }

    private func cancel(with error: Error? = nil) {
        if let error = error {
            log.notice("cancel: \(String(describing: error), privacy: .public)")
        } else {
            log.notice("cancel: user cancelled")
        }
        extensionContext.cancelRequest(
            withError: NSError(
                domain: ASExtensionErrorDomain,
                code: ASExtensionError.failed.rawValue
            )
        )
    }
}
