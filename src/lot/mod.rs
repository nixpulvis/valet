#[cfg(feature = "db")]
use crate::encrypt::Encrypted;
#[cfg(feature = "db")]
use crate::{
    db::{self, Database},
    record::{self, RecordIndex},
    user::User,
};
use crate::{
    encrypt::{self, Key},
    uuid::Uuid,
};
#[cfg(feature = "db")]
use sea_orm::{ActiveValue::Set, IntoActiveModel, entity::prelude::*};
use std::fmt;
#[cfg(feature = "db")]
use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(feature = "db")]
use storgit::{Layout, SubmoduleLayout};

pub const DEFAULT_LOT: &str = "main";

/// An encrypted collection of secrets.
///
/// Each lot has its own _lot key_, i.e. [`Key<Lot>`] which is used to encrypt
/// all of the records within the lot. Users with access to a lot obtain the lot
/// key through the `user_lots` SQL table.
///
/// Example `user_lots` table:
///
/// | username | lot |    data    |   nonce    |
/// |----------|-----|------------|------------|
/// | Alice    | `a` | `tvuZQ1XS` | `6jLC3aP9` |
/// | Alice    | `b` | `LyZJM8GA` | `SCW2EWjc` |
/// | Bob      | `a` | `dWPiZfO9` | `oQ/2Y845` |
///
/// The lot keys they derive:
///
/// |  Key   | `Decrypt_A` is Alice's            | `Decrypt_B` is Bob's              |
/// |--------|-----------------------------------|-----------------------------------|
/// | `Ka`   | `= Decrypt_A(tvuZQ1XS, 6jLC3aP9)` | `= Decrypt_B(dWPiZfO9, oQ/2Y845)` |
/// | `Kb`   | `= Decrypt_A(LyZJM8GA, SCW2EWjc)` | N/A                               |
pub struct Lot {
    uuid: Uuid<Self>,
    name: String,
    key: Arc<Key<Self>>,
    /// Live storgit store for this lot. Backed by a persistent
    /// directory under the data root (see [`Lot::repo_path`]); the
    /// parent + per-module bare repos live there across sessions.
    #[cfg(feature = "db")]
    store: storgit::Store<SubmoduleLayout>,
    /// `<data_dir>/lots/<uuid>/`. Owns the storgit repo at
    /// [`Self::repo_subdir`].
    #[cfg(feature = "db")]
    lot_dir: PathBuf,
    /// Materialised label->uuid index for every live record in the
    /// lot. Built from the store's label cache on construction and
    /// kept in sync by [`Record::save`] / [`Record::delete`], which
    /// hold `&mut Lot` for the mutation.
    #[cfg(feature = "db")]
    index: RecordIndex,
}

impl PartialEq for Lot {
    fn eq(&self, other: &Self) -> bool {
        // Store identity is uuid + name + key. The live storgit handle
        // carries session-scoped state (dirty tracking) that is not
        // part of the lot's persisted identity.
        self.uuid == other.uuid && self.name == other.name && self.key == other.key
    }
}
impl Eq for Lot {}

impl Lot {
    /// Build a fresh, in-memory lot rooted at `data_dir`. The bare
    /// git repos are created under [`Self::lot_path`]; the lot key
    /// is generated and stays in memory until [`Self::save`] persists
    /// the wrapped key into `user_lots`.
    #[cfg(feature = "db")]
    pub fn new(name: &str, data_dir: &Path) -> Result<Self, Error> {
        let uuid = Uuid::now();
        let lot_dir = Self::lot_path(data_dir, &uuid);
        std::fs::create_dir_all(&lot_dir).map_err(|e| {
            Error::Record(record::Error::Storgit(storgit::Error::Io(e)))
        })?;
        let store = storgit::Store::<SubmoduleLayout>::new(Self::repo_subdir(&lot_dir))
            .map_err(|e| Error::Record(record::Error::Storgit(e)))?;
        Ok(Lot {
            uuid,
            name: name.into(),
            key: Arc::new(Key::generate()),
            store,
            lot_dir,
            index: RecordIndex::default(),
        })
    }

    /// Build an in-memory lot with no on-disk backing store. Used by
    /// the WASM/no-`db` build, which only needs the type to exist for
    /// data-model code paths and has no filesystem access.
    #[cfg(not(feature = "db"))]
    pub fn new(name: &str) -> Self {
        Lot {
            uuid: Uuid::now(),
            name: name.into(),
            key: Arc::new(Key::generate()),
        }
    }

    pub fn uuid(&self) -> &Uuid<Self> {
        &self.uuid
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn key(&self) -> &Key<Self> {
        &self.key
    }

    /// Per-lot directory under a `data_dir` root: `<data_dir>/lots/<uuid>/`.
    #[cfg(feature = "db")]
    pub fn lot_path(data_dir: &Path, uuid: &Uuid<Lot>) -> PathBuf {
        data_dir.join("lots").join(uuid.to_string())
    }

    /// Storgit repo subdirectory inside a lot dir.
    #[cfg(feature = "db")]
    fn repo_subdir(lot_dir: &Path) -> PathBuf {
        lot_dir.join("repo")
    }

    /// AAD for the `user_lots.data` ciphertext (the lot key wrapped under
    /// the user key). Username is part of the AAD because `user_lots` is
    /// per-user: each grant is scoped to a specific owner.
    #[cfg(feature = "db")]
    fn user_lot_aad(username: &str, uuid: &Uuid<Lot>) -> Vec<u8> {
        [
            b"l".as_slice(),
            username.as_bytes(),
            uuid.to_uuid().as_bytes(),
        ]
        .concat()
    }

    /// Immutable access to this lot's live storgit store.
    #[cfg(feature = "db")]
    pub(crate) fn store(&self) -> &storgit::Store<SubmoduleLayout> {
        &self.store
    }

    /// Mutable access to this lot's live storgit store.
    #[cfg(feature = "db")]
    pub(crate) fn store_mut(&mut self) -> &mut storgit::Store<SubmoduleLayout> {
        &mut self.store
    }

    /// The label->uuid index for this lot.
    #[cfg(feature = "db")]
    pub fn index(&self) -> &RecordIndex {
        &self.index
    }

    /// Mutable access to the index. Used by
    /// [`Record::save`](crate::record::Record::save) and
    /// [`Record::delete`](crate::record::Record::delete) to mirror a
    /// storgit put/archive into the index under the same `&mut Lot`
    /// borrow.
    #[cfg(feature = "db")]
    pub(crate) fn index_mut(&mut self) -> &mut RecordIndex {
        &mut self.index
    }

    /// Persist this lot's identity to SQLite and bind it to `user`.
    ///
    /// Flushes any pending parent commit so the on-disk repo reflects
    /// current state, upserts the `lots` row (uuid only) and the
    /// per-user `user_lots` row that wraps the lot key under the
    /// user's key. Only the lot name is mutable on an existing
    /// `user_lots` row; lot-key rotation is not supported.
    #[cfg(feature = "db")]
    pub async fn save(&mut self, db: &Database, user: &User) -> Result<Uuid<Self>, Error> {
        // Flush any pending parent commit so the on-disk repo is up
        // to date. The returned bytes are discarded; persistence is
        // by virtue of being on disk.
        let _ = self
            .store
            .bundle()
            .map_err(|e| Error::Record(record::Error::Storgit(e)))?;

        let uuid = self.uuid.to_string();
        // Upsert the bare lot row (FK target for user_lots).
        // `ON CONFLICT DO NOTHING` on a duplicate uuid is a no-op
        // we want to swallow, but only that one DbErr; any other
        // failure (closed connection, schema mismatch) must
        // propagate so we don't end up trying to insert a
        // user_lots row that's about to fail on the FK.
        let active = self::orm::ActiveModel {
            uuid: Set(uuid.clone()),
        };
        let on_conflict = sea_orm::sea_query::OnConflict::column(self::orm::Column::Uuid)
            .do_nothing()
            .to_owned();
        match self::orm::Entity::insert(active)
            .on_conflict(on_conflict)
            .exec(db.connection())
            .await
        {
            Ok(_) | Err(sea_orm::DbErr::RecordNotInserted) => {}
            Err(e) => return Err(e.into()),
        }

        // Load existing user_lot once to detect changes.
        let existing_ul =
            self::orm::user_lots::Entity::find_by_id((user.username().to_owned(), uuid.to_owned()))
                .one(db.connection())
                .await?;

        let aad = Lot::user_lot_aad(user.username(), &self.uuid);
        match existing_ul {
            None => {
                let encrypted = user.key().encrypt_with_aad(self.key.as_bytes(), &aad)?;
                let active = self::orm::user_lots::ActiveModel {
                    username: Set(user.username().into()),
                    lot_uuid: Set(uuid),
                    name: Set(self.name.clone()),
                    data: Set(encrypted.data),
                    nonce: Set(encrypted.nonce),
                };
                self::orm::user_lots::Entity::insert(active)
                    .exec(db.connection())
                    .await?;
            }
            Some(existing) => {
                // Only name changes are supported for existing rows.
                if existing.name != self.name {
                    let mut active = existing.into_active_model();
                    active.name = Set(self.name.clone());
                    active.update(db.connection()).await?;
                }
            }
        }

        Ok(self.uuid.clone())
    }

    /// Load a user's lot by name.
    #[cfg(feature = "db")]
    pub async fn load(
        db: &Database,
        name: &str,
        user: &User,
        data_dir: &Path,
    ) -> Result<Option<Self>, Error> {
        let ul = self::orm::user_lots::Entity::find()
            .filter(self::orm::user_lots::Column::Username.eq(user.username()))
            .filter(self::orm::user_lots::Column::Name.eq(name))
            .one(db.connection())
            .await?
            .ok_or(Error::MissingLotKey)?;
        Ok(Some(Self::build_from_user_lot(user, ul, data_dir)?))
    }

    /// Load a user's lots.
    #[cfg(feature = "db")]
    pub async fn load_all(
        db: &Database,
        user: &User,
        data_dir: &Path,
    ) -> Result<Vec<Self>, Error> {
        let uls = self::orm::user_lots::Entity::find()
            .filter(self::orm::user_lots::Column::Username.eq(user.username()))
            .all(db.connection())
            .await?;
        let mut lots = Vec::with_capacity(uls.len());
        for ul in uls {
            lots.push(Self::build_from_user_lot(user, ul, data_dir)?);
        }
        Ok(lots)
    }

    /// Delete this lot, cascading to user_lots in SQLite. The
    /// on-disk repo at [`Self::lot_dir`] is also removed.
    #[cfg(feature = "db")]
    pub async fn delete(self, db: &Database) -> Result<(), Error> {
        self::orm::Entity::delete_by_id(self.uuid.to_string())
            .exec(db.connection())
            .await?;
        // Remove the on-disk repo. Best effort: a missing dir is
        // fine, but propagate other errors so the caller sees a
        // partial-state failure.
        match std::fs::remove_dir_all(&self.lot_dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(Error::Record(record::Error::Storgit(storgit::Error::Io(e)))),
        }
        Ok(())
    }

    /// Decrypt the lot key from `ul`, open (or initialise) the
    /// on-disk repo at `<data_dir>/lots/<uuid>/repo`, and build a
    /// `Lot` around it. Used by [`Self::load`] and [`Self::load_all`].
    #[cfg(feature = "db")]
    fn build_from_user_lot(
        user: &User,
        ul: self::orm::user_lots::Model,
        data_dir: &Path,
    ) -> Result<Lot, Error> {
        let uuid = Uuid::<Lot>::parse(&ul.lot_uuid)?;
        let encrypted = Encrypted {
            data: ul.data,
            nonce: ul.nonce,
        };
        let aad = Lot::user_lot_aad(user.username(), &uuid);
        let key_bytes = user.key().decrypt_with_aad(&encrypted, &aad)?;
        let key = Arc::new(Key::<Lot>::from_bytes(&key_bytes));

        let lot_dir = Self::lot_path(data_dir, &uuid);
        let repo = Self::repo_subdir(&lot_dir);
        let store = if repo.exists() {
            storgit::Store::<SubmoduleLayout>::open(repo)
                .map_err(|e| Error::Record(record::Error::Storgit(e)))?
        } else {
            std::fs::create_dir_all(&lot_dir).map_err(|e| {
                Error::Record(record::Error::Storgit(storgit::Error::Io(e)))
            })?;
            storgit::Store::<SubmoduleLayout>::new(repo)
                .map_err(|e| Error::Record(record::Error::Storgit(e)))?
        };
        let index = RecordIndex::from_store(&store).map_err(Error::Record)?;
        Ok(Lot {
            uuid,
            name: ul.name,
            key,
            store,
            lot_dir,
            index,
        })
    }
}

impl fmt::Debug for Lot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Lot")
            .field("uuid", &self.uuid)
            .field("name", &self.name)
            .finish()
    }
}

#[derive(Debug)]
pub enum Error {
    MissingLotKey,
    Uuid(crate::uuid::Error),
    Encrypt(encrypt::Error),
    #[cfg(feature = "db")]
    Record(record::Error),
    #[cfg(feature = "db")]
    Database(db::Error),
}

impl From<crate::uuid::Error> for Error {
    fn from(err: crate::uuid::Error) -> Self {
        Error::Uuid(err)
    }
}

impl From<encrypt::Error> for Error {
    fn from(err: encrypt::Error) -> Self {
        Error::Encrypt(err)
    }
}

#[cfg(feature = "db")]
impl From<db::Error> for Error {
    fn from(err: db::Error) -> Self {
        Error::Database(err)
    }
}

#[cfg(feature = "db")]
impl From<sea_orm::DbErr> for Error {
    fn from(err: sea_orm::DbErr) -> Self {
        Error::Database(err.into())
    }
}

#[cfg(feature = "db")]
impl From<record::Error> for Error {
    fn from(err: record::Error) -> Self {
        Error::Record(err)
    }
}

#[cfg(all(feature = "db", feature = "orm"))]
pub mod orm;
#[cfg(all(feature = "db", not(feature = "orm")))]
pub(crate) mod orm;

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "db")]
    use crate::{
        db::Database,
        record::{Data, Label, Record},
    };

    #[cfg(feature = "db")]
    async fn fixture() -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("valet.sqlite");
        let url = format!("sqlite://{}?mode=rwc", db_path.display());
        let db = Database::new(&url).await.unwrap();
        (dir, db)
    }

    #[cfg(feature = "db")]
    #[tokio::test(flavor = "multi_thread")]
    async fn create_load() {
        let (dir, db) = fixture().await;
        let user = User::new("nixpulvis", "password".try_into().unwrap())
            .expect("failed to make user")
            .register(&db)
            .await
            .expect("failed to register user");
        let mut lot_a = Lot::new("lot a", dir.path()).unwrap();
        lot_a.save(&db, &user).await.expect("failed to save lot");
        Record::new(
            &lot_a,
            "a".parse::<Label>().unwrap(),
            Data::new("1".try_into().unwrap()),
        )
        .save(&mut lot_a)
        .await
        .expect("failed to save record");
        Record::new(
            &lot_a,
            "b".parse::<Label>().unwrap(),
            Data::new("2".try_into().unwrap()),
        )
        .save(&mut lot_a)
        .await
        .expect("failed to save record");

        let lot_b = Lot::load(&db, lot_a.name(), &user, dir.path())
            .await
            .expect("failed to load lot")
            .expect("no lot");
        let mut labels_a: Vec<_> = lot_a.index().labels().collect();
        let mut labels_b: Vec<_> = lot_b.index().labels().collect();
        labels_a.sort_by_key(|l| l.to_string());
        labels_b.sort_by_key(|l| l.to_string());
        assert_eq!(labels_a, labels_b);
    }

    #[cfg(feature = "db")]
    #[tokio::test(flavor = "multi_thread")]
    async fn remote_persists_across_reload() {
        use storgit::Distribute;

        let (dir, db) = fixture().await;
        let user = User::new("nixpulvis", "password".try_into().unwrap())
            .expect("failed to make user")
            .register(&db)
            .await
            .expect("failed to register user");
        let mut lot = Lot::new("lot a", dir.path()).unwrap();
        lot.save(&db, &user).await.expect("failed to save lot");

        lot.store_mut()
            .add_remote("origin", "file:///tmp/valet-remote-test")
            .expect("add_remote");
        lot.save(&db, &user).await.expect("failed to re-save lot");

        let reloaded = Lot::load(&db, lot.name(), &user, dir.path())
            .await
            .expect("failed to load lot")
            .expect("no lot");
        let remotes = reloaded.store().remotes().expect("list remotes");
        assert_eq!(remotes.len(), 1);
        assert_eq!(remotes[0].name, "origin");
        assert_eq!(remotes[0].url, "file:///tmp/valet-remote-test");
    }

    #[cfg(feature = "db")]
    #[tokio::test(flavor = "multi_thread")]
    async fn create_load_all() {
        let (dir, db) = fixture().await;
        let user = User::new("nixpulvis", "password".try_into().unwrap())
            .expect("failed to make user")
            .register(&db)
            .await
            .expect("failed to register user");
        let mut lot_a = Lot::new("lot a", dir.path()).unwrap();
        lot_a.save(&db, &user).await.expect("failed to save lot");
        Record::new(
            &lot_a,
            "a".parse::<Label>().unwrap(),
            Data::new("1".try_into().unwrap()),
        )
        .save(&mut lot_a)
        .await
        .expect("failed to save record");
        let mut lot_b = Lot::new("lot b", dir.path()).unwrap();
        lot_b.save(&db, &user).await.expect("failed to save lot");
        Record::new(
            &lot_b,
            "b".parse::<Label>().unwrap(),
            Data::new("2".try_into().unwrap()),
        )
        .save(&mut lot_b)
        .await
        .expect("failed to save record");

        let lots = Lot::load_all(&db, &user, dir.path())
            .await
            .expect("failed to load lots");
        assert_eq!(lots.len(), 2);
    }

    #[cfg(feature = "db")]
    #[tokio::test(flavor = "multi_thread")]
    async fn user_lot() {
        let (dir, db) = fixture().await;
        let user = User::new("nixpulvis", "password".try_into().unwrap())
            .expect("failed to make user")
            .register(&db)
            .await
            .expect("failed to register user");
        let mut lot = Lot::new("lot a", dir.path()).unwrap();
        lot.save(&db, &user).await.expect("failed to save lot");
        let lot_key = get_user_lot_key(&db, &user, &lot).await;
        assert_eq!(lot.key().as_bytes(), lot_key.as_bytes());
    }

    /// `Lot::load` should open the existing on-disk repo (rather
    /// than init a fresh one). Drop a sentinel file into the repo
    /// dir between save and reload; if `Store::open` is taken, the
    /// sentinel survives. If `Store::new` ran by mistake it would
    /// have refused to create over an existing dir, but the
    /// sentinel-survival check is the more direct assertion.
    #[cfg(feature = "db")]
    #[tokio::test(flavor = "multi_thread")]
    async fn load_reuses_existing_on_disk_repo() {
        let (dir, db) = fixture().await;
        let user = User::new("nixpulvis", "password".try_into().unwrap())
            .expect("failed to make user")
            .register(&db)
            .await
            .expect("failed to register user");
        let mut lot = Lot::new("lot a", dir.path()).unwrap();
        lot.save(&db, &user).await.expect("failed to save lot");
        let lot_dir = Lot::lot_path(dir.path(), lot.uuid());
        let sentinel = lot_dir.join("sentinel.txt");
        std::fs::write(&sentinel, b"hello").unwrap();
        drop(lot);

        let _reloaded = Lot::load(&db, "lot a", &user, dir.path())
            .await
            .expect("failed to reload lot")
            .expect("no lot");
        assert!(
            sentinel.exists(),
            "Lot::load wiped or replaced the on-disk repo dir at {lot_dir:?}",
        );
        assert_eq!(std::fs::read_to_string(&sentinel).unwrap(), "hello");
    }

    #[cfg(feature = "db")]
    #[tokio::test(flavor = "multi_thread")]
    async fn delete() {
        let (dir, db) = fixture().await;
        let user = User::new("nixpulvis", "password".try_into().unwrap())
            .expect("failed to make user")
            .register(&db)
            .await
            .expect("failed to register user");
        let mut lot = Lot::new("lot a", dir.path()).unwrap();
        lot.save(&db, &user).await.expect("failed to save lot");
        Record::new(
            &lot,
            "a".parse::<Label>().unwrap(),
            Data::new("1".try_into().unwrap()),
        )
        .save(&mut lot)
        .await
        .expect("failed to save record");
        let uuid = lot.uuid().clone();
        let lot_dir = Lot::lot_path(dir.path(), &uuid);
        assert!(
            lot_dir.exists(),
            "lot dir should exist before delete: {lot_dir:?}",
        );
        lot.delete(&db).await.expect("failed to delete lot");
        let lots = Lot::load_all(&db, &user, dir.path())
            .await
            .expect("failed to load lots");
        assert!(lots.is_empty());
        let user_lot = self::orm::user_lots::Entity::find_by_id((
            user.username().to_owned(),
            uuid.to_string(),
        ))
        .one(db.connection())
        .await
        .expect("failed to load user_lot");
        assert!(user_lot.is_none());
        assert!(
            !lot_dir.exists(),
            "lot dir should be removed after delete: {lot_dir:?}",
        );
    }

    /// Returns the lot key for a given user/lot as decrypted from the
    /// user_lots table.
    #[cfg(feature = "db")]
    async fn get_user_lot_key(db: &Database, user: &User, lot: &Lot) -> Key<Lot> {
        let ul = self::orm::user_lots::Entity::find_by_id((
            user.username().to_owned(),
            lot.uuid().to_string(),
        ))
        .one(db.connection())
        .await
        .expect("failed to select user lot key")
        .expect("missing lot key");
        let encrypted = Encrypted {
            data: ul.data,
            nonce: ul.nonce,
        };
        let aad = Lot::user_lot_aad(user.username(), lot.uuid());
        Key::<Lot>::from_bytes(
            &user
                .key()
                .decrypt_with_aad(&encrypted, &aad)
                .expect("failed to decrypted lot key"),
        )
    }
}
