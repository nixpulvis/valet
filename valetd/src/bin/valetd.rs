//! `valetd` - the Valet daemon.
//!
//! Owns a single SQLite database, a cache of unlocked [`User`] / [`Lot`]
//! keys, and serves the [`valetd::Request`] / [`valetd::Response`] protocol
//! on a Unix socket. Cached keys are dropped after [`IDLE_TIMEOUT`] with no
//! activity; they are also dropped when the process exits because
//! [`valet::encrypt::Key`] is `ZeroizeOnDrop`.
//!
//! Socket path: `$VALET_SOCKET` if set, otherwise [`valetd::socket::default_path`].
//! Database path: `$VALET_DB` if set, otherwise [`valet::db::default_url`].

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;
use tokio::time::Instant;
use valet::{
    Lot, Record,
    db::{self, Database},
    password::Password,
    record::{Data, Label, RecordIndex},
    user::User,
    uuid::Uuid,
};
use valetd::{
    Request, Response,
    request::{Frame, label_matches_domain},
    socket,
};

const FAILED_UNLOCK_DELAY_MS: u64 = 750;
const IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const IDLE_CHECK_INTERVAL: Duration = Duration::from_secs(15);

struct State {
    db: Database,
    users: HashMap<String, User>,
    lots: HashMap<(String, String), Lot>,
    /// Set whenever there are unlocked users; cleared when state is dropped.
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

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let db_url = std::env::var("VALET_DB").unwrap_or_else(|_| db::default_url());
    let socket_path: PathBuf = std::env::var_os("VALET_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(socket::default_path);

    let db = match Database::new(&db_url).await {
        Ok(db) => db,
        Err(err) => {
            eprintln!("valetd: failed to open database at {db_url}: {err:?}");
            std::process::exit(1);
        }
    };

    if let Some(parent) = socket_path.parent() {
        if let Err(err) = std::fs::create_dir_all(parent) {
            eprintln!(
                "valetd: failed to create socket directory {}: {err}",
                parent.display()
            );
            std::process::exit(1);
        }
    }
    // A stale socket file from a crashed previous run would make bind() fail
    // with EADDRINUSE; remove it. If something is actually listening, the
    // bind below will still fail on the port-already-in-use race (not much
    // we can do from here).
    let _ = std::fs::remove_file(&socket_path);

    let listener = match UnixListener::bind(&socket_path) {
        Ok(l) => l,
        Err(err) => {
            eprintln!("valetd: failed to bind {}: {err}", socket_path.display());
            std::process::exit(1);
        }
    };
    eprintln!("valetd: listening on {}", socket_path.display());

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
                        eprintln!("valetd: idle timeout, locked all users");
                    }
                }
            }
        });
    }

    loop {
        let (conn, _) = match listener.accept().await {
            Ok(x) => x,
            Err(err) => {
                eprintln!("valetd: accept failed: {err}");
                continue;
            }
        };
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(err) = serve(conn, state).await {
                eprintln!("valetd: connection ended: {err}");
            }
        });
    }
}

async fn serve(mut conn: UnixStream, state: Arc<Mutex<State>>) -> std::io::Result<()> {
    loop {
        let req = match Request::recv_async(&mut conn).await {
            Ok(r) => r,
            // Clean EOF when the client closes the socket.
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(e),
        };

        let response = match dispatch(&state, req).await {
            Ok(r) => {
                state.lock().await.touch();
                r
            }
            Err(msg) => Response::Error(msg),
        };
        response.send_async(&mut conn).await?;
    }
}

async fn dispatch(state: &Arc<Mutex<State>>, req: Request) -> Result<Response, String> {
    match req {
        Request::Status => {
            let st = state.lock().await;
            let mut users: Vec<String> = st.users.keys().cloned().collect();
            users.sort();
            Ok(Response::Users(users))
        }
        Request::ListUsers => {
            let st = state.lock().await;
            let users = User::list(&st.db).await.map_err(err)?;
            Ok(Response::Users(users))
        }
        Request::Unlock { username, password } => {
            let mut st = state.lock().await;
            match User::load(&st.db, &username, password).await {
                Ok(user) => {
                    st.users.insert(username, user);
                    Ok(Response::Ok)
                }
                Err(e) => {
                    drop(st);
                    tokio::time::sleep(Duration::from_millis(FAILED_UNLOCK_DELAY_MS)).await;
                    Err(err(e))
                }
            }
        }
        Request::Lock { username } => {
            state.lock().await.drop_user(&username);
            Ok(Response::Ok)
        }
        Request::LockAll => {
            state.lock().await.drop_all();
            Ok(Response::Ok)
        }
        Request::List { username, queries } => list(state, &username, &queries).await,
        Request::Fetch { username, uuid } => fetch_any_lot(state, &username, &uuid).await,
        Request::FindRecords {
            username,
            lot,
            query,
        } => find_records(state, &username, &lot, &query).await,
        Request::GetRecord {
            username,
            lot,
            uuid,
        } => {
            let mut st = state.lock().await;
            ensure_lot(&mut st, &username, &lot).await?;
            let State { db, lots, .. } = &*st;
            let l = &lots[&(username, lot)];
            let record = Record::show(db, l, &uuid)
                .await
                .map_err(err)?
                .ok_or_else(|| "record not found".to_string())?;
            Ok(Response::Record(record))
        }
        Request::CreateRecord {
            username,
            lot,
            label,
            password,
            extra,
        } => create_record(state, username, lot, label, password, extra).await,
        Request::GenerateRecord {
            username,
            lot,
            label,
        } => {
            let password = Password::generate();
            let extra = HashMap::new();
            create_record(state, username, lot, label, password, extra).await
        }
    }
}

async fn list(
    state: &Arc<Mutex<State>>,
    username: &str,
    queries: &[String],
) -> Result<Response, String> {
    use valet::record::Query;
    let parsed = queries
        .iter()
        .map(|s| s.parse::<Query>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("invalid query: {e}"))?;

    let st = state.lock().await;
    let user_lots = {
        let user = st
            .users
            .get(username)
            .ok_or_else(|| format!("user '{username}' is locked"))?;
        user.lots(&st.db).await.map_err(err)?
    };

    let mut out: Vec<(Uuid<Record>, Label)> = Vec::new();
    for lot in &user_lots {
        let index = RecordIndex::load(&st.db, lot).await.map_err(err)?;
        for (label, uuid) in index.iter() {
            let keep = parsed.is_empty()
                || parsed
                    .iter()
                    .any(|q| q.matches_lot(lot.name()) && q.matches_label(label));
            if keep {
                out.push((uuid.clone(), label.clone()));
            }
        }
    }
    Ok(Response::Index(out))
}

async fn fetch_any_lot(
    state: &Arc<Mutex<State>>,
    username: &str,
    uuid: &Uuid<Record>,
) -> Result<Response, String> {
    let st = state.lock().await;
    let user_lots = {
        let user = st
            .users
            .get(username)
            .ok_or_else(|| format!("user '{username}' is locked"))?;
        user.lots(&st.db).await.map_err(err)?
    };
    for lot in &user_lots {
        if let Some(record) = Record::show(&st.db, lot, uuid).await.map_err(err)? {
            return Ok(Response::Record(record));
        }
    }
    Err(format!("no record with uuid {uuid}"))
}

async fn find_records(
    state: &Arc<Mutex<State>>,
    username: &str,
    lot: &str,
    query: &str,
) -> Result<Response, String> {
    let mut st = state.lock().await;
    ensure_lot(&mut st, username, lot).await?;
    let State { db, lots, .. } = &*st;
    let l = &lots[&(username.to_owned(), lot.to_owned())];
    let index = RecordIndex::load(db, l).await.map_err(err)?;
    let entries: Vec<(Uuid<Record>, Label)> = index
        .iter()
        .filter(|(label, _)| label_matches_domain(label, query))
        .map(|(label, uuid)| (uuid.clone(), label.clone()))
        .collect();
    Ok(Response::Index(entries))
}

async fn create_record(
    state: &Arc<Mutex<State>>,
    username: String,
    lot: String,
    label: Label,
    password: Password,
    extra: HashMap<String, String>,
) -> Result<Response, String> {
    let mut st = state.lock().await;
    ensure_lot(&mut st, &username, &lot).await?;

    let mut data = Data::new(password);
    if !extra.is_empty() {
        data = data.with_extra(extra);
    }
    // Separate the mutable borrow of the lot from the immutable db borrow.
    let State { db, lots, .. } = &mut *st;
    let l = lots
        .get_mut(&(username, lot))
        .expect("ensure_lot inserted it");
    let record = Record::new(l, label, data);
    record.save(db, l).await.map_err(err)?;
    Ok(Response::Record(record))
}

async fn ensure_lot(st: &mut State, username: &str, lot_name: &str) -> Result<(), String> {
    let key = (username.to_owned(), lot_name.to_owned());
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

fn err<E: std::fmt::Debug>(e: E) -> String {
    format!("{e:?}")
}
