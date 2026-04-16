# Valet Firefox Extension

A Firefox extension for [Valet](../../README.md) that gives you browser access
to your password vault. The popup automatically matches credentials to the
current tab's domain and lets you copy passwords to the clipboard (auto-cleared
after 20 seconds). You can also save new credentials directly from the browser.

## Architecture

The extension has three main pieces:

```
popup (Yew/WASM)
<- sendMessage ->
background (WASM)
<- native messaging ->
native-host (Rust binary)
<- SQL ->
valet SQLite DB
```

- **Popup** (`src/popup/`) — A [Yew](https://yew.rs) single-page app rendered
  in the extension popup. Handles unlock, record listing, copy-to-clipboard,
  and credential creation. Communicates with the background script via
  `browser.runtime.sendMessage`.

- **Background script** (`src/background/`) — Runs as a persistent service
  worker. Opens a native messaging port to the native host and multiplexes
  RPC calls from the popup.

- **Native host** (`native-host/`) — A standalone Rust binary
  (`valet-native-host`) that Firefox launches via the native messaging API.
  Speaks the WebExtensions 4-byte-length-prefixed JSON wire format on
  stdin/stdout. Holds unlocked user keys in memory, with an idle timeout
  (5 min) that automatically locks all users. Successful RPC results are
  base64-encoded bitcode for compact, typed transport back to the WASM side.

### Wire format

The JSON frame between background and native host:

```json
{ "id": 1, "method": "unlock", "params": { "username": "...", "password": "..." } }
```

Responses carry a base64-bitcode `result` field (decoded in WASM via the shared
`valet-native-host` lib crate) or an `error` string.

### RPC methods

| Method | Params | Returns |
|--------|--------|---------|
| `status` | — | `Unlocked(Vec<String>)` |
| `list_users` | — | `Users(Vec<String>)` |
| `unlock` | `username`, `password` | `Ok` |
| `lock` | `username` | `Ok` |
| `lock_all` | — | `Ok` |
| `list_lots` | `username` | `Lots(Vec<String>)` |
| `find_records` | `username`, `lot`, `domain` | `Records(Vec<RecordView>)` |
| `get_password` | `username`, `lot`, `record_uuid` | `Password(String)` |
| `create_record` | `username`, `lot`, `label`, `password` | `Created { uuid }` |

## Prerequisites

- [Rust](https://rustup.rs/) (stable)
- [`wasm-pack`](https://rustwasm.github.io/wasm-pack/installer/)
- Firefox (Developer Edition recommended for extension development)
- An existing Valet database with at least one registered user (via
  `cargo run --bin valet -- register <username>`)

## Building

### 1. Build the WASM package

```sh
wasm-pack build --target web platform/firefox
```

This produces the `pkg/` directory that the extension loads at runtime.

### 2. Build and install the native host

```sh
cargo firefox-xtask install-native-host
```

This builds `valet-native-host` in release mode and writes a native
messaging manifest to the OS-appropriate location:

- **macOS:** `~/Library/Application Support/Mozilla/NativeMessagingHosts/com.nixpulvis.valet.json`
- **Linux:** `~/.mozilla/native-messaging-hosts/com.nixpulvis.valet.json`
- **Windows:** TODO

Set `VALET_DB` in your shell environment before launching Firefox if you want
to use a non-default database path.

### 3. Load the extension

1. Open `about:debugging#/runtime/this-firefox` in Firefox
2. Click **Load Temporary Add-on...**
3. Select `platform/firefox/manifest.json`

The Valet popup should now appear when you click the extension icon.

## Development

Rebuild WASM after Rust changes:

```sh
wasm-pack build --target web --dev platform/firefox
```

The `--dev` flag includes debug assertions and TRACE-level logging to the
browser console (visible in the extension's background page inspector and the
popup's dev tools).

Rebuild the native host after changes to `native-host/`:

```sh
cargo build -p valet-native-host
```

## Crate structure

```
platform/firefox/
  Cargo.toml            # valet-firefox (cdylib, wasm)
  manifest.json         # MV3 extension manifest
  popup.html / popup.js # Extension popup entry point
  background.js         # Background script entry point
  xtask/                # cargo firefox-xtask (install-native-host, etc.)
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
    Cargo.toml          # valet-native-host (lib + bin)
    src/
      lib.rs            # Wire schema (RpcResult, RecordView, encode/decode)
      main.rs           # Native messaging host binary
    com.nixpulvis.valet.json.template
```
