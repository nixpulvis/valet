//! Firefox native messaging host for the valet addon.
//!
//! Speaks the WebExtensions native messaging wire format on stdin/stdout:
//! each message is a 4-byte little-endian length followed by a UTF-8 JSON
//! payload. The successful `result` field of a response is a base64 string
//! whose bytes are bitcode-encoded `RpcResult` (see the library half of this crate).
//!
//! Firefox launches this process when the addon calls
//! `browser.runtime.connectNative("com.nixpulvis.valet")` and closes it by
//! closing stdin. When the process exits, valet's `ZeroizeOnDrop` clears the
//! cached user/lot keys.

use serde_json::{Value, json};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;
use tokio::time::Instant;
use valet::{
    Lot, Record,
    db::{self, Database},
    password::Password,
    record::{Data, Label},
    user::User,
    uuid::Uuid,
};
use valet_native_host::{RecordResult, RpcResult, encode_result};

const FAILED_UNLOCK_DELAY_MS: u64 = 750;
const IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const IDLE_CHECK_INTERVAL: Duration = Duration::from_secs(15);
/// Maximum native messaging frame size (1 MiB). Firefox enforces a similar limit.
const MAX_FRAME_SIZE: usize = 1024 * 1024;

struct State {
    db: Database,
    users: HashMap<String, User>,
    lots: HashMap<(String, String), Lot>,
    /// Set whenever there are unlocked users; cleared when state is dropped.
    /// The reaper task uses this as a sliding deadline.
    last_activity: Option<Instant>,
}

impl State {
    fn drop_user(&mut self, username: &str) {
        self.users.remove(username);
        self.lots.retain(|(u, _), _| u != username);
        if self.users.is_empty() {
            self.last_activity = None;
        }
    }

    fn drop_all(&mut self) {
        self.users.clear();
        self.lots.clear();
        self.last_activity = None;
    }

    fn touch(&mut self) {
        if !self.users.is_empty() {
            self.last_activity = Some(Instant::now());
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let db_url = std::env::var("VALET_DB").unwrap_or_else(|_| db::default_url());

    let db = match Database::new(&db_url).await {
        Ok(db) => db,
        Err(err) => {
            eprintln!("valet-native-host: failed to open database: {err:?}");
            std::process::exit(1);
        }
    };

    let state = Arc::new(Mutex::new(State {
        db,
        users: HashMap::new(),
        lots: HashMap::new(),
        last_activity: None,
    }));

    // Background reaper: drops cached keys after IDLE_TIMEOUT of inactivity.
    {
        let state = state.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(IDLE_CHECK_INTERVAL).await;
                let mut st = state.lock().await;
                if let Some(last) = st.last_activity {
                    if last.elapsed() >= IDLE_TIMEOUT {
                        st.drop_all();
                        eprintln!("valet-native-host: idle timeout, locked all users");
                    }
                }
            }
        });
    }

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
            Ok(req) => handle(&state, req).await,
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

async fn handle(state: &Arc<Mutex<State>>, req: Value) -> Value {
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    let method = req
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let params = req.get("params").cloned().unwrap_or(Value::Null);

    match dispatch(state, &method, params).await {
        Ok(result) => {
            state.lock().await.touch();
            json!({ "id": id, "result": encode_result(&result) })
        }
        Err(err) => json!({ "id": id, "error": err }),
    }
}

async fn dispatch(
    state: &Arc<Mutex<State>>,
    method: &str,
    params: Value,
) -> Result<RpcResult, String> {
    match method {
        // TODO: The lib should also define RpcRequests instead of each of these
        // parsing it manually here. Requests should be paired with responses in
        // a separate structure and used here in a repeatable abstracted way.
        "status" => {
            let st = state.lock().await;
            let mut users: Vec<String> = st.users.keys().cloned().collect();
            users.sort();
            Ok(RpcResult::Unlocked(users))
        }
        "list_users" => {
            let st = state.lock().await;
            let users = User::list(&st.db).await.map_err(err)?;
            Ok(RpcResult::Users(users))
        }
        "unlock" => {
            let username = string_param(&params, "username")?;
            let password_str = string_param(&params, "password")?;
            let password = Password::try_from(password_str.as_str())
                .map_err(|_| "password too long".to_string())?;
            let mut st = state.lock().await;
            match User::load(&st.db, &username, password).await {
                Ok(user) => {
                    st.users.insert(username, user);
                    Ok(RpcResult::Ok)
                }
                Err(e) => {
                    drop(st);
                    tokio::time::sleep(Duration::from_millis(FAILED_UNLOCK_DELAY_MS)).await;
                    Err(err(e))
                }
            }
        }
        "lock" => {
            let username = string_param(&params, "username")?;
            let mut st = state.lock().await;
            st.drop_user(&username);
            Ok(RpcResult::Ok)
        }
        "lock_all" => {
            let mut st = state.lock().await;
            st.drop_all();
            Ok(RpcResult::Ok)
        }
        "find_records" => {
            let username = string_param(&params, "username")?;
            let lot_name = string_param(&params, "lot")?;
            let domain = string_param(&params, "domain")?.to_lowercase();
            let mut st = state.lock().await;
            ensure_lot(&mut st, &username, &lot_name).await?;
            let State { db, lots, .. } = &*st;
            let lot = &lots[&(username, lot_name)];
            let records = lot.records(db).await.map_err(err)?;
            let matches: Vec<Record> = records
                .into_iter()
                .filter(|r| match r.label() {
                    Label::Domain { domain: d, .. } => domain_matches(&d.to_lowercase(), &domain),
                    _ => false,
                })
                .collect();
            Ok(RpcResult::Record(RecordResult::List(matches)))
        }
        "get_record" => {
            let username = string_param(&params, "username")?;
            let lot_name = string_param(&params, "lot")?;
            let record_uuid = string_param(&params, "record_uuid")?;
            let target = Uuid::<Record>::parse(&record_uuid).map_err(err)?;
            let mut st = state.lock().await;
            ensure_lot(&mut st, &username, &lot_name).await?;
            let State { db, lots, .. } = &*st;
            let lot = &lots[&(username, lot_name)];
            let records = lot.records(db).await.map_err(err)?;
            let record = records
                .into_iter()
                .find(|r| r.uuid() == &target)
                .ok_or_else(|| "record not found".to_string())?;
            Ok(RpcResult::Record(RecordResult::Get(record)))
        }
        "create_record" => {
            let username = string_param(&params, "username")?;
            let lot_name = string_param(&params, "lot")?;
            let label_str = string_param(&params, "label")?;
            let password_str = string_param(&params, "password")?;
            let extra = extra_param(&params)?;
            let label = Label::from_str(&label_str).map_err(|e| format!("{e:?}"))?;
            let password = Password::try_from(password_str.as_str())
                .map_err(|_| "password too long".to_string())?;
            let mut data = Data::new(label, password);
            if !extra.is_empty() {
                data = data.with_extra(extra);
            }
            let mut st = state.lock().await;
            ensure_lot(&mut st, &username, &lot_name).await?;
            let State { db, lots, .. } = &*st;
            let lot = &lots[&(username, lot_name)];
            let record = Record::new(lot, data);
            record.upsert(db, lot).await.map_err(err)?;
            Ok(RpcResult::Record(RecordResult::Created(record)))
        }
        "generate_record" => {
            let username = string_param(&params, "username")?;
            let lot_name = string_param(&params, "lot")?;
            let label_str = string_param(&params, "label")?;
            let label = Label::from_str(&label_str).map_err(|e| format!("{e:?}"))?;
            let password = Password::generate();
            let data = Data::new(label, password);
            let mut st = state.lock().await;
            ensure_lot(&mut st, &username, &lot_name).await?;
            let State { db, lots, .. } = &*st;
            let lot = &lots[&(username, lot_name)];
            let record = Record::new(lot, data);
            record.upsert(db, lot).await.map_err(err)?;
            Ok(RpcResult::Record(RecordResult::Generated(record)))
        }
        other => Err(format!("unknown method '{other}'")),
    }
}

async fn ensure_lot(st: &mut State, username: &str, lot_name: &str) -> Result<(), String> {
    let key = (username.to_string(), lot_name.to_string());
    if st.lots.contains_key(&key) {
        return Ok(());
    }
    let user = st
        .users
        .get(username)
        .ok_or_else(|| format!("user '{username}' is locked"))?;
    let lot = Lot::load(&st.db, lot_name, user)
        .await
        .map_err(err)?
        .ok_or_else(|| format!("lot '{lot_name}' not found"))?;
    st.lots.insert(key, lot);
    Ok(())
}

fn string_param(v: &Value, name: &str) -> Result<String, String> {
    v.get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("missing string param '{name}'"))
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

fn domain_matches(record_domain: &str, query: &str) -> bool {
    record_domain == query
        || query.ends_with(&format!(".{record_domain}"))
        || record_domain.ends_with(&format!(".{query}"))
}

fn err<E: std::fmt::Debug>(e: E) -> String {
    format!("{e:?}")
}
