#[cfg(feature = "db")]
use crate::encrypt::Encrypted;
#[cfg(feature = "db")]
use crate::{
    db::{self, Database},
    record::{self, Record, RecordIndex},
    user::User,
};
use crate::{
    encrypt::{self, Key},
    uuid::Uuid,
};
#[cfg(feature = "db")]
use sea_orm::{
    ActiveValue::{Set, Unchanged},
    IntoActiveModel,
    entity::prelude::*,
};
use std::fmt;
use std::sync::Arc;
#[cfg(feature = "db")]
use storgit::{
    Layout, SubmoduleLayout,
    layout::submodule::{Bundle, ModuleFetcher, Modules},
};

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
    /// Shared so the fetcher closure installed on [`Lot::store`] can
    /// hold an [`Arc`] clone of the same live key - no byte copy of
    /// the secret, and one authoritative zeroize on final drop.
    key: Arc<Key<Self>>,
    /// Live storgit store for this lot. Opened once on
    /// [`Lot::decrypt_and_build`] (with a fetcher that decrypts
    /// `records.module` rows on demand under this lot's key) or
    /// initialised fresh on [`Lot::new`]. Reused across every record
    /// op against this lot so we pay the parent untar and gix open
    /// costs once per session instead of once per op.
    #[cfg(feature = "db")]
    store: storgit::Store<SubmoduleLayout>,
    /// Scratch dir that owns the storgit repo's on-disk location.
    /// Kept alongside `store` so the directory lives exactly as long
    /// as the `Lot`; dropping the lot removes the backing repo. A
    /// future milestone will replace this with a persistent
    /// caller-managed path so the repo survives across sessions.
    #[cfg(feature = "db")]
    _scratch: tempfile::TempDir,
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
        // carries session-scoped state (scratch dir, dirty tracking)
        // that is not part of the lot's persisted identity.
        self.uuid == other.uuid && self.name == other.name && self.key == other.key
    }
}
impl Eq for Lot {}

impl Lot {
    pub fn new(name: &str) -> Self {
        // TODO: plumb a fallible ctor so the scratch TempDir
        // failure path doesn't panic - exhausting inodes or
        // $TMPDIR being unwritable shouldn't kill the process,
        // but Lot::new has no Error return today.
        #[cfg(feature = "db")]
        let scratch = tempfile::Builder::new()
            .prefix("valet-lot-")
            .tempdir()
            .expect("scratch tempdir for lot");
        #[cfg(feature = "db")]
        let store = storgit::Store::<SubmoduleLayout>::new(scratch.path().join("repo"))
            .expect("fresh storgit store");
        Lot {
            uuid: Uuid::now(),
            name: name.into(),
            key: Arc::new(Key::generate()),
            #[cfg(feature = "db")]
            store,
            #[cfg(feature = "db")]
            _scratch: scratch,
            #[cfg(feature = "db")]
            index: RecordIndex::default(),
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

    /// AAD for the outer encryption wrapping the storgit parent tarball.
    /// Lot-scoped only: `lots.store` is one row per lot, shared by every
    /// user granted access, so no username belongs here.
    #[cfg(feature = "db")]
    pub(crate) fn store_aad(uuid: &Uuid<Lot>) -> Vec<u8> {
        [b"s".as_slice(), uuid.to_uuid().as_bytes()].concat()
    }

    /// Immutable access to this lot's live storgit store. Use for
    /// read-only operations: [`storgit::Store::get`],
    /// [`storgit::Store::list_labels`], [`storgit::Store::history`].
    /// On a miss the installed fetcher (see [`Lot::make_fetcher`])
    /// will decrypt the relevant `records.module` row, so callers
    /// don't need to push bytes in ahead of time.
    #[cfg(feature = "db")]
    pub(crate) fn store(&self) -> &storgit::Store<SubmoduleLayout> {
        &self.store
    }

    /// Mutable access to this lot's live storgit store. Use for
    /// mutating operations: [`storgit::Store::put`],
    /// [`storgit::Store::archive`], [`storgit::Store::snapshot`].
    #[cfg(feature = "db")]
    pub(crate) fn store_mut(&mut self) -> &mut storgit::Store<SubmoduleLayout> {
        &mut self.store
    }

    /// Build a [`ModuleFetcher`] that resolves module bytes
    /// for `lot_uuid` by looking them up in `records` under the given
    /// lot key. The fetcher is sync (storgit's interface is sync), so
    /// it bridges to the async DB via [`tokio::runtime::Handle::block_on`].
    /// Callers must therefore invoke any storgit op that may trigger
    /// it from inside [`tokio::task::spawn_blocking`] (or
    /// [`tokio::task::block_in_place`]); calling from a plain async
    /// context panics when `block_on` executes.
    ///
    /// The query filters on `lot_uuid`, so a record uuid claimed by a
    /// different lot simply doesn't appear - the fetcher returns
    /// `Ok(None)` and storgit's usual "live in parent / no backing
    /// bytes" check surfaces the inconsistency if it matters.
    #[cfg(feature = "db")]
    fn make_fetcher(db: Database, lot_key: Arc<Key<Lot>>, lot_uuid: Uuid<Lot>) -> ModuleFetcher {
        Arc::new(move |id: &storgit::EntryId| {
            let record_uuid = Uuid::<Record>::parse(id.as_str())
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync + 'static>)?;
            let handle = tokio::runtime::Handle::current();
            let lot_uuid_str = lot_uuid.to_string();
            let model = handle
                .block_on(async {
                    record::orm::Entity::find()
                        .filter(record::orm::Column::Uuid.eq(record_uuid.to_string()))
                        .filter(record::orm::Column::LotUuid.eq(lot_uuid_str))
                        .one(db.connection())
                        .await
                })
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync + 'static>)?;
            let Some(model) = model else {
                return Ok(None);
            };
            let aad = Record::module_aad(&record_uuid, &lot_uuid);
            let bytes = lot_key
                .decrypt_with_aad(&Encrypted::unpack(&model.module), &aad)
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync + 'static>)?;
            Ok(Some(bytes))
        })
    }

    /// The label->uuid index for this lot.
    ///
    /// Materialised once when the lot is loaded (from the storgit
    /// store's label cache, which covers every live record in the
    /// lot) and kept in sync as records are added or removed. No DB
    /// round trip, no ciphertext opened here - pair with
    /// [`Record::show`] to reveal one password at a time.
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

    /// Persist this lot and its binding to `user`.
    ///
    /// Upserts the `lots` row (uuid + encrypted parent tarball) when the
    /// live store has a dirty parent to flush, then writes or updates the
    /// per-user `user_lots` row binding `user` to this lot under the
    /// user's key. Only the lot name is mutable on an existing
    /// `user_lots` row; lot-key rotation is not supported.
    #[cfg(feature = "db")]
    pub async fn save(&mut self, db: &Database, user: &User) -> Result<Uuid<Self>, Error> {
        let uuid = self.uuid.to_string();
        // Persist whatever parent state the store currently has. A
        // fresh store bundles an empty-parent tarball (dirty on
        // open); a loaded store with no mutations returns an empty
        // parent and we skip the write. We upsert on conflict so a
        // dirty parent flushed through here overwrites the existing
        // row rather than being discarded.
        let parent_bytes = self
            .store
            .bundle()
            .map_err(|e| Error::Record(record::Error::Storgit(e)))?
            .parent;
        if !parent_bytes.is_empty() {
            let initial_store = self.encrypt_store(&parent_bytes)?;
            let active = self::orm::ActiveModel {
                uuid: Unchanged(uuid.clone()),
                store: Set(initial_store),
            };
            let on_conflict = sea_orm::sea_query::OnConflict::column(self::orm::Column::Uuid)
                .update_column(self::orm::Column::Store)
                .to_owned();
            self::orm::Entity::insert(active)
                .on_conflict(on_conflict)
                .exec(db.connection())
                .await?;
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
                //
                // TODO: lot-key rotation. Needs to re-encrypt the
                // parent tarball and every records.module row under
                // the new key, coordinate that with any other users
                // sharing the lot, and survive a crash mid-rotation.
                // The previous implementation did none of those
                // well, so it was removed rather than left as a
                // footgun.
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
    pub async fn load(db: &Database, name: &str, user: &User) -> Result<Option<Self>, Error> {
        let ul = self::orm::user_lots::Entity::find()
            .filter(self::orm::user_lots::Column::Username.eq(user.username()))
            .filter(self::orm::user_lots::Column::Name.eq(name))
            .one(db.connection())
            .await?
            .ok_or(Error::MissingLotKey)?;
        if let Some(model) = self::orm::Entity::find_by_id(&ul.lot_uuid)
            .one(db.connection())
            .await?
        {
            let lot = Self::decrypt_and_build(db, user, model, ul)?;
            Ok(Some(lot))
        } else {
            Ok(None)
        }
    }

    /// Load a user's lots.
    #[cfg(feature = "db")]
    pub async fn load_all(db: &Database, user: &User) -> Result<Vec<Self>, Error> {
        let uls = self::orm::user_lots::Entity::find()
            .filter(self::orm::user_lots::Column::Username.eq(user.username()))
            .all(db.connection())
            .await?;
        let mut lots = Vec::new();
        for ul in uls {
            if let Some(model) = self::orm::Entity::find_by_id(&ul.lot_uuid)
                .one(db.connection())
                .await?
            {
                let lot = Self::decrypt_and_build(db, user, model, ul)?;
                lots.push(lot);
            }
        }
        Ok(lots)
    }

    /// Delete this lot, cascading to records and user_lots.
    ///
    /// Consumes the handle so callers can't accidentally read its
    /// stale cached index after the row is gone.
    #[cfg(feature = "db")]
    pub async fn delete(self, db: &Database) -> Result<(), Error> {
        self::orm::Entity::delete_by_id(self.uuid.to_string())
            .exec(db.connection())
            .await?;
        Ok(())
    }

    #[cfg(feature = "db")]
    fn decrypt_and_build(
        db: &Database,
        user: &User,
        model: self::orm::Model,
        ul: self::orm::user_lots::Model,
    ) -> Result<Lot, Error> {
        let uuid = Uuid::<Lot>::parse(&model.uuid)?;
        let encrypted = Encrypted {
            data: ul.data,
            nonce: ul.nonce,
        };
        let aad = Lot::user_lot_aad(user.username(), &uuid);
        let key_bytes = user.key().decrypt_with_aad(&encrypted, &aad)?;
        let key = Arc::new(Key::<Lot>::from_bytes(&key_bytes));

        // Decrypt the parent tarball under the (just-derived) lot key.
        let store_aad = Lot::store_aad(&uuid);
        let parent_bytes = key.decrypt_with_aad(&Encrypted::unpack(&model.store), &store_aad)?;

        let fetcher = Lot::make_fetcher(db.clone(), key.clone(), uuid.clone());
        let scratch = tempfile::Builder::new()
            .prefix("valet-lot-")
            .tempdir()
            .map_err(|e| Error::Record(record::Error::Storgit(storgit::Error::Io(e))))?;
        let layout = SubmoduleLayout::new(scratch.path().join("repo"))
            .and_then(|l| {
                l.with_bundle(Bundle {
                    parent: parent_bytes,
                    modules: Modules::new(),
                    deleted: Vec::new(),
                })
            })
            .map(|l: SubmoduleLayout| l.with_fetcher(fetcher))
            .map_err(|e| Error::Record(record::Error::Storgit(e)))?;
        let store = storgit::Store { layout };
        let index = RecordIndex::from_store(&store).map_err(Error::Record)?;

        Ok(Lot {
            uuid,
            name: ul.name,
            key,
            store,
            _scratch: scratch,
            index,
        })
    }

    /// Encrypt the storgit parent tarball under this lot's key with the
    /// lot-scoped store AAD. Packed for the `lots.store` column.
    #[cfg(feature = "db")]
    pub(crate) fn encrypt_store(&self, parent_bytes: &[u8]) -> Result<Vec<u8>, encrypt::Error> {
        let aad = Lot::store_aad(&self.uuid);
        let encrypted = self.key.encrypt_with_aad(parent_bytes, &aad)?;
        Ok(encrypted.pack())
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
        record::{Data, Label},
    };

    #[test]
    fn new() {
        let lot = Lot::new("lot a");
        assert_eq!(36, lot.uuid.to_string().len());
    }

    #[cfg(feature = "db")]
    #[tokio::test(flavor = "multi_thread")]
    async fn create_load() {
        let db = Database::new("sqlite://:memory:")
            .await
            .expect("failed to create database");
        let user = User::new("nixpulvis", "password".try_into().unwrap())
            .expect("failed to make user")
            .register(&db)
            .await
            .expect("failed to register user");
        let mut lot_a = Lot::new("lot a");
        lot_a.save(&db, &user).await.expect("failed to save lot");
        Record::new(
            &lot_a,
            "a".parse::<Label>().unwrap(),
            Data::new("1".try_into().unwrap()),
        )
        .save(&db, &mut lot_a)
        .await
        .expect("failed to save record");
        Record::new(
            &lot_a,
            "b".parse::<Label>().unwrap(),
            Data::new("2".try_into().unwrap()),
        )
        .save(&db, &mut lot_a)
        .await
        .expect("failed to save record");

        let lot_b = Lot::load(&db, lot_a.name(), &user)
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
    async fn create_load_all() {
        let db = Database::new("sqlite://:memory:")
            .await
            .expect("failed to create database");
        let user = User::new("nixpulvis", "password".try_into().unwrap())
            .expect("failed to make user")
            .register(&db)
            .await
            .expect("failed to register user");
        let mut lot_a = Lot::new("lot a");
        lot_a.save(&db, &user).await.expect("failed to save lot");
        Record::new(
            &lot_a,
            "a".parse::<Label>().unwrap(),
            Data::new("1".try_into().unwrap()),
        )
        .save(&db, &mut lot_a)
        .await
        .expect("failed to save record");
        let mut lot_b = Lot::new("lot b");
        lot_b.save(&db, &user).await.expect("failed to save lot");
        Record::new(
            &lot_b,
            "b".parse::<Label>().unwrap(),
            Data::new("2".try_into().unwrap()),
        )
        .save(&db, &mut lot_b)
        .await
        .expect("failed to save record");

        let lots = Lot::load_all(&db, &user)
            .await
            .expect("failed to load lots");
        assert_eq!(lots, vec![lot_a, lot_b]);
    }

    #[cfg(feature = "db")]
    #[tokio::test(flavor = "multi_thread")]
    async fn user_lot() {
        let db = Database::new("sqlite://:memory:")
            .await
            .expect("failed to create database");
        let user = User::new("nixpulvis", "password".try_into().unwrap())
            .expect("failed to make user")
            .register(&db)
            .await
            .expect("failed to register user");
        let mut lot = Lot::new("lot a");
        lot.save(&db, &user).await.expect("failed to save lot");
        let lot_key = get_user_lot_key(&db, &user, &lot).await;
        assert_eq!(lot.key().as_bytes(), lot_key.as_bytes());
    }

    #[cfg(feature = "db")]
    #[tokio::test(flavor = "multi_thread")]
    async fn delete() {
        let db = Database::new("sqlite://:memory:")
            .await
            .expect("failed to create database");
        let user = User::new("nixpulvis", "password".try_into().unwrap())
            .expect("failed to make user")
            .register(&db)
            .await
            .expect("failed to register user");
        let mut lot = Lot::new("lot a");
        lot.save(&db, &user).await.expect("failed to save lot");
        Record::new(
            &lot,
            "a".parse::<Label>().unwrap(),
            Data::new("1".try_into().unwrap()),
        )
        .save(&db, &mut lot)
        .await
        .expect("failed to save record");
        let uuid = lot.uuid().to_string();
        lot.delete(&db).await.expect("failed to delete lot");
        let lots = Lot::load_all(&db, &user)
            .await
            .expect("failed to load lots");
        assert!(lots.is_empty());
        let user_lot = self::orm::user_lots::Entity::find_by_id((user.username().to_owned(), uuid))
            .one(db.connection())
            .await
            .expect("failed to load user_lot");
        assert!(user_lot.is_none());
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
