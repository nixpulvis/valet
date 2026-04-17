import AppKit

// Minimal host for ValetAutoFillExt.appex. macOS requires the .appex to live
// inside a .app so pluginkit discovers it; this binary's only job is to be
// that container. When valet-gui ships a real macOS .app bundle, the .appex
// moves into its Contents/PlugIns/ and this placeholder is deleted.
//
// Identity-store seeding lives in the extension (see
// CredentialProviderViewController.swift:syncIdentityStore), following the
// pattern used by Pass for iOS — writes happen from the extension's sandbox,
// not the host app's.

let app = NSApplication.shared
app.setActivationPolicy(.regular)

let alert = NSAlert()
alert.messageText = "ValetAutoFill installed"
alert.informativeText =
    "Enable Valet in System Settings → General → AutoFill & Passwords to use it for password AutoFill. Trigger AutoFill in Safari once (via the key icon → ValetAutoFill…) to seed Safari's inline suggestion list."
alert.addButton(withTitle: "OK")
_ = alert.runModal()
