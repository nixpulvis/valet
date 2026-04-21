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

1. **Native-host install paths** in `xtask/src/main.rs` (`native_messaging_dir`)
   point at the Mozilla-specific directories
   (`~/Library/Application Support/Mozilla/NativeMessagingHosts` on macOS,
   `~/.mozilla/native-messaging-hosts` on Linux). Chromium-family browsers
   use per-browser directories (e.g. `…/Google/Chrome/NativeMessagingHosts`).
2. **Native-host manifest template** (`native-host/com.nixpulvis.valet.json.template`)
   uses Firefox's `allowed_extensions` field with the Gecko add-on ID.
   Chromium uses `allowed_origins: ["chrome-extension://<id>/"]`.
3. **Extension `manifest.json`**: the `browser_specific_settings.gecko.id`
   field is Gecko-only, and the `background` entry uses `scripts` + module
   loading. Chromium MV3 requires `background.service_worker`, and service
   workers place tighter limits on WASM and top-level `await` that would
   push some of `background.js` around.
4. **Dev-load instructions**: `about:debugging` + "Load Temporary Add-on"
   is Firefox's flow. Chromium uses `chrome://extensions` + Developer mode
   + "Load unpacked".

Everything else is the same across browsers: `browser.*` WebExtensions
API use, the native-messaging wire format, the Yew popup, and the
`valetd` RPC shape. For multi-browser support the natural shape is a
`--target firefox|chrome` flag on the xtask that selects (1) and (2) and
templates (3) from per-browser manifest variants.

## Architecture

The extension has three main pieces:

```
popup (Yew/WASM)
<- sendMessage ->
background (WASM)
<- native messaging ->
native-host shim (valet-native-host)
<- Unix socket ->
valetd daemon
<- SQL ->
valet SQLite DB
```

- **Popup** (`src/popup/`): a [Yew](https://yew.rs) single-page app rendered
  in the extension popup. Handles unlock, record listing, copy-to-clipboard,
  and credential creation. Communicates with the background script via
  `browser.runtime.sendMessage`.

- **Background script** (`src/background/`): runs as a persistent service
  worker. Opens a native messaging port to the native host and multiplexes
  RPC calls from the popup.

- **Native-host shim** (`native-host/`): a stateless Rust binary
  (`valet-native-host`) that the browser launches via the WebExtensions
  native messaging API. Speaks the 4-byte-length-prefixed JSON wire format
  on stdin/stdout and forwards each request to the `valetd` daemon over a
  Unix socket. Auto-spawns a sibling `valetd` binary the first time the
  socket is absent. Holds no state of its own.

- **`valetd` daemon** (`../../valetd/`): owns the database and the
  unlocked-user cache. Persists across shim restarts and serves other
  platforms (macOS AutoFill) from the same socket. An idle-timeout reaper
  (5 min) clears cached keys automatically.

### Wire format

The JSON frame between background and native host:

```json
{ "id": 1, "method": "unlock", "params": { "username": "...", "password": "..." } }
```

The successful `result` field is a base64-encoded bitcode blob of a
`valetd::Response`. The popup decodes it in WASM by depending on `valetd`
with `default-features = false` (wire-types only) and calling
`Response::decode_base64`, so both halves share exactly one wire schema.
An unsuccessful call sets an `error` string instead.

### RPC methods

| Method | Params | `Response` variant |
|--------|--------|--------------------|
| `status` | — | `Users(Vec<String>)` (unlocked users) |
| `list_users` | — | `Users(Vec<String>)` (all registered) |
| `unlock` | `username`, `password` | `Ok` |
| `lock` | `username` | `Ok` |
| `lock_all` | — | `Ok` |
| `find_records` | `username`, `lot`, `domain` | `Index(Vec<(Uuid<Record>, Label)>)` |
| `get_record` | `username`, `lot`, `record_uuid` | `Record(Record)` |
| `create_record` | `username`, `lot`, `label`, `password`, `extra?` | `Record(Record)` |
| `generate_record` | `username`, `lot`, `label` | `Record(Record)` |

`find_records` returns only label-and-uuid pairs; the actual password is
decrypted on demand by `get_record` when the user clicks *Copy*, so no
password material crosses the socket unless it's about to be used.

## Prerequisites

- [Rust](https://rustup.rs/) (stable)
- [`wasm-pack`](https://rustwasm.github.io/wasm-pack/installer/)
- Firefox (Developer Edition recommended for extension development)
- An existing Valet database with at least one registered user (via
  `cargo run --bin valet -- register <username>`)

## Building

### 1. Build the WASM package

```sh
wasm-pack build --target web platform/browser
```

This produces the `pkg/` directory that the extension loads at runtime.

### 2. Build and install the native host

```sh
cargo browser-xtask install-native-host
```

This builds `valet-native-host` and `valetd` in release mode and writes a
native messaging manifest to the OS-appropriate Firefox location:

- **macOS:** `~/Library/Application Support/Mozilla/NativeMessagingHosts/com.nixpulvis.valet.json`
- **Linux:** `~/.mozilla/native-messaging-hosts/com.nixpulvis.valet.json`
- **Windows:** TODO

The shim looks for `valetd` next to its own executable; both land in the
same `target/<profile>/` directory by default. Set `VALET_DB` or
`VALET_SOCKET` in your shell environment before launching the browser if
you want to use non-default paths.

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

Rebuild the native host or daemon after Rust changes:

```sh
cargo build -p valet-native-host -p valetd
```

## Crate structure

```
platform/browser/
  Cargo.toml            # valet-browser (cdylib, wasm)
  manifest.json         # MV3 extension manifest
  popup.html / popup.js # Extension popup entry point
  background.js         # Background script entry point
  xtask/                # cargo browser-xtask (install-native-host, etc.)
  src/
    lib.rs              # wasm_bindgen exports
    logging.rs          # tracing → browser console
    background/
      mod.rs            # start_background()
      externs.rs        # FFI bindings to browser.runtime.*
      port.rs           # Native messaging port, RPC multiplexing
    popup/
      mod.rs            # start_popup(), Yew renderer
      app.rs            # Yew components (LockView, UnlockedView)
      browser.rs        # FFI bindings to browser.tabs.*, clipboard
      rpc.rs            # Typed RPC wrappers (list_users, unlock, etc.)
  native-host/
    Cargo.toml          # valet-native-host (bin only)
    src/
      main.rs           # Native messaging host binary (stdio <-> Unix socket)
    com.nixpulvis.valet.json.template
```
