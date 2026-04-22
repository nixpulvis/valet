import AuthenticationServices
import SwiftUI
import os

private let log = Logger(subsystem: "com.nixpulvis.valet.autofill", category: "extension")

final class CredentialProviderViewController: ASCredentialProviderViewController {
    /// The currently-embedded SwiftUI host. We hold a reference so we can
    /// tear down exactly our child when swapping between UnlockView and
    /// CredentialListView, without touching any siblings the system might
    /// add.
    private var hostingChild: NSHostingController<AnyView>?

    override func prepareCredentialList(
        for serviceIdentifiers: [ASCredentialServiceIdentifier]
    ) {
        presentList(domains: serviceIdentifiers.map { domainFor($0.identifier) })
    }

    override func provideCredentialWithoutUserInteraction(
        for credentialIdentity: ASPasswordCredentialIdentity
    ) {
        let recordID = credentialIdentity.recordIdentifier ?? ""
        Task { [weak self] in
            guard let self = self else { return }
            do {
                let client = try ValetClient.default()
                guard let username = try await client.status().first else {
                    self.fallbackToUI()
                    return
                }
                let record = try await client.fetch(username: username, uuid: recordID)
                self.extensionContext.completeRequest(
                    withSelectedCredential: ASPasswordCredential(
                        user: record.username ?? "",
                        password: record.password
                    )
                )
            } catch {
                log.error("provideCredential failed: \(String(describing: error), privacy: .public)")
                self.fallbackToUI()
            }
        }
    }

    /// Safari routes here when `provideCredentialWithoutUserInteraction`
    /// returns `userInteractionRequired` (i.e. the daemon is locked). The
    /// user already picked a specific credential from Safari's inline
    /// popup, so we show the unlock prompt, then fill that exact record
    /// directly. No list view in between.
    override func prepareInterfaceToProvideCredential(
        for credentialIdentity: ASPasswordCredentialIdentity
    ) {
        let recordID = credentialIdentity.recordIdentifier ?? ""
        do {
            let client = try ValetClient.default()
            preferredContentSize = NSSize(width: 400, height: 320)
            Task { [weak self] in
                guard let self = self else { return }
                do {
                    if let username = try await client.status().first {
                        self.completeSpecific(
                            client: client, username: username, recordID: recordID
                        )
                    } else {
                        self.showUnlockForDirectFill(client: client, recordID: recordID)
                    }
                } catch {
                    log.error("status failed: \(String(describing: error), privacy: .public)")
                    self.cancelFailed()
                }
            }
        } catch {
            log.error("prepareInterfaceToProvideCredential failed: \(String(describing: error), privacy: .public)")
            cancelFailed()
        }
    }

    private func showUnlockForDirectFill(client: ValetClient, recordID: String) {
        let view = UnlockView(
            client: client,
            onUnlocked: { [weak self] username in
                self?.completeSpecific(
                    client: client, username: username, recordID: recordID
                )
            },
            onCancel: { [weak self] in self?.cancel() }
        )
        embed(AnyView(view))
    }

    private func completeSpecific(client: ValetClient, username: String, recordID: String) {
        Task { [weak self] in
            guard let self = self else { return }
            do {
                let record = try await client.fetch(username: username, uuid: recordID)
                self.extensionContext.completeRequest(
                    withSelectedCredential: ASPasswordCredential(
                        user: record.username ?? "",
                        password: record.password
                    )
                )
                // Refresh the identity store after the user gets their fill;
                // this is a fresh-unlock moment just like the picker flow.
                Task.detached(priority: .utility) {
                    do { _ = try await syncIdentities(client: client, username: username) }
                    catch { log.error("post-unlock identity sync failed: \(String(describing: error), privacy: .public)") }
                }
            } catch {
                log.error("direct fetch failed: \(String(describing: error), privacy: .public)")
                self.fallbackToUI()
            }
        }
    }

    private func presentList(domains: [String]) {
        do {
            let client = try ValetClient.default()
            // Without this, macOS presents the popover at 1x0 and nothing is visible.
            preferredContentSize = NSSize(width: 400, height: 320)
            Task { [weak self] in
                guard let self = self else { return }
                do {
                    let unlocked = try await client.status()
                    if let username = unlocked.first {
                        self.showList(client: client, username: username, domains: domains)
                    } else {
                        self.showUnlock(client: client, domains: domains)
                    }
                } catch {
                    log.error("status failed: \(String(describing: error), privacy: .public)")
                    self.cancelFailed()
                }
            }
        } catch {
            log.error("presentList failed: \(String(describing: error), privacy: .public)")
            cancelFailed()
        }
    }

    private func showList(
        client: ValetClient,
        username: String,
        domains: [String],
        onLoaded: (() -> Void)? = nil
    ) {
        let view = CredentialListView(
            client: client,
            username: username,
            domains: domains,
            onSelect: { [weak self] record in self?.complete(with: record, username: username) },
            onCancel: { [weak self] in self?.cancel() },
            onLoaded: onLoaded
        )
        embed(AnyView(view))
    }

    private func showUnlock(client: ValetClient, domains: [String]) {
        let view = UnlockView(
            client: client,
            onUnlocked: { [weak self] username in
                // Show the credential list first; once its findRecords
                // calls have resolved, rebuild the ASCredentialIdentityStore
                // so Safari's inline popup reflects the just-unlocked
                // daemon state. Running the sync later keeps the list
                // paint fast because the two paths would otherwise contend
                // for the client mutex.
                self?.showList(client: client, username: username, domains: domains) {
                    Task.detached(priority: .utility) {
                        do { _ = try await syncIdentities(client: client, username: username) }
                        catch { log.error("post-unlock identity sync failed: \(String(describing: error), privacy: .public)") }
                    }
                }
            },
            onCancel: { [weak self] in self?.cancel() }
        )
        embed(AnyView(view))
    }

    private func embed(_ root: AnyView) {
        if let existing = hostingChild {
            existing.view.removeFromSuperview()
            existing.removeFromParent()
            hostingChild = nil
        }
        let hosting = NSHostingController(rootView: root)
        addChild(hosting)
        hosting.view.translatesAutoresizingMaskIntoConstraints = false
        self.view.addSubview(hosting.view)
        NSLayoutConstraint.activate([
            hosting.view.leadingAnchor.constraint(equalTo: self.view.leadingAnchor),
            hosting.view.trailingAnchor.constraint(equalTo: self.view.trailingAnchor),
            hosting.view.topAnchor.constraint(equalTo: self.view.topAnchor),
            hosting.view.bottomAnchor.constraint(equalTo: self.view.bottomAnchor),
        ])
        hostingChild = hosting
    }

    private func complete(with entry: RecordIndexEntry, username: String) {
        Task { [weak self] in
            guard let self = self else { return }
            do {
                let client = try ValetClient.default()
                let record = try await client.fetch(username: username, uuid: entry.id)
                self.extensionContext.completeRequest(
                    withSelectedCredential: ASPasswordCredential(
                        user: record.username ?? "",
                        password: record.password
                    )
                )
            } catch {
                log.error("fetch failed: \(String(describing: error), privacy: .public)")
                self.fallbackToUI()
            }
        }
    }

    /// Normalize an `ASCredentialServiceIdentifier.identifier` (a bare
    /// hostname like `github.com` or sometimes a full URL) into a plain
    /// domain string suitable for `findRecords`.
    private func domainFor(_ identifier: String) -> String {
        URL(string: identifier)?.host ?? identifier
    }

    private func cancel() {
        extensionContext.cancelRequest(
            withError: NSError(
                domain: ASExtensionErrorDomain,
                code: ASExtensionError.userCanceled.rawValue
            )
        )
    }

    private func cancelFailed() {
        extensionContext.cancelRequest(
            withError: NSError(
                domain: ASExtensionErrorDomain,
                code: ASExtensionError.failed.rawValue
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
