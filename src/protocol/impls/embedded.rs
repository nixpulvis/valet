//! In-process handler: [`EmbeddedHandler`] owns a SQLite handle plus
//! the cache of unlocked [`User`] / [`Lot`] keys and answers
//! [`Request`]s directly. There is no wire protocol here; nothing
//! frames bytes over a socket.
//!
//! [`User`]: crate::user::User
//! [`Lot`]: crate::Lot

use crate::protocol::SendHandler;
use crate::protocol::message::{
    RemoteEntry, Request, Response, RevisionEntry, SyncOutcome, SyncReport,
};
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

/// Delay applied after a failed [`Request::Unlock`]. Makes credential
/// guessing noticeably slow without being user-visible on the success
/// path.
pub const FAILED_UNLOCK_DELAY_MS: u64 = 750;

/// Cached keys are dropped after this much wall-clock inactivity.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// How often the reaper checks the idle window.
pub const IDLE_CHECK_INTERVAL: Duration = Duration::from_secs(15);

/// Cache state shared between [`EmbeddedHandler`] and its idle
/// reaper. The SQLite handle plus per-user / per-lot key caches
/// currently held in memory.
pub struct State {
    pub db: Database,
    /// Filesystem root for per-lot bare repos. Each lot's storgit
    /// store lives at [`Lot::lot_path`] beneath this directory.
    pub data_dir: std::path::PathBuf,
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
    pub fn new(db: Database, data_dir: std::path::PathBuf) -> Self {
        Self {
            db,
            data_dir,
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

/// Public handler type. Owns the SQLite handle plus the in-memory
/// caches of unlocked users and their lots; dispatches every
/// [`Request`] against that state.
pub struct EmbeddedHandler {
    state: Arc<Mutex<State>>,
}

impl EmbeddedHandler {
    /// Build a handler around `db` and spawn the idle reaper on `rt`.
    /// Taking an explicit [`Handle`] makes the "must have a tokio
    /// runtime" requirement static: the caller cannot name this
    /// function without naming a live runtime.
    ///
    /// [`Handle`]: tokio::runtime::Handle
    pub fn new(
        db: Database,
        data_dir: std::path::PathBuf,
        rt: &tokio::runtime::Handle,
    ) -> Self {
        let state = Arc::new(Mutex::new(State::new(db, data_dir)));
        spawn_reaper(rt, state.clone(), IDLE_TIMEOUT, IDLE_CHECK_INTERVAL);
        Self { state }
    }

    /// Open the database under `$VALET_DIR` (or
    /// [`crate::db::default_dir`] when unset) and build a handler
    /// around it. Used by the `valetd` binary and by any transport
    /// that just wants the default location.
    pub async fn open_from_env(rt: &tokio::runtime::Handle) -> Result<Self, String> {
        let data_dir = std::env::var_os("VALET_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(crate::db::default_dir);
        let db = Database::open_dir(&data_dir)
            .await
            .map_err(|e| format!("failed to open database at {}: {e:?}", data_dir.display()))?;
        Ok(Self::new(db, data_dir, rt))
    }
}

impl SendHandler for EmbeddedHandler {
    async fn handle(&self, req: Request) -> io::Result<Response> {
        let kind: &'static str = (&req).into();
        let response = match dispatch(&self.state, req).await {
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
        self.state.lock().await.touch();
        Ok(response)
    }
}

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

fn spawn_reaper(
    rt: &tokio::runtime::Handle,
    state: Arc<Mutex<State>>,
    idle_timeout: Duration,
    check_interval: Duration,
) {
    rt.spawn(async move {
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
            let record = Record::show(l, &uuid)
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
            let mut lot = Lot::new(DEFAULT_LOT, &st.data_dir).map_err(err)?;
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
        Request::RemoteAdd {
            username,
            name,
            url,
            lots,
        } => remote_add(state, &username, &name, &url, &lots).await,
        Request::RemoteRemove {
            username,
            name,
            lots,
        } => remote_remove(state, &username, &name, &lots).await,
        Request::RemoteList { username, lots } => remote_list(state, &username, &lots).await,
        Request::Sync { username, lots } => sync(state, &username, &lots).await,
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
        if let Some(record) = Record::show(lot, uuid).await.map_err(err)? {
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
    let State { lots, .. } = &mut *st;
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
    record.save(l).await.map_err(err)?;
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
    let mut lot = Lot::new(lot_name, &st.data_dir).map_err(err)?;
    let user = st
        .users
        .get(username)
        .ok_or_else(|| format!("user '{username}' is locked"))?;
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
    let revisions = Record::history(l, uuid)
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

async fn remote_add(
    state: &Arc<Mutex<State>>,
    username: &str,
    name: &str,
    url: &str,
    lots: &[String],
) -> Result<Response, String> {
    use storgit::Distribute;
    if lots.is_empty() {
        return Err("remote add: at least one lot is required".to_string());
    }
    let mut st = state.lock().await;
    let lot_uuids: Vec<(String, Uuid<Lot>)> = lots
        .iter()
        .map(|name| lookup_lot_uuid(&st, username, name).map(|u| (name.clone(), u)))
        .collect::<Result<_, _>>()?;
    let mut out = Vec::with_capacity(lot_uuids.len());
    for (lot_name, lot_uuid) in lot_uuids {
        let resolved = resolve_remote_url(url, &lot_uuid);
        let State {
            db, users, lots, ..
        } = &mut *st;
        let user = users
            .get(username)
            .ok_or_else(|| format!("user '{username}' is locked"))?;
        let l = lots
            .get_mut(&lot_uuid)
            .ok_or_else(|| Error::LotCacheMiss(lot_uuid.clone()))?;
        let result: Result<(), String> = match l.store_mut().add_remote(name, &resolved) {
            Ok(()) => l.save(db, user).await.map_err(err).map(|_| ()),
            Err(e) => Err(err(e)),
        };
        if result.is_ok() {
            info!(
                user = %username,
                lot = %lot_name,
                remote = %name,
                url = %resolved,
                "remote added",
            );
        }
        out.push((lot_name, result));
    }
    Ok(Response::RemoteResults(out))
}

/// Resolve a `file://` URL the user typed at `remote add` into the
/// canonical bare-repo path storgit/git want. Other schemes pass
/// through.
///
/// Rules, first match wins:
/// 1. URL already ends in `/parent.git`: returned as-is. The caller
///    is being explicit; respect it.
/// 2. `<path>` looks like a valet data dir (has a `lots/` subdir or
///    a `valet.sqlite`): rewrite to
///    `<path>/lots/<our-lot-uuid>/repo/parent.git`. The target may
///    not exist yet; first push auto-inits it.
/// 3. Otherwise: rewrite to `<path>/parent.git`. Treats `<path>` as
///    a generic shared bare-remote location; first push auto-inits.
fn resolve_remote_url(url: &str, lot_uuid: &Uuid<Lot>) -> String {
    let Some(path) = url.strip_prefix("file://") else {
        return url.to_string();
    };
    let p = std::path::Path::new(path);

    if p.file_name() == Some(std::ffi::OsStr::new("parent.git")) {
        return url.to_string();
    }

    let looks_like_data_dir = p.join("lots").is_dir() || p.join("valet.sqlite").is_file();
    let target = if looks_like_data_dir {
        p.join("lots")
            .join(lot_uuid.to_string())
            .join("repo")
            .join("parent.git")
    } else {
        p.join("parent.git")
    };
    format!("file://{}", target.display())
}

async fn remote_remove(
    state: &Arc<Mutex<State>>,
    username: &str,
    name: &str,
    lots: &[String],
) -> Result<Response, String> {
    use storgit::Distribute;
    if lots.is_empty() {
        return Err("remote remove: at least one lot is required".to_string());
    }
    let mut st = state.lock().await;
    let lot_uuids: Vec<(String, Uuid<Lot>)> = lots
        .iter()
        .map(|name| lookup_lot_uuid(&st, username, name).map(|u| (name.clone(), u)))
        .collect::<Result<_, _>>()?;
    let mut out = Vec::with_capacity(lot_uuids.len());
    for (lot_name, lot_uuid) in lot_uuids {
        let State {
            db, users, lots, ..
        } = &mut *st;
        let user = users
            .get(username)
            .ok_or_else(|| format!("user '{username}' is locked"))?;
        let l = lots
            .get_mut(&lot_uuid)
            .ok_or_else(|| Error::LotCacheMiss(lot_uuid.clone()))?;
        let result: Result<(), String> = match l.store_mut().remove_remote(name) {
            Ok(()) => l.save(db, user).await.map_err(err).map(|_| ()),
            Err(e) => Err(err(e)),
        };
        if result.is_ok() {
            info!(user = %username, lot = %lot_name, remote = %name, "remote removed");
        }
        out.push((lot_name, result));
    }
    Ok(Response::RemoteResults(out))
}

async fn remote_list(
    state: &Arc<Mutex<State>>,
    username: &str,
    lots: &[String],
) -> Result<Response, String> {
    use storgit::Distribute;
    let st = state.lock().await;
    let target_lots = resolve_lots(&st, username, lots)?;
    let mut out = Vec::with_capacity(target_lots.len());
    for lot_name in target_lots {
        let lot_uuid = lookup_lot_uuid(&st, username, &lot_name)?;
        let l = st.get_lot(&lot_uuid)?;
        let remotes = l
            .store()
            .remotes()
            .map_err(err)?
            .into_iter()
            .map(|r| RemoteEntry {
                name: r.name,
                url: r.url,
            })
            .collect();
        out.push((lot_name, remotes));
    }
    Ok(Response::RemoteList(out))
}

async fn sync(
    state: &Arc<Mutex<State>>,
    username: &str,
    lots: &[String],
) -> Result<Response, String> {
    use storgit::{Distribute, MergeStatus};
    let mut st = state.lock().await;
    let target_lots = resolve_lots(&st, username, lots)?;
    let lot_uuids: Vec<(String, Uuid<Lot>)> = target_lots
        .iter()
        .map(|name| lookup_lot_uuid(&st, username, name).map(|u| (name.clone(), u)))
        .collect::<Result<_, _>>()?;
    let mut out: Vec<SyncOutcome> = Vec::new();
    for (lot_name, lot_uuid) in lot_uuids {
        // Snapshot the configured remote list once before pulling so
        // we don't iterate while the layout's git config is mutating.
        let remote_names: Vec<String> = {
            let l = st.get_lot(&lot_uuid)?;
            match l.store().remotes() {
                Ok(rs) => rs.into_iter().map(|r| r.name).collect(),
                Err(e) => {
                    out.push(SyncOutcome {
                        lot: lot_name.clone(),
                        remote: String::new(),
                        result: Err(format!("{e:?}")),
                    });
                    continue;
                }
            }
        };
        for remote in remote_names {
            let State {
                db, users, lots, ..
            } = &mut *st;
            let user = users
                .get(username)
                .ok_or_else(|| format!("user '{username}' is locked"))?;
            let l = lots
                .get_mut(&lot_uuid)
                .ok_or_else(|| Error::LotCacheMiss(lot_uuid.clone()))?;
            // Pull first; conflicts stop the round before any push.
            let pulled: Result<MergeStatus, String> =
                l.store_mut().pull(&remote).map_err(|e| format!("{e:?}"));
            let result: Result<SyncReport, String> = match pulled {
                Ok(MergeStatus::Clean(ffs)) => {
                    let advanced = ffs.len() as u32;
                    match l.save(db, user).await {
                        Err(e) => Err(format!("save after pull: {e:?}")),
                        Ok(_) => {
                            info!(user = %username, lot = %lot_name, remote = %remote, advanced, "sync pull clean");
                            let pushed = match l.store().push(&remote) {
                                Ok(()) => {
                                    info!(user = %username, lot = %lot_name, remote = %remote, "sync push ok");
                                    Ok(())
                                }
                                Err(e) => {
                                    warn!(user = %username, lot = %lot_name, remote = %remote, "sync push failed: {e}");
                                    Err(format!("{e}"))
                                }
                            };
                            Ok(SyncReport::Clean { advanced, pushed })
                        }
                    }
                }
                Ok(MergeStatus::Conflicted(progress)) => {
                    let conflicts: Vec<String> = progress
                        .conflicts()
                        .iter()
                        .map(|c| c.id.to_string())
                        .collect();
                    warn!(user = %username, lot = %lot_name, remote = %remote, count = conflicts.len(), "sync conflicted");
                    Ok(SyncReport::Conflicted { conflicts })
                }
                Err(e) => Err(e),
            };
            out.push(SyncOutcome {
                lot: lot_name.clone(),
                remote,
                result,
            });
        }
    }
    Ok(Response::SyncResults(out))
}

/// Resolve the user-supplied lot name list into the lot names to act
/// on. Empty input means "every lot the user has access to".
fn resolve_lots(st: &State, username: &str, lots: &[String]) -> Result<Vec<String>, String> {
    if !lots.is_empty() {
        return Ok(lots.to_vec());
    }
    let uuids = user_lot_uuids(st, username)?;
    uuids
        .iter()
        .map(|u| {
            st.get_lot(u)
                .map(|l| l.name().to_owned())
                .map_err(Into::into)
        })
        .collect()
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
    let lots = Lot::load_all(&st.db, user, &st.data_dir)
        .await
        .map_err(err)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_uuid() -> Uuid<Lot> {
        Uuid::<Lot>::parse("019f0000-0000-7000-8000-000000000abc").unwrap()
    }

    #[test]
    fn resolve_passes_through_non_file_urls() {
        let u = fake_uuid();
        assert_eq!(
            resolve_remote_url("ssh://host/path", &u),
            "ssh://host/path"
        );
        assert_eq!(
            resolve_remote_url("https://example.com/repo.git", &u),
            "https://example.com/repo.git"
        );
    }

    #[test]
    fn resolve_keeps_explicit_parent_git_suffix() {
        let u = fake_uuid();
        assert_eq!(
            resolve_remote_url("file:///tmp/x/parent.git", &u),
            "file:///tmp/x/parent.git"
        );
    }

    #[test]
    fn resolve_uses_lot_uuid_path_for_valet_data_dir() {
        let dir = tempfile::tempdir().unwrap();
        // Make the path look like a valet data dir.
        std::fs::create_dir(dir.path().join("lots")).unwrap();
        let u = fake_uuid();
        let url = format!("file://{}", dir.path().display());
        assert_eq!(
            resolve_remote_url(&url, &u),
            format!(
                "file://{}/lots/{}/repo/parent.git",
                dir.path().display(),
                u
            ),
        );
    }

    #[test]
    fn resolve_uses_lot_uuid_path_when_sqlite_present() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("valet.sqlite"), b"").unwrap();
        let u = fake_uuid();
        let url = format!("file://{}", dir.path().display());
        assert_eq!(
            resolve_remote_url(&url, &u),
            format!(
                "file://{}/lots/{}/repo/parent.git",
                dir.path().display(),
                u
            ),
        );
    }

    #[test]
    fn resolve_falls_back_to_parent_git_for_generic_path() {
        let dir = tempfile::tempdir().unwrap();
        // Empty dir, neither lots/ nor valet.sqlite.
        let u = fake_uuid();
        let url = format!("file://{}", dir.path().display());
        assert_eq!(
            resolve_remote_url(&url, &u),
            format!("file://{}/parent.git", dir.path().display()),
        );
    }
}
