import AppKit
import AuthenticationServices
import os

// Host for ValetAutoFillExt.appex. macOS requires the .appex to live inside a
// .app so pluginkit discovers it. This process also owns the one job only a
// regular app process can do: writing the credential index into
// ASCredentialIdentityStore. Passwords never leave the daemon; the store only
// holds (domain, username, recordIdentifier) tuples that Safari uses to show
// inline suggestions.

private let log = Logger(subsystem: "com.nixpulvis.valet.autofill", category: "host")

private let enableWaitTimeout: TimeInterval = 120
private let pollInterval: TimeInterval = 1

private func syncNow() async throws -> Int {
    let entries = try await ValetClient.default().list(queries: [])
    let identities = entries.compactMap { entry -> ASPasswordCredentialIdentity? in
        guard
            let urlString = entry.extras["url"],
            let host = URL(string: urlString)?.host
        else { return nil }
        return ASPasswordCredentialIdentity(
            serviceIdentifier: ASCredentialServiceIdentifier(identifier: host, type: .domain),
            user: entry.extras["username"] ?? "",
            recordIdentifier: entry.id
        )
    }
    try await ASCredentialIdentityStore.shared.replaceCredentialIdentities(identities)
    log.notice("replaced \(identities.count) identities")
    return identities.count
}

/// Resolves true as soon as the user enables Valet as their AutoFill provider,
/// false on timeout.
private func waitForEnabled(timeout: TimeInterval) async -> Bool {
    let deadline = Date().addingTimeInterval(timeout)
    while Date() < deadline {
        if await ASCredentialIdentityStore.shared.state().isEnabled {
            return true
        }
        try? await Task.sleep(nanoseconds: UInt64(pollInterval * 1_000_000_000))
    }
    return false
}

NSApplication.shared.setActivationPolicy(.regular)

// Fast path: provider already enabled. Runs on a background-actor Task so we
// can block the main thread on the semaphore without deadlocking. The Box
// carries the result across task boundaries; the semaphore provides the
// happens-before edge that makes the read on the main thread safe.
final class Box<T>: @unchecked Sendable { var value: T; init(_ v: T) { value = v } }
let fastPath = DispatchSemaphore(value: 0)
let fastPathEnabled = Box(false)
Task.detached {
    let enabled = await ASCredentialIdentityStore.shared.state().isEnabled
    fastPathEnabled.value = enabled
    if enabled {
        do { _ = try await syncNow() }
        catch { log.error("sync failed: \(String(describing: error), privacy: .public)") }
    }
    fastPath.signal()
}
fastPath.wait()

if fastPathEnabled.value {
    exit(0)
}

// Slow path: alert explains what to do; background Task watches for the user
// flipping the toggle in System Settings and dismisses the alert when done.
let alert = NSAlert()
alert.messageText = "Enable ValetAutoFill"
alert.informativeText = "Open System Settings -> General -> AutoFill & Passwords and turn on Valet. This window will close automatically once suggestions are populated."
alert.addButton(withTitle: "Cancel")

Task.detached {
    let enabled = await waitForEnabled(timeout: enableWaitTimeout)
    if enabled {
        do { _ = try await syncNow() }
        catch { log.error("sync failed: \(String(describing: error), privacy: .public)") }
    } else {
        log.notice("timed out waiting for provider to be enabled")
    }
    DispatchQueue.main.async {
        if enabled { NSApp.stopModal() } else { NSApp.abortModal() }
    }
}

NSApp.activate(ignoringOtherApps: true)
_ = alert.runModal()
