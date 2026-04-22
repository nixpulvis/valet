# Valet Browser Extension

A WebExtensions browser extension for [Valet](../../README.md) that gives you
in-browser access to your password vault. The popup automatically matches
credentials to the current tab's domain and lets you copy passwords to the
clipboard (auto-cleared after 20 seconds). You can also save new credentials
directly from the browser. Firefox (MV3) is the currently-supported target;
the WebExtensions APIs used here are portable to Chrome-family browsers in
principle, though install paths and manifest details differ.

### Firefox-specific surface

Most of the tree is portable WebExtensions code. The pieces that are
actually Firefox-only, and would need alternatives for another browser:

1. **Native-host install paths** in `Makefile` (`NM_DIR`) point at the
   Mozilla-specific directories
   (`~/Library/Application Support/Mozilla/NativeMessagingHosts` on macOS,
   `~/.mozilla/native-messaging-hosts` on Linux). Chromium-family browsers
   use per-browser directories (e.g. `…/Google/Chrome/NativeMessagingHosts`).
2. **Native-host manifest** (inlined in `Makefile`) uses Firefox's
   `allowed_extensions` field with the Gecko add-on ID. Chromium uses
   `allowed_origins: ["chrome-extension://<id>/"]`.
3. **Extension `manifest.json`**: the `browser_specific_settings.gecko.id`
   field is Gecko-only, and the `background` entry uses `scripts` + module
   loading. Chromium MV3 requires `background.service_worker`, and service
   workers place tighter limits on WASM and top-level `await` that would
   push some of `background.js` around.
4. **Dev-load instructions**: `about:debugging` + "Load Temporary Add-on"
   is Firefox's flow. Chromium uses `chrome://extensions` + Developer mode
   + "Load unpacked".

Everything else is the same across browsers: `browser.*` WebExtensions
API use, the native-messaging wire format, the Yew popup, and the RPC
shape. For multi-browser support the natural shape is a
`TARGET=firefox|chrome` variable on the Makefile that selects (1) and
(2) and templates (3) from per-browser manifest variants.

## Architecture

The extension has two WASM pieces plus the `valetd` binary on the host:

```
popup (Yew/WASM)
<- sendMessage ->
background (WASM)
<- native messaging ->
valetd (native-messaging transport)
<- SQLite / Unix socket ->
valet vault
```

- **Popup** (`src/popup/`): a [Yew](https://yew.rs) single-page app rendered
  in the extension popup. Handles unlock, record listing, copy-to-clipboard,
  and credential creation. Communicates with the background script via
  `browser.runtime.sendMessage`.

- **Background script** (`src/background/`): opens a native-messaging port
  to `valetd` and multiplexes typed RPC calls from the popup through it.
  Uses `Client<NativeMessage>` from the main `valet` crate; the same
  `Handler` surface the native clients (CLI, GUI, FFI) use.

- **`valetd` daemon** (top-level `src/bin/valetd/`): when the browser
  launches it with piped stdio, `valetd` auto-selects its
  native-messaging transport. `VALET_BACKEND` picks what fields each
  request:

  - `embedded` (default when no socket is live): `valetd` opens its own
    SQLite handle and serves requests in-process. The spawned process
    lives only as long as the browser keeps the port open; per-user key
    caches die with it.
  - `socket`: relay each frame to a long-lived
    `valetd --transport=socket` running elsewhere. Use this when macOS
    AutoFill and the browser need to share one unlocked session.

### Wire format

WebExtensions frames stdio with a 4-byte little-endian length prefix,
then a JSON envelope. The envelope carries a base64-encoded bitcode
payload so the typed Rust schema crosses the wire unchanged:

```json
{ "id": 1, "request": "<base64-bitcode-Request>" }
```

Reply:

```json
{ "backend": "embedded",
  "payload": { "Ok": { "id": 1, "data": "<base64-bitcode-Response>" } } }
```

or on failure:

```json
{ "backend": "embedded", "payload": { "Err": "<message>" } }
```

The `id` correlates each reply to its request so the background script
can route out-of-order replies back to waiting popup callers. `backend`
tags which transport actually served the call so logs can tell
embedded-mode vs. socket-relay traffic apart.

`Request` / `Response` are the same enums the socket transport speaks;
both halves of the wire import them from
`valet::protocol::message`. See `src/protocol/impls/native_msg.rs` for
the envelope types (`NativeRequest`, `NativeReply`, `NativePayload`)
and `src/protocol/message.rs` for the message enums.

## Prerequisites

- [Rust](https://rustup.rs/) (stable)
- [`wasm-pack`](https://rustwasm.github.io/wasm-pack/installer/)
- Firefox (Developer Edition recommended for extension development)
- An existing Valet database with at least one registered user (via
  `cargo run --bin valet -- register <username>`)

## Building

### 1. Build the WASM package

From the repo root:

```sh
make browser
```

(or `make -C platform/browser` directly). This produces the `pkg/`
directory that the extension loads at runtime.

### 2. Build and register the native host

```sh
make install-browser
```

This rebuilds the WASM package, builds `valetd`, and writes a
native-messaging manifest pointing at that binary:

- **macOS:** `~/Library/Application Support/Mozilla/NativeMessagingHosts/com.nixpulvis.valet.json`
- **Linux:** `~/.mozilla/native-messaging-hosts/com.nixpulvis.valet.json`
- **Windows:** TODO

Pass `RELEASE=1` to either target for an optimized build. Set
`VALET_DB`, `VALET_SOCKET`, or `VALET_BACKEND` in your shell environment
before launching the browser to override the defaults; the spawned
`valetd` inherits its env.

### 3. Load the extension

1. Open `about:debugging#/runtime/this-firefox` in Firefox
2. Click **Load Temporary Add-on...**
3. Select `platform/browser/manifest.json`

The Valet popup should now appear when you click the extension icon.

## Development

Rebuild WASM after Rust changes:

```sh
wasm-pack build --target web --dev platform/browser
```

The `--dev` flag includes debug assertions and TRACE-level logging to the
browser console (visible in the extension's background page inspector and the
popup's dev tools).

Rebuild `valetd` after Rust changes:

```sh
cargo build --bin valetd
```

After rebuilding, reload the extension (`about:debugging` → *Reload*) so
the background script reopens the native-messaging port against the
fresh binary.

## Crate structure

```
platform/browser/
  Cargo.toml            # valet-browser (cdylib, wasm)
  manifest.json         # MV3 extension manifest
  popup.html / popup.js # Extension popup entry point
  background.js         # Background script entry point
  Makefile              # default: build WASM; `install` also builds valetd and writes the native-messaging manifest
  src/
    lib.rs              # wasm_bindgen exports
    logging.rs          # tracing -> browser console
    background/
      mod.rs            # start_background()
      externs.rs        # FFI bindings to browser.runtime.*
      port.rs           # Client<NativeMessage> wrapper + RPC routing
    popup/
      mod.rs            # start_popup(), Yew renderer
      app.rs            # Yew components (LockView, UnlockedView)
      browser.rs        # FFI bindings to browser.tabs.*, clipboard
    rpc.rs              # Typed RPC wrappers over Client<NativeMessage>
```
