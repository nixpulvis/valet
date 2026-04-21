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

/// Rebuild `ASCredentialIdentityStore` on app launch. Aggressive: any
/// failure (daemon unreachable, daemon locked, list error) clears the
/// store rather than leaving stale entries from a previous install
/// behind.
private func syncNow() async -> Int {
    do {
        let client = try ValetClient.default()
        guard let username = try await client.status().first else {
            log.notice("daemon locked; clearing identity store")
            try await ASCredentialIdentityStore.shared.replaceCredentialIdentities([])
            return 0
        }
        return try await syncIdentities(client: client, username: username)
    } catch {
        log.error("sync failed, clearing identity store: \(String(describing: error), privacy: .public)")
        try? await ASCredentialIdentityStore.shared.replaceCredentialIdentities([])
        return 0
    }
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
        _ = await syncNow()
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
        _ = await syncNow()
    } else {
        log.notice("timed out waiting for provider to be enabled")
    }
    DispatchQueue.main.async {
        if enabled { NSApp.stopModal() } else { NSApp.abortModal() }
    }
}

NSApp.activate(ignoringOtherApps: true)
_ = alert.runModal()
