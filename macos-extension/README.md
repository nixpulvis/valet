# ValetAutoFill — macOS AutoFill credential provider

Prototype of Valet's macOS password AutoFill integration. An
`ASCredentialProviderExtension` ships inside `ValetAutoFill.app`. The
extension is a thin Swift wrapper over the `valet-ipc` Rust staticlib, which
owns the wire protocol. Today it runs against a compiled-in stub; swapping in
the real daemon is a one-line change in Swift.

This is deliberately *not* the full Valet macOS app. `ValetAutoFill.app`
(bundle id `com.nixpulvis.valet.autofill`) is a minimal host whose only job is
to satisfy macOS's requirement that a credential provider `.appex` live inside
a `.app`. When `valet-gui` gets a real macOS `.app` bundle
(`com.nixpulvis.valet`), the `.appex` moves into its `Contents/PlugIns/` and
`ValetAutoFill.app` is deleted.

## Requirements

- macOS 14 or later
- Xcode Command Line Tools (`xcode-select --install`)
- Rust with the `aarch64-apple-darwin` target: `rustup target add aarch64-apple-darwin`
- No Xcode project, no GUI tooling — this builds entirely from the command line

## Build

    make                      # arm64, stub mode, ad-hoc signed
    make BUILD_MODE=connected # link for real daemon (no stub entry point)
    make ARCH=x86_64          # Intel
    make universal            # lipo'd arm64 + x86_64

Artifacts land in `build/`. The staticlib and generated C header come from
`../crates/valet-ipc/` (workspace member `valet-ipc`). Signing defaults to
ad-hoc (`-`). Override via `make SIGN='Developer ID Application: …'
DEVELOPMENT_TEAM=XXXXXXXXXX`.

## Install and enable

    make install     # copies build/ValetAutoFill.app to /Applications/ and launches
                     # it once so pluginkit registers the extension

Then:

1. Open **System Settings → General → AutoFill & Passwords**.
2. Toggle **ValetAutoFill** on.
3. Trigger AutoFill in Safari or any `UITextField` with a password content
   type. The Valet picker should appear with the two stub credentials.

Inspect with:

    make verify
    log stream --predicate 'subsystem == "com.nixpulvis.valet.autofill"'

## Swap the stub for the daemon

When the Valet daemon ships:

1. `make BUILD_MODE=connected` — strips the `VALET_IPC_STUB` define.
2. In `Extension/CredentialProviderViewController.swift`, `makeClient()`
   already falls through to `ValetClient.connect(socketPath:)` in the
   non-stub branch.
3. Uncomment the temporary-exception block in
   `Extension/ValetAutoFillExt.entitlements` so the sandboxed extension can
   reach the daemon socket.

No wire-type or FFI changes are required — the bitcode-encoded `Request` /
`Response` enums and the extern-"C" surface don't move.

## Layout

    App/
        main.swift             - minimal NSApplication host for the .appex
        Info.plist
        ValetAutoFill.entitlements
    Extension/
        CredentialProviderViewController.swift
        CredentialListView.swift
        Info.plist
        ValetAutoFillExt.entitlements
    Shared/
        ValetClient.swift      - Swift wrapper over the C FFI
        module.modulemap       - exposes crates/valet-ipc/include/valet_ipc.h
    Makefile
    README.md
