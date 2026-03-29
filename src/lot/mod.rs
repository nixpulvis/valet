use crate::{
    db::{self, Database},
    encrypt::{self, Encrypted, Key},
    record::{self, Record},
    user::User,
    uuid::Uuid,
};
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
    // user: &'a User, TODO name and key are meaningless without a user.
    uuid: Uuid<Self>,
    name: String,
    key: Key<Self>,
}

impl Lot {
    pub fn new(name: &str) -> Self {
        Lot {
            uuid: Uuid::now(),
            name: name.into(),
            key: Key::generate(),
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

    /// Load this lot's records from the database.
    pub async fn records(&self, db: &Database) -> Result<Vec<Record>, Error> {
        Record::load_all(db, self).await.map_err(Into::into)
    }

    /// Save this lot to the database, detecting and handling key rotation.
    ///
    /// If the lot key has changed since the last save, all records are
    /// re-encrypted with the new key before the updated key is stored.
    pub async fn save(&self, db: &Database, user: &User) -> Result<Uuid<Self>, Error> {
        let uuid = self.uuid.to_string();
        let active = self::orm::ActiveModel {
            uuid: Unchanged(uuid.clone()),
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

        match existing_ul {
            None => {
                let encrypted = user.key().encrypt(self.key.as_bytes())?;
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
                let existing_key_bytes = user.key().decrypt(&existing_encrypted)?;
                let key_changed = existing_key_bytes != self.key.as_bytes();

                let mut active = existing.into_active_model();

                if name_changed {
                    active.name = Set(self.name.clone());
                }

                if key_changed {
                    let encrypted = user.key().encrypt(self.key.as_bytes())?;
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

    async fn reencrypt_records(
        &self,
        db: &Database,
        existing_key_bytes: &[u8],
    ) -> Result<(), Error> {
        let old_key = Key::from_bytes(&existing_key_bytes);
        let old_lot = Lot {
            uuid: self.uuid.clone(),
            name: self.name.clone(),
            key: old_key,
        };
        let records = Record::load_all(db, &old_lot).await?;
        for record in &records {
            record.upsert(db, self).await?;
        }
        Ok(())
    }

    /// Load a user's lot by name.
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

    fn decrypt_and_build(
        user: &User,
        model: self::orm::Model,
        ul: self::orm::user_lots::Model,
    ) -> Result<Lot, Error> {
        let encrypted = Encrypted {
            data: ul.data,
            nonce: ul.nonce,
        };
        let key_bytes = user.key().decrypt(&encrypted)?;
        Ok(Lot {
            uuid: Uuid::parse(&model.uuid)?,
            name: ul.name,
            key: Key::from_bytes(&key_bytes),
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
    Record(record::Error),
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

impl From<db::Error> for Error {
    fn from(err: db::Error) -> Self {
        Error::Database(err)
    }
}

impl From<sea_orm::DbErr> for Error {
    fn from(err: sea_orm::DbErr) -> Self {
        Error::Database(err.into())
    }
}

impl From<record::Error> for Error {
    fn from(err: record::Error) -> Self {
        Error::Record(err)
    }
}

#[cfg(feature = "orm")]
pub mod orm;
#[cfg(not(feature = "orm"))]
pub(crate) mod orm;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db::Database, pw, record::RecordData};

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
        let user = User::new("nixpulvis", pw!("password"))
            .expect("failed to make user")
            .register(&db)
            .await
            .expect("failed to register user");
        let lot_a = Lot::new("lot a");
        lot_a.save(&db, &user).await.expect("failed to save lot");
        Record::new(&lot_a, RecordData::plain("a", "1"))
            .upsert(&db, &lot_a)
            .await
            .expect("failed to upsert record");
        Record::new(&lot_a, RecordData::plain("b", "2"))
            .upsert(&db, &lot_a)
            .await
            .expect("failed to upsert record");

        let lot_b = Lot::load(&db, lot_a.name(), &user)
            .await
            .expect("failed to load lot")
            .expect("no lot");
        let records_a = lot_a.records(&db).await.expect("failed to load records");
        let records_b = lot_b.records(&db).await.expect("failed to load records");
        assert_eq!(records_a, records_b);
    }

    #[tokio::test]
    async fn create_load_all() {
        let db = Database::new("sqlite://:memory:")
            .await
            .expect("failed to create database");
        let user = User::new("nixpulvis", pw!("password"))
            .expect("failed to make user")
            .register(&db)
            .await
            .expect("failed to register user");
        let lot_a = Lot::new("lot a");
        lot_a.save(&db, &user).await.expect("failed to save lot");
        Record::new(&lot_a, RecordData::plain("a", "1"))
            .upsert(&db, &lot_a)
            .await
            .expect("failed to upsert record");
        let lot_b = Lot::new("lot b");
        lot_b.save(&db, &user).await.expect("failed to save lot");
        Record::new(&lot_b, RecordData::plain("b", "2"))
            .upsert(&db, &lot_b)
            .await
            .expect("failed to upsert record");

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
        let user = User::new("nixpulvis", pw!("password"))
            .expect("failed to make user")
            .register(&db)
            .await
            .expect("failed to register user");
        let lot = Lot::new("lot a");
        lot.save(&db, &user).await.expect("failed to save lot");
        let lot_key = get_user_lot_key(&db, &user, &lot).await;
        assert_eq!(lot.key().as_bytes(), lot_key.as_bytes());
    }

    #[tokio::test]
    async fn user_lot_update() {
        let db = Database::new("sqlite://:memory:")
            .await
            .expect("failed to create database");
        let user = User::new("nixpulvis", pw!("password"))
            .expect("failed to make user")
            .register(&db)
            .await
            .expect("failed to register user");
        let mut lot = Lot::new("lot a");
        lot.save(&db, &user).await.expect("failed to save lot");
        Record::new(&lot, RecordData::plain("a", "1"))
            .upsert(&db, &lot)
            .await
            .expect("failed to upsert record");
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
        let records = lot.records(&db).await.expect("failed to load records");
        assert_eq!(1, records.len());
        assert_eq!("a", records[0].data().label());
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
        Key::<Lot>::from_bytes(
            &user
                .key()
                .decrypt(&encrypted)
                .expect("failed to decrypted lot key"),
        )
    }
}
