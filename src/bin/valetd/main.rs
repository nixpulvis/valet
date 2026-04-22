//! `valetd` - the Valet daemon.
//!
//! One process, two I/O loops selected by
//! `--transport=socket|native-messaging`. Auto-detect when the flag is
//! absent: stdin is a terminal -> `socket`; stdin is a pipe ->
//! `native-messaging` (the way browsers launch native-host binaries).
//!
//! Socket transport: binds `$VALET_SOCKET` (or the default path) and
//! serves a `Client<Embedded>` owning a SQLite handle.
//!
//! Native-messaging transport: reads/writes the browser's stdio
//! native-messaging envelope. `VALET_BACKEND=auto|socket|embedded`
//! picks what handler fields the requests:
//!
//! * `embedded` - a `Client<Embedded>` owning its own SQLite handle.
//! * `socket` - a `Client<Socket>` that relays to another long-lived
//!   `valetd --transport=socket`.
//! * `auto` (default) - probe the socket; fall back to embedded.
//!
//! Each transport and backend is gated on the corresponding
//! `protocol-*` feature. A build that leaves one out still produces a
//! `valetd` binary; the missing paths are simply rejected at runtime
//! with a feature-specific error.

use std::io::IsTerminal;

use clap::{Parser, ValueEnum};
use tracing::error;

#[cfg(all(
    feature = "protocol-embedded",
    any(feature = "protocol-socket", feature = "protocol-native-msg-server"),
))]
use std::sync::Arc;
#[cfg(all(
    feature = "protocol-embedded",
    any(feature = "protocol-socket", feature = "protocol-native-msg-server"),
))]
use valet::protocol::{Client, embedded::Embedded};

#[cfg(all(feature = "protocol-socket", feature = "protocol-embedded"))]
mod socket_cli;
#[cfg(feature = "protocol-native-msg-server")]
mod native_msg_cli;

#[derive(Clone, Copy, Debug, ValueEnum)]
#[clap(rename_all = "kebab-case")]
pub(crate) enum Transport {
    Socket,
    NativeMessaging,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Backend {
    Auto,
    Socket,
    Embedded,
}

/// Valet daemon.
///
/// Without `--transport`, auto-detects: stdin is a terminal -> socket;
/// stdin is a pipe (a browser launched us) -> native-messaging.
#[derive(Debug, Parser)]
#[command(
    name = "valetd",
    about = "Valet daemon.",
    after_help = "Environment:\n  \
        VALET_SOCKET   Socket path for --transport=socket and for the\n                 \
                       native-messaging socket-relay backend.\n  \
        VALET_DB       SQLite URL for the embedded backend.\n  \
        VALET_BACKEND  For --transport=native-messaging only:\n                 \
                       auto|socket|embedded (default: auto)."
)]
struct Args {
    /// Transport to serve. Auto-detected from stdin when omitted.
    #[arg(long, value_enum)]
    transport: Option<Transport>,

    /// Browsers launch native-messaging hosts with extra positional
    /// argv (Firefox: extension id, manifest path; Chrome: caller
    /// origin). Swallow and ignore them.
    #[arg(trailing_var_arg = true, hide = true)]
    extras: Vec<String>,
}

fn auto_transport() -> Transport {
    // The browser launches native-host binaries with piped stdio, so
    // a non-terminal stdin is the strongest indicator we should speak
    // native-messaging. A user launching from a shell gets the socket
    // listener. systemd/launchd deployments should pass
    // `--transport=socket` explicitly.
    if std::io::stdin().is_terminal() {
        Transport::Socket
    } else {
        Transport::NativeMessaging
    }
}

pub(crate) fn parse_backend() -> Result<Backend, String> {
    match std::env::var("VALET_BACKEND").as_deref() {
        Ok("") | Ok("auto") | Err(_) => Ok(Backend::Auto),
        Ok("socket") => Ok(Backend::Socket),
        Ok("embedded") => Ok(Backend::Embedded),
        Ok(other) => Err(format!(
            "VALET_BACKEND={other}: expected 'auto', 'socket', or 'embedded'"
        )),
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    valet::logging::init();
    let args = Args::parse();
    for extra in &args.extras {
        tracing::debug!(arg = %extra, "ignoring positional argument");
    }
    let transport = args.transport.unwrap_or_else(auto_transport);

    let result = match transport {
        Transport::Socket => run_socket().await,
        Transport::NativeMessaging => {
            let backend = match parse_backend() {
                Ok(b) => b,
                Err(err) => {
                    error!("{err}");
                    std::process::exit(1);
                }
            };
            run_native_msg(backend).await
        }
    };
    if let Err(err) = result {
        error!("{err}");
        std::process::exit(1);
    }
}

#[cfg(all(feature = "protocol-socket", feature = "protocol-embedded"))]
async fn run_socket() -> Result<(), String> {
    socket_cli::run().await
}

#[cfg(not(all(feature = "protocol-socket", feature = "protocol-embedded")))]
async fn run_socket() -> Result<(), String> {
    Err("socket transport is disabled in this build \
         (requires features `protocol-socket` and `protocol-embedded`)"
        .to_string())
}

#[cfg(feature = "protocol-native-msg-server")]
async fn run_native_msg(backend: Backend) -> Result<(), String> {
    native_msg_cli::run(backend).await
}

#[cfg(not(feature = "protocol-native-msg-server"))]
async fn run_native_msg(_backend: Backend) -> Result<(), String> {
    Err("native-messaging transport is disabled in this build \
         (requires feature `protocol-native-msg-server`)"
        .to_string())
}

#[cfg(all(
    feature = "protocol-embedded",
    any(feature = "protocol-socket", feature = "protocol-native-msg-server"),
))]
pub(crate) async fn build_embedded_handler() -> Result<Arc<Client<Embedded>>, String> {
    Ok(Arc::new(Client::<Embedded>::open_from_env().await?))
}
