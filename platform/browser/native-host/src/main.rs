//! Browser native messaging shim for the valet addon.
//!
//! Speaks the WebExtensions native messaging wire format on stdin/stdout:
//! each message is a 4-byte little-endian length followed by a UTF-8 JSON
//! payload. The successful `result` field of a response is a base64 string
//! whose bytes are a bitcode-encoded [`valetd::Response`]. This program is
//! a stateless translator: it forwards every addon RPC to the `valetd`
//! daemon over a Unix socket and relays the reply back. The daemon holds
//! all the crypto state; when the shim exits, nothing is lost.
//!
//! The shim auto-spawns a sibling `valetd` binary the first time it fails to
//! reach the socket (for example, on first use after install). The sibling
//! is expected to live in the same directory as the shim binary. The daemon
//! detaches and outlives the shim; the idle reaper in `valetd` is what
//! eventually clears cached keys.

use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::Mutex;
use valet::{password::Password, record::Label, uuid::Uuid};
use valetd::{Request, Response, request::Frame, socket};

/// Maximum native messaging frame size (1 MiB). The browser enforces a
/// similar limit on the addon side.
const MAX_FRAME_SIZE: usize = 1024 * 1024;

/// How long to wait for the daemon's socket to appear after we spawn it.
const SPAWN_SOCKET_TIMEOUT: Duration = Duration::from_secs(5);
const SPAWN_SOCKET_POLL: Duration = Duration::from_millis(50);

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let socket_path: PathBuf = std::env::var_os("VALET_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(socket::default_path);

    let conn = match ensure_daemon(&socket_path).await {
        Ok(c) => c,
        Err(err) => {
            eprintln!("valet-native-host: cannot reach valetd: {err}");
            std::process::exit(1);
        }
    };
    let conn = Mutex::new(conn);

    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();

    loop {
        let mut len_buf = [0u8; 4];
        if stdin.read_exact(&mut len_buf).await.is_err() {
            break;
        }
        let len = u32::from_le_bytes(len_buf) as usize;
        if len == 0 || len > MAX_FRAME_SIZE {
            eprintln!("valet-native-host: invalid frame length {len}");
            break;
        }
        let mut buf = vec![0u8; len];
        if stdin.read_exact(&mut buf).await.is_err() {
            break;
        }

        let response = match serde_json::from_slice::<Value>(&buf) {
            Ok(req) => handle(&conn, req).await,
            Err(e) => json!({ "id": Value::Null, "error": format!("invalid json: {e}") }),
        };

        let bytes = match serde_json::to_vec(&response) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("valet-native-host: failed to serialize response: {e}");
                continue;
            }
        };
        if bytes.len() > MAX_FRAME_SIZE {
            eprintln!(
                "valet-native-host: response too large ({} bytes)",
                bytes.len()
            );
            continue;
        }
        let header = (bytes.len() as u32).to_le_bytes();
        if stdout.write_all(&header).await.is_err() || stdout.write_all(&bytes).await.is_err() {
            break;
        }
        if stdout.flush().await.is_err() {
            break;
        }
    }
}

/// Connect to the daemon, spawning a sibling `valetd` first if the socket is
/// absent. Returns the open stream.
async fn ensure_daemon(socket_path: &Path) -> std::io::Result<UnixStream> {
    if let Ok(conn) = UnixStream::connect(socket_path).await {
        return Ok(conn);
    }
    spawn_valetd()?;
    // Poll until the daemon binds its socket or we give up.
    let deadline = std::time::Instant::now() + SPAWN_SOCKET_TIMEOUT;
    loop {
        match UnixStream::connect(socket_path).await {
            Ok(conn) => return Ok(conn),
            Err(err) => {
                if std::time::Instant::now() >= deadline {
                    return Err(err);
                }
                tokio::time::sleep(SPAWN_SOCKET_POLL).await;
            }
        }
    }
}

/// Spawn the sibling `valetd` binary. Looks next to the current executable
/// first; falls back to `PATH`.
fn spawn_valetd() -> std::io::Result<()> {
    let sibling = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("valetd")))
        .filter(|p| p.exists());
    let program: PathBuf = sibling.unwrap_or_else(|| PathBuf::from("valetd"));

    // Detach: new session, stdio -> /dev/null, so the daemon outlives us.
    let mut cmd = std::process::Command::new(program);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            // Detach from the shim's controlling terminal and process group
            // so a terminating shim does not take the daemon with it.
            libc_setsid();
            Ok(())
        });
    }
    cmd.spawn().map(|_child| ())
}

#[cfg(unix)]
fn libc_setsid() {
    // Inline the libc call without adding a libc crate dep.
    unsafe extern "C" {
        fn setsid() -> i32;
    }
    unsafe {
        setsid();
    }
}

async fn handle(conn: &Mutex<UnixStream>, req: Value) -> Value {
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    let method = req
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let params = req.get("params").cloned().unwrap_or(Value::Null);

    match build_request(&method, &params) {
        Ok(daemon_req) => {
            let response = {
                let mut stream = conn.lock().await;
                match round_trip(&mut stream, daemon_req).await {
                    Ok(r) => r,
                    Err(e) => return json!({ "id": id, "error": format!("daemon io: {e}") }),
                }
            };
            match response {
                Response::Error(msg) => json!({ "id": id, "error": msg }),
                other => json!({ "id": id, "result": other.encode_base64() }),
            }
        }
        Err(err) => json!({ "id": id, "error": err }),
    }
}

async fn round_trip(conn: &mut UnixStream, req: Request) -> std::io::Result<Response> {
    req.send_async(conn).await?;
    Response::recv_async(conn).await
}

fn build_request(method: &str, params: &Value) -> Result<Request, String> {
    match method {
        "status" => Ok(Request::Status),
        "list_users" => Ok(Request::ListUsers),
        "unlock" => {
            let username = string_param(params, "username")?;
            let password = password_param(params, "password")?;
            Ok(Request::Unlock { username, password })
        }
        "lock" => Ok(Request::Lock {
            username: string_param(params, "username")?,
        }),
        "lock_all" => Ok(Request::LockAll),
        "find_records" => {
            let username = string_param(params, "username")?;
            let lot = string_param(params, "lot")?;
            let query = string_param(params, "domain")?;
            Ok(Request::FindRecords {
                username,
                lot,
                query,
            })
        }
        "get_record" => {
            let username = string_param(params, "username")?;
            let lot = string_param(params, "lot")?;
            let uuid_str = string_param(params, "record_uuid")?;
            let uuid = Uuid::parse(&uuid_str).map_err(|e| format!("{e:?}"))?;
            Ok(Request::GetRecord {
                username,
                lot,
                uuid,
            })
        }
        "create_record" => {
            let username = string_param(params, "username")?;
            let lot = string_param(params, "lot")?;
            let label = label_param(params, "label")?;
            let password = password_param(params, "password")?;
            let extra = extra_param(params)?;
            Ok(Request::CreateRecord {
                username,
                lot,
                label,
                password,
                extra,
            })
        }
        "generate_record" => {
            let username = string_param(params, "username")?;
            let lot = string_param(params, "lot")?;
            let label = label_param(params, "label")?;
            Ok(Request::GenerateRecord {
                username,
                lot,
                label,
            })
        }
        other => Err(format!("unknown method '{other}'")),
    }
}

fn string_param(v: &Value, name: &str) -> Result<String, String> {
    v.get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("missing string param '{name}'"))
}

fn password_param(v: &Value, name: &str) -> Result<Password, String> {
    let s = string_param(v, name)?;
    Password::try_from(s.as_str()).map_err(|_| "password too long".to_string())
}

fn label_param(v: &Value, name: &str) -> Result<Label, String> {
    let s = string_param(v, name)?;
    Label::from_str(&s).map_err(|e| format!("{e:?}"))
}

fn extra_param(v: &Value) -> Result<HashMap<String, String>, String> {
    match v.get("extra") {
        None | Some(Value::Null) => Ok(HashMap::new()),
        Some(Value::Object(map)) => map
            .iter()
            .map(|(k, val)| {
                val.as_str()
                    .map(|s| (k.clone(), s.to_string()))
                    .ok_or_else(|| format!("extra['{k}'] must be a string"))
            })
            .collect(),
        Some(_) => Err("'extra' must be an object".to_string()),
    }
}
