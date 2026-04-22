//! In-process protocol: [`Client<Embedded>`] owns a SQLite handle plus
//! the cache of unlocked [`User`] / [`Lot`] keys and answers
//! [`Request`]s directly. There is no [`Server<Embedded>`]; nothing
//! listens for an Embedded protocol.
//!
//! [`Client<Embedded>`]: crate::protocol::Client
//! [`Server<Embedded>`]: crate::protocol::Server
//! [`User`]: crate::user::User
//! [`Lot`]: crate::Lot

use crate::protocol::message::{Request, Response, RevisionEntry};
use crate::protocol::{Client, Handler, Never, Protocol};
use crate::{
    Lot, Record,
    db::Database,
    lot::DEFAULT_LOT,
    password::Password,
    record::{Data, Label},
    user::User,
    uuid::Uuid,
};
use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::Instant;
use tracing::{info, warn};

/// Dispatch-layer errors the embedded handler can raise internally.
/// Flattened into the [`Response::Error`] message string as it leaves
/// `handle`; kept as a typed enum here so lookup helpers can signal
/// specific failure modes without building ad-hoc format strings at
/// every call site.
#[derive(Debug)]
enum Error {
    /// A uuid present in `State::user_lots` had no matching entry in
    /// `State::lots`. A process-internal invariant violation rather
    /// than a caller-visible condition; reaching this is a bug.
    LotCacheMiss(Uuid<Lot>),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::LotCacheMiss(uuid) => write!(f, "lot '{uuid}' missing from cache"),
        }
    }
}

impl From<Error> for String {
    fn from(e: Error) -> Self {
        e.to_string()
    }
}

/// Wire-protocol marker for in-process dispatch against a local DB.
pub struct Embedded;

impl Protocol for Embedded {
    type Client = EmbeddedClient;
    type Server = Never;
}

/// Delay applied after a failed [`Request::Unlock`]. Makes credential
/// guessing noticeably slow without being user-visible on the success
/// path.
pub const FAILED_UNLOCK_DELAY_MS: u64 = 750;

/// Cached keys are dropped after this much wall-clock inactivity.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// How often the reaper checks the idle window.
pub const IDLE_CHECK_INTERVAL: Duration = Duration::from_secs(15);

/// Cache state shared between [`Client<Embedded>`] and its idle
/// reaper. The SQLite handle plus per-user / per-lot key caches
/// currently held in memory.
pub struct State {
    pub db: Database,
    pub users: HashMap<String, User>,
    /// Every lot any unlocked user can access, keyed by lot uuid.
    /// Shared lots (multiple users with access) occupy one slot, not
    /// one copy per user, so the live storgit store is shared too.
    pub lots: HashMap<Uuid<Lot>, Lot>,
    /// Per-user access list into [`lots`](Self::lots). Synced from
    /// SQLite at specific points (unlock, register, create_lot,
    /// delete_lot); an entry with an empty `Vec` is a populated
    /// no-lots state.
    pub user_lots: HashMap<String, Vec<Uuid<Lot>>>,
    /// Set whenever there are unlocked users; cleared when state is
    /// dropped.
    pub last_activity: Option<Instant>,
}

impl State {
    pub fn new(db: Database) -> Self {
        Self {
            db,
            users: HashMap::new(),
            lots: HashMap::new(),
            user_lots: HashMap::new(),
            last_activity: None,
        }
    }

    pub fn drop_user(&mut self, username: &str) {
        self.users.remove(username);
        self.user_lots.remove(username);
        self.reap_unreferenced_lots();
        if self.users.is_empty() {
            self.last_activity = None;
        }
    }

    pub fn drop_all(&mut self) {
        self.users.clear();
        self.user_lots.clear();
        self.lots.clear();
        self.last_activity = None;
    }

    /// Drop any lot no unlocked user references. Called after
    /// `drop_user` so a shared lot stays cached as long as at least
    /// one accessor is still unlocked.
    fn reap_unreferenced_lots(&mut self) {
        let mut live: std::collections::HashSet<Uuid<Lot>> = std::collections::HashSet::new();
        for uuids in self.user_lots.values() {
            for uuid in uuids {
                live.insert(uuid.clone());
            }
        }
        self.lots.retain(|uuid, _| live.contains(uuid));
    }

    pub fn touch(&mut self) {
        if !self.users.is_empty() {
            self.last_activity = Some(Instant::now());
        }
    }

    fn get_lot(&self, uuid: &Uuid<Lot>) -> Result<&Lot, Error> {
        self.lots
            .get(uuid)
            .ok_or_else(|| Error::LotCacheMiss(uuid.clone()))
    }

    /// Cache `lot` and record that `username` has access to it. Used
    /// after a successful SQLite write to keep `lots` and `user_lots`
    /// in sync at one call site.
    fn insert_lot(&mut self, username: &str, lot: Lot) {
        let uuid = lot.uuid().clone();
        self.lots.insert(uuid.clone(), lot);
        self.user_lots
            .entry(username.to_owned())
            .or_default()
            .push(uuid);
    }
}

/// State behind [`Client<Embedded>`]. Private; only accessible via the
/// typed client methods and the [`Handler`] impl.
pub struct EmbeddedClient {
    state: Arc<Mutex<State>>,
}

impl Client<Embedded> {
    /// Build a client around `db` and spawn the idle reaper. Must be
    /// called from within a tokio runtime; the reaper is what
    /// guarantees cached keys are dropped after [`IDLE_TIMEOUT`] of
    /// inactivity.
    pub fn new(db: Database) -> Self {
        let state = Arc::new(Mutex::new(State::new(db)));
        spawn_reaper(state.clone(), IDLE_TIMEOUT, IDLE_CHECK_INTERVAL);
        Self {
            inner: EmbeddedClient { state },
        }
    }

    /// Open the database at `$VALET_DB` (or [`crate::db::default_url`]
    /// when unset) and build a client around it. Used by the `valetd`
    /// binary and by any transport that just wants the default
    /// location.
    pub async fn open_from_env() -> Result<Self, String> {
        let db_url = std::env::var("VALET_DB").unwrap_or_else(|_| crate::db::default_url());
        let db = Database::new(&db_url)
            .await
            .map_err(|e| format!("failed to open database at {db_url}: {e:?}"))?;
        Ok(Self::new(db))
    }
}

impl Handler for Client<Embedded> {
    async fn handle(&self, req: Request) -> io::Result<Response> {
        let kind: &'static str = (&req).into();
        let response = match dispatch(&self.inner.state, req).await {
            Ok(r) => {
                let resp_kind: &'static str = (&r).into();
                info!(request = kind, response = resp_kind, "ok");
                r
            }
            Err(msg) => {
                warn!(request = kind, "error: {msg}");
                Response::Error(msg)
            }
        };
        // Any dispatch attempt counts as activity.
        self.inner.state.lock().await.touch();
        Ok(response)
    }
}

// No `Server<Embedded>` in scope: the `Server<P>` struct is gated on
// the wire protocols in `protocol/mod.rs`, so it doesn't even exist
// in builds that only have `protocol-embedded`.

/// Drop every cached user if the idle window has elapsed since the
/// last request touched the state. Returns `true` when something was
/// dropped, so the reaper can log it.
async fn reap_if_idle(state: &Arc<Mutex<State>>, idle_timeout: Duration) -> bool {
    let mut st = state.lock().await;
    match st.last_activity {
        Some(last) if last.elapsed() >= idle_timeout => {
            st.drop_all();
            true
        }
        _ => false,
    }
}

fn spawn_reaper(state: Arc<Mutex<State>>, idle_timeout: Duration, check_interval: Duration) {
    tokio::spawn(async move {
        info!(
            idle_timeout_secs = idle_timeout.as_secs(),
            check_interval_secs = check_interval.as_secs(),
            "reaper started",
        );
        loop {
            tokio::time::sleep(check_interval).await;
            if reap_if_idle(&state, idle_timeout).await {
                info!("idle timeout, locked all users");
            }
        }
    });
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
                    st.users.insert(username.clone(), user);
                    sync_user_lots(&mut st, &username).await?;
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
            let st = state.lock().await;
            let lot_uuid = lookup_lot_uuid(&st, &username, &lot)?;
            let l = st.get_lot(&lot_uuid)?;
            let db = &st.db;
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
        Request::Register { username, password } => {
            let mut st = state.lock().await;
            let user = User::new(&username, password)
                .map_err(err)?
                .register(&st.db)
                .await
                .map_err(err)?;
            let mut lot = Lot::new(DEFAULT_LOT);
            lot.save(&st.db, &user).await.map_err(err)?;
            // Leave the newly-registered user unlocked. The caller has
            // just proved the password; forcing a follow-up Unlock to
            // re-derive the key is redundant Argon2 work.
            st.insert_lot(&username, lot);
            st.users.insert(username.clone(), user);
            info!(user = %username, "registered and unlocked");
            Ok(Response::Ok)
        }
        Request::Validate { username, password } => {
            let st = state.lock().await;
            let user = match User::load(&st.db, &username, password).await {
                Ok(u) => u,
                Err(e) => {
                    drop(st);
                    tokio::time::sleep(Duration::from_millis(FAILED_UNLOCK_DELAY_MS)).await;
                    warn!(user = %username, "validate failed");
                    return Err(err(e));
                }
            };
            if user.validate() {
                Ok(Response::Ok)
            } else {
                Err("validation token mismatch".to_string())
            }
        }
        Request::ListLots { username } => list_lots(state, &username).await,
        Request::CreateLot { username, lot } => create_lot(state, &username, &lot).await,
        Request::DeleteLot { username, lot } => delete_lot(state, &username, &lot).await,
        Request::History {
            username,
            lot,
            uuid,
        } => history(state, &username, &lot, &uuid).await,
    }
}

async fn list(
    state: &Arc<Mutex<State>>,
    username: &str,
    queries: &[String],
) -> Result<Response, String> {
    use crate::record::Query;
    let parsed = queries
        .iter()
        .map(|s| s.parse::<Query>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("invalid query: {e}"))?;

    let st = state.lock().await;
    let lot_uuids = user_lot_uuids(&st, username)?;

    let mut out: Vec<(Uuid<Record>, Label)> = Vec::new();
    for lot_uuid in lot_uuids {
        let lot = st.get_lot(lot_uuid)?;
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
    let lot_uuids = user_lot_uuids(&st, username)?.to_vec();
    for lot_uuid in lot_uuids {
        let lot = st.get_lot(&lot_uuid)?;
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
    use crate::protocol::message::label_matches_domain;
    let st = state.lock().await;
    let lot_uuid = lookup_lot_uuid(&st, username, lot)?;
    let l = st.get_lot(&lot_uuid)?;
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
    let lot_uuid = lookup_lot_uuid(&st, &username, &lot)?;

    let mut data = Data::new(password);
    if !extra.is_empty() {
        data = data.with_extra(extra);
    }
    let State { db, lots, .. } = &mut *st;
    let l = lots
        .get_mut(&lot_uuid)
        .ok_or_else(|| Error::LotCacheMiss(lot_uuid.clone()))?;
    // Upsert: reuse the uuid already bound to this label name so
    // storgit extends that submodule's history, rather than minting a
    // fresh uuid on every save.
    let record = match l.index().find_by_name(label.name()).cloned() {
        Some(existing) => Record::with_uuid(existing, l, label, data),
        None => Record::new(l, label, data),
    };
    record.save(db, l).await.map_err(err)?;
    info!(user = %username, lot = %lot, uuid = %record.uuid(), "record saved");
    Ok(Response::Record(record))
}

async fn list_lots(state: &Arc<Mutex<State>>, username: &str) -> Result<Response, String> {
    let st = state.lock().await;
    let lot_uuids = user_lot_uuids(&st, username)?;
    let mut entries: Vec<(Uuid<Lot>, String)> = lot_uuids
        .iter()
        .map(|u| st.get_lot(u).map(|l| (u.clone(), l.name().to_owned())))
        .collect::<Result<_, _>>()?;
    entries.sort_by(|a, b| a.1.cmp(&b.1));
    Ok(Response::Lots(entries))
}

async fn create_lot(
    state: &Arc<Mutex<State>>,
    username: &str,
    lot_name: &str,
) -> Result<Response, String> {
    let mut st = state.lock().await;
    let user = st
        .users
        .get(username)
        .ok_or_else(|| format!("user '{username}' is locked"))?;
    let mut lot = Lot::new(lot_name);
    lot.save(&st.db, user).await.map_err(err)?;
    info!(user = %username, lot = %lot_name, "lot created");
    st.insert_lot(username, lot);
    Ok(Response::Ok)
}

async fn delete_lot(
    state: &Arc<Mutex<State>>,
    username: &str,
    lot_name: &str,
) -> Result<Response, String> {
    let mut st = state.lock().await;
    let lot_uuid = lookup_lot_uuid(&st, username, lot_name)?;
    // Lot rows cascade-delete in SQLite, so access is revoked for
    // every user regardless of who initiated. Mirror that: drop the
    // lot from every cache entry, not just this user's.
    for uuids in st.user_lots.values_mut() {
        uuids.retain(|u| u != &lot_uuid);
    }
    let lot = st
        .lots
        .remove(&lot_uuid)
        .ok_or_else(|| Error::LotCacheMiss(lot_uuid.clone()))?;
    lot.delete(&st.db).await.map_err(err)?;
    info!(user = %username, lot = %lot_name, "lot deleted");
    Ok(Response::Ok)
}

async fn history(
    state: &Arc<Mutex<State>>,
    username: &str,
    lot_name: &str,
    uuid: &Uuid<Record>,
) -> Result<Response, String> {
    let st = state.lock().await;
    let lot_uuid = lookup_lot_uuid(&st, username, lot_name)?;
    let l = st.get_lot(&lot_uuid)?;
    let revisions = Record::history(&st.db, l, uuid)
        .await
        .map_err(err)?
        .ok_or_else(|| format!("record '{uuid}' not found in lot '{lot_name}'"))?;
    let entries = revisions
        .into_iter()
        .map(|rev| {
            let time_millis = rev
                .time
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or_else(|e| -(e.duration().as_millis() as i64));
            let password = rev.data.password().clone();
            RevisionEntry {
                time_millis,
                label: rev.label,
                password,
            }
        })
        .collect();
    Ok(Response::History(entries))
}

/// Eager-sync the cached lot set for `username` against SQLite. Loads
/// every lot the user has access to, de-duplicates by uuid into
/// [`State::lots`], and records the user's uuid list in
/// [`State::user_lots`]. Called at the boundary events that change lot
/// membership (unlock, create_lot, delete_lot) rather than on every
/// cross-lot read.
async fn sync_user_lots(st: &mut State, username: &str) -> Result<(), String> {
    let user = st
        .users
        .get(username)
        .ok_or_else(|| format!("user '{username}' is locked"))?;
    let lots = Lot::load_all(&st.db, user).await.map_err(err)?;
    let mut uuids = Vec::with_capacity(lots.len());
    for lot in lots {
        let uuid = lot.uuid().clone();
        uuids.push(uuid.clone());
        // First user to load wins; subsequent identical lots (shared
        // across users) reuse the already-cached entry with its live
        // storgit store.
        st.lots.entry(uuid).or_insert(lot);
    }
    st.user_lots.insert(username.to_owned(), uuids);
    Ok(())
}

/// Look up the uuid of `lot_name` for `username`, reading only from
/// cache (no DB). Errors if the user is locked or the lot isn't in
/// their access list.
fn lookup_lot_uuid(st: &State, username: &str, lot_name: &str) -> Result<Uuid<Lot>, String> {
    let uuids = user_lot_uuids(st, username)?;
    for uuid in uuids {
        if st.get_lot(uuid)?.name() == lot_name {
            return Ok(uuid.clone());
        }
    }
    Err(format!("lot '{lot_name}' not found"))
}

/// Borrow the per-user uuid list. Errors if the user isn't unlocked.
fn user_lot_uuids<'a>(st: &'a State, username: &str) -> Result<&'a [Uuid<Lot>], String> {
    if !st.users.contains_key(username) {
        return Err(format!("user '{username}' is locked"));
    }
    Ok(st
        .user_lots
        .get(username)
        .map(|v| v.as_slice())
        .unwrap_or(&[]))
}

fn err<E: std::fmt::Debug>(e: E) -> String {
    format!("{e:?}")
}
