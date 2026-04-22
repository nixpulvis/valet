//! Request handler and shared server plumbing.
//!
//! [`Handler`] is the one abstraction every transport speaks to: the
//! [`valetd`](crate) binary (Unix socket), the browser native-host shim
//! (stdio with native-messaging framing, either as a relay or with an
//! embedded [`DaemonHandler`]), the FFI client, and the in-process
//! [`Stub`] all call `handler.handle(req)` and let the implementation
//! decide whether that means hitting SQLite, forwarding bytes to another
//! process, or answering from a fixed in-memory set.
//!
//! [`Stub`]: crate::stub::Stub

use crate::request::{Request, Response};
use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::Instant;
use tracing::{info, warn};
use valet::{
    Lot, Record,
    db::Database,
    password::Password,
    record::{Data, Label},
    user::User,
    uuid::Uuid,
};

/// Delay applied after a failed [`Request::Unlock`]. Makes credential
/// guessing noticeably slow without being user-visible on the success path.
pub const FAILED_UNLOCK_DELAY_MS: u64 = 750;

/// Anything that can turn a [`Request`] into a [`Response`]. Implemented by
/// the real DB-backed daemon ([`DaemonHandler`]), the in-process fake
/// ([`crate::stub::Stub`]), and by byte-level relays that forward frames
/// without decoding.
///
/// The [`io::Result`] outer return is for transport failures (socket
/// dropped, disk full while writing, …); application-level failures —
/// locked user, record not found, bad query — are conveyed as
/// [`Response::Error`]. In-process backends (`DaemonHandler`, `Stub`) never
/// return `Err`; `Err` only comes from remote/relay impls.
pub trait Handler: Send + Sync {
    fn handle(
        &self,
        req: Request,
    ) -> impl std::future::Future<Output = io::Result<Response>> + Send;
}

/// Daemon-side state: the SQLite handle plus caches of currently unlocked
/// [`User`] and [`Lot`] keys. Held behind a mutex inside
/// [`DaemonHandler`] so multiple connections can share one cache.
pub struct State {
    pub db: Database,
    pub users: HashMap<String, User>,
    pub lots: HashMap<(String, String), Lot>,
    /// Set whenever there are unlocked users; cleared when state is dropped.
    pub last_activity: Option<Instant>,
}

impl State {
    pub fn new(db: Database) -> Self {
        Self {
            db,
            users: HashMap::new(),
            lots: HashMap::new(),
            last_activity: None,
        }
    }

    pub fn drop_user(&mut self, username: &str) {
        self.users.remove(username);
        self.lots.retain(|(u, _), _| u != username);
        if self.users.is_empty() {
            self.last_activity = None;
        }
    }

    pub fn drop_all(&mut self) {
        self.users.clear();
        self.lots.clear();
        self.last_activity = None;
    }

    pub fn touch(&mut self) {
        if !self.users.is_empty() {
            self.last_activity = Some(Instant::now());
        }
    }
}

/// Real server: owns the database and the unlocked-user cache. The mutex
/// lives inside so [`Handler::handle`] can take `&self` and still allow
/// multiple concurrent connections to share one cache.
/// Cached keys are dropped after this much wall-clock inactivity.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// How often the reaper checks the idle window.
pub const IDLE_CHECK_INTERVAL: Duration = Duration::from_secs(15);

pub struct DaemonHandler {
    state: Arc<Mutex<State>>,
}

impl DaemonHandler {
    /// Build a handler around `db` and spawn the idle reaper. Must be called
    /// from within a tokio runtime; the reaper is what guarantees cached keys
    /// are dropped after [`IDLE_TIMEOUT`] of inactivity, so construction and
    /// reaper-spawning are deliberately fused.
    pub fn new(db: Database) -> Arc<Self> {
        let handler = Arc::new(Self {
            state: Arc::new(Mutex::new(State::new(db))),
        });
        handler.spawn_reaper(IDLE_TIMEOUT, IDLE_CHECK_INTERVAL);
        handler
    }

    /// Open the database at `$VALET_DB` (or [`valet::db::default_url`] when
    /// unset) and build a handler around it. Used by the `valetd` binary
    /// and by any transport, such as the browser native-host's embedded
    /// mode, that just wants the default location.
    pub async fn from_env() -> Result<Arc<Self>, String> {
        let db_url = std::env::var("VALET_DB").unwrap_or_else(|_| valet::db::default_url());
        let db = Database::new(&db_url)
            .await
            .map_err(|e| format!("failed to open database at {db_url}: {e:?}"))?;
        Ok(Self::new(db))
    }

    /// Drop every cached user if the idle window has elapsed since the last
    /// request touched the state. Returns `true` when something was dropped,
    /// so the caller can log it.
    async fn reap_if_idle(&self, idle_timeout: Duration) -> bool {
        let mut state = self.state.lock().await;
        match state.last_activity {
            Some(last) if last.elapsed() >= idle_timeout => {
                state.drop_all();
                true
            }
            _ => false,
        }
    }

    /// Spawn a background task that calls [`reap_if_idle`] on `check_interval`,
    /// dropping cached keys after `idle_timeout` of inactivity.
    ///
    /// [`reap_if_idle`]: Self::reap_if_idle
    fn spawn_reaper(self: &Arc<Self>, idle_timeout: Duration, check_interval: Duration) {
        let handler = self.clone();
        tokio::spawn(async move {
            info!(
                idle_timeout_secs = idle_timeout.as_secs(),
                check_interval_secs = check_interval.as_secs(),
                "reaper started",
            );
            loop {
                tokio::time::sleep(check_interval).await;
                if handler.reap_if_idle(idle_timeout).await {
                    info!("idle timeout, locked all users");
                }
            }
        });
    }
}

impl Handler for DaemonHandler {
    async fn handle(&self, req: Request) -> io::Result<Response> {
        let kind: &'static str = (&req).into();
        let response = match dispatch(&self.state, req).await {
            Ok(r) => {
                self.state.lock().await.touch();
                let resp_kind: &'static str = (&r).into();
                info!(request = kind, response = resp_kind, "ok");
                r
            }
            Err(msg) => {
                warn!(request = kind, "error: {msg}");
                Response::Error(msg)
            }
        };
        Ok(response)
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
                    info!(user = %username, "unlocked");
                    st.users.insert(username, user);
                    Ok(Response::Ok)
                }
                Err(e) => {
                    drop(st);
                    tokio::time::sleep(Duration::from_millis(FAILED_UNLOCK_DELAY_MS)).await;
                    warn!(user = %username, "unlock failed");
                    Err(err(e))
                }
            }
        }
        Request::Lock { username } => {
            info!(user = %username, "locked");
            state.lock().await.drop_user(&username);
            Ok(Response::Ok)
        }
        Request::LockAll => {
            info!("locked all users");
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
        for (label, uuid) in lot.index().iter() {
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
    use crate::request::label_matches_domain;
    let mut st = state.lock().await;
    ensure_lot(&mut st, username, lot).await?;
    let State { lots, .. } = &*st;
    let l = &lots[&(username.to_owned(), lot.to_owned())];
    let entries: Vec<(Uuid<Record>, Label)> = l
        .index()
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
    let State { db, lots, .. } = &mut *st;
    let l = lots
        .get_mut(&(username.clone(), lot.clone()))
        .expect("ensure_lot inserted it");
    let record = Record::new(l, label, data);
    record.save(db, l).await.map_err(err)?;
    info!(user = %username, lot = %lot, uuid = %record.uuid(), "record saved");
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
