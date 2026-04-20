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

pub const DEFAULT_LOT: &'static str = "main";

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
#[derive(PartialEq, Eq)]
pub struct Lot {
    uuid: Uuid<Self>,
    name: String,
    key: Key<Self>,
    /// Decrypted storgit parent tarball bytes for this lot. An empty vec means
    /// "fresh store"; `storgit::Store::open` treats that as the signal to
    /// initialise a new parent on first snapshot.
    ///
    /// Kept in memory so `Record::save` / `Record::delete` can round-trip
    /// through `Store::open` / `Store::snapshot` without re-reading and
    /// re-decrypting `lots.store` for every operation. Writes update this
    /// field to reflect the snapshot just persisted.
    store: Vec<u8>,
}

impl Lot {
    pub fn new(name: &str) -> Self {
        Lot {
            uuid: Uuid::now(),
            name: name.into(),
            key: Key::generate(),
            store: Vec::new(),
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

    /// The decrypted storgit parent tarball bytes for this lot. Empty for a
    /// fresh lot that has never had a record written.
    #[cfg(feature = "db")]
    pub(crate) fn store_bytes(&self) -> &[u8] {
        &self.store
    }

    /// Replace the in-memory parent tarball after a storgit snapshot has been
    /// persisted to `lots.store`.
    #[cfg(feature = "db")]
    pub(crate) fn set_store_bytes(&mut self, bytes: Vec<u8>) {
        self.store = bytes;
    }

    /// Load the label-to-uuid index for this lot.
    ///
    /// The index decrypts only labels, leaving the password-bearing data
    /// column on disk. Pair this with [`Record::show`] to reveal exactly one
    /// password at a time.
    #[cfg(feature = "db")]
    pub async fn index(&self, db: &Database) -> Result<RecordIndex, Error> {
        RecordIndex::load(db, self).await.map_err(Into::into)
    }

    /// Save this lot to the database, detecting and handling key rotation.
    ///
    /// Writes the `lots` row (uuid + encrypted parent tarball), then the
    /// per-user `user_lots` row binding `user` to this lot. If the lot key
    /// has changed since the last save, all records are re-encrypted with
    /// the new key before the updated key is stored.
    #[cfg(feature = "db")]
    pub async fn save(&mut self, db: &Database, user: &User) -> Result<Uuid<Self>, Error> {
        let uuid = self.uuid.to_string();
        let initial_store = self.encrypt_store(&self.store)?;
        let active = self::orm::ActiveModel {
            uuid: Unchanged(uuid.clone()),
            store: Set(initial_store),
        };
        self::orm::Entity::insert(active)
            .on_conflict_do_nothing()
            .exec(db.connection())
            .await?;

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
                let name_changed = existing.name != self.name;

                // Detect key change by comparing the stored key with current self.key.
                // If the current key is different, we re-encrypt all of the records in
                // this lot under the new lot key.
                //
                // TODO: A mechanism for sharing the new lot key with other users will
                // be needed, similarly to how we need a way to share a lot in the first
                // place.
                let existing_encrypted = Encrypted {
                    data: existing.data.clone(),
                    nonce: existing.nonce.clone(),
                };
                let existing_key_bytes = user.key().decrypt_with_aad(&existing_encrypted, &aad)?;
                let key_changed = existing_key_bytes != self.key.as_bytes();

                let mut active = existing.into_active_model();

                if name_changed {
                    active.name = Set(self.name.clone());
                }

                if key_changed {
                    let encrypted = user.key().encrypt_with_aad(self.key.as_bytes(), &aad)?;
                    active.data = Set(encrypted.data);
                    active.nonce = Set(encrypted.nonce);

                    self.reencrypt_records(db, &existing_key_bytes).await?;
                }

                if name_changed || key_changed {
                    active.update(db.connection()).await?;
                }
            }
        }

        Ok(self.uuid.clone())
    }

    #[cfg(feature = "db")]
    async fn reencrypt_records(
        &mut self,
        db: &Database,
        existing_key_bytes: &[u8],
    ) -> Result<(), Error> {
        let old_key = Key::from_bytes(&existing_key_bytes);
        let old_lot = Lot {
            uuid: self.uuid.clone(),
            name: self.name.clone(),
            key: old_key,
            store: self.store.clone(),
        };
        let records = Record::load_all(db, &old_lot).await?;
        // Wipe the old storgit state: the existing parent tarball and every
        // module tarball were encrypted under the old key, and save has no
        // cheap way to re-wrap them. Clear the in-memory parent and the
        // `records` rows so each `save` below rebuilds a fresh history
        // under the new key. This does drop per-record commit history; the
        // plan explicitly trades that for the simpler re-key path.
        self.store = Vec::new();
        record::orm::Entity::delete_many()
            .filter(record::orm::Column::LotUuid.eq(self.uuid.to_string()))
            .exec(db.connection())
            .await?;
        // No records left to drive a snapshot, so rewrite `lots.store` here
        // under the new key (empty parent).
        if records.is_empty() {
            let store_packed = self.encrypt_store(&self.store)?;
            self::orm::Entity::update(self::orm::ActiveModel {
                uuid: Unchanged(self.uuid.to_string()),
                store: Set(store_packed),
            })
            .exec(db.connection())
            .await?;
        }
        for record in &records {
            record.save(db, self).await?;
        }
        Ok(())
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
            let lot = Self::decrypt_and_build(&user, model, ul)?;
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
                let lot = Self::decrypt_and_build(&user, model, ul)?;
                lots.push(lot);
            }
        }
        Ok(lots)
    }

    /// Delete this lot, cascading to records and user_lots.
    #[cfg(feature = "db")]
    pub async fn delete(&self, db: &Database) -> Result<(), Error> {
        self::orm::Entity::delete_by_id(self.uuid.to_string())
            .exec(db.connection())
            .await?;
        Ok(())
    }

    #[cfg(feature = "db")]
    fn decrypt_and_build(
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
        let key = Key::<Lot>::from_bytes(&key_bytes);
        let mut lot = Lot {
            uuid,
            name: ul.name,
            key,
            store: Vec::new(),
        };
        lot.store = lot.decrypt_store_bytes(&model.store)?;
        Ok(lot)
    }

    /// Encrypt the storgit parent tarball under this lot's key with the
    /// lot-scoped store AAD. Packed for the `lots.store` column.
    #[cfg(feature = "db")]
    pub(crate) fn encrypt_store(&self, parent_bytes: &[u8]) -> Result<Vec<u8>, encrypt::Error> {
        let aad = Lot::store_aad(&self.uuid);
        let encrypted = self.key.encrypt_with_aad(parent_bytes, &aad)?;
        Ok(encrypted.pack())
    }

    /// Inverse of [`Lot::encrypt_store`]. Round-trips an empty parent through
    /// an encrypt/decrypt pair.
    #[cfg(feature = "db")]
    pub(crate) fn decrypt_store_bytes(&self, packed: &[u8]) -> Result<Vec<u8>, encrypt::Error> {
        let aad = Lot::store_aad(&self.uuid);
        let encrypted = Encrypted::unpack(packed);
        self.key.decrypt_with_aad(&encrypted, &aad)
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
    use crate::{
        db::Database,
        record::{Data, Label},
    };

    #[test]
    fn new() {
        let lot = Lot::new("lot a");
        assert_eq!(36, lot.uuid.to_string().len());
    }

    #[tokio::test]
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
        let index_a = lot_a.index(&db).await.expect("failed to load index a");
        let index_b = lot_b.index(&db).await.expect("failed to load index b");
        let mut labels_a: Vec<_> = index_a.labels().collect();
        let mut labels_b: Vec<_> = index_b.labels().collect();
        labels_a.sort_by_key(|l| l.to_string());
        labels_b.sort_by_key(|l| l.to_string());
        assert_eq!(labels_a, labels_b);
    }

    #[tokio::test]
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

    #[tokio::test]
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

    #[tokio::test]
    async fn user_lot_update() {
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
        let record_uuid = Record::new(
            &lot,
            "a".parse::<Label>().unwrap(),
            Data::new("1".try_into().unwrap()),
        )
        .save(&db, &mut lot)
        .await
        .expect("failed to save record");
        let lot_key_a = get_user_lot_key(&db, &user, &lot).await;
        lot.key = Key::<Lot>::generate();
        // Update lot key, user_lot, and re-encrypt all records.
        lot.save(&db, &user).await.expect("failed to save lot");
        let lot_key_b = get_user_lot_key(&db, &user, &lot).await;
        assert_ne!(lot_key_a.as_bytes(), lot_key_b.as_bytes());
        // Ensure the records got re-encrypted and we can still access them.
        let lot = Lot::load(&db, lot.name(), &user)
            .await
            .expect("failed to load lot")
            .expect("no lot");
        let index = lot.index(&db).await.expect("failed to load index");
        assert_eq!(1, index.len());
        assert_eq!(
            Some(&record_uuid),
            index.find(&"a".parse::<Label>().unwrap()),
        );
        let record = Record::show(&db, &lot, &record_uuid)
            .await
            .expect("failed to show record")
            .expect("record missing");
        assert_eq!("1", record.password().to_string());
    }

    #[tokio::test]
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
        lot.delete(&db).await.expect("failed to delete lot");
        let lots = Lot::load_all(&db, &user)
            .await
            .expect("failed to load lots");
        assert!(lots.is_empty());
        // Stale lot handle: the row is gone, so the index load must error out
        // rather than silently return an empty index.
        assert!(matches!(
            lot.index(&db).await,
            Err(Error::Record(record::Error::MissingLot)),
        ));
        let user_lot = self::orm::user_lots::Entity::find_by_id((
            user.username().to_owned(),
            lot.uuid().to_string(),
        ))
        .one(db.connection())
        .await
        .expect("failed to load user_lot");
        assert!(user_lot.is_none());
    }

    /// Returns the lot key for a given user/lot as decrypted from the
    /// user_lots table.
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
