#[cfg(feature = "db")]
use crate::db::{self, Database};
#[cfg(feature = "db")]
use crate::encrypt::{Encrypted, Stash};
use crate::{encrypt, lot::Lot, password::Password, uuid::Uuid};
use bitcode::{Decode, Encode};
#[cfg(feature = "db")]
use sea_orm::{IntoActiveModel, TransactionTrait, entity::prelude::*, sea_query::OnConflict};
#[cfg(feature = "db")]
use std::collections::HashMap;
use std::fmt;

/// One historical revision of a record, produced by [`Record::history`].
///
/// Each live commit in the record's submodule contributes one entry with
/// the plaintext [`Label`] / [`Data`] recovered from that commit; tombstone
/// commits (written by [`Record::delete`]) are filtered out upstream.
#[cfg(feature = "db")]
#[derive(Debug)]
pub struct Revision {
    /// Commit timestamp as recorded by storgit.
    pub time: std::time::SystemTime,
    /// Git commit id for this revision.
    pub commit: storgit::CommitId,
    pub label: Label,
    pub data: Data,
}

/// Progress event emitted by [`Record::save_many`] so callers can report
/// on long bulk imports without polling.
///
/// Events fire in this order:
/// 1. [`LoadedRecords`](Self::LoadedRecords) after existing `records` rows
///    for the batch have been fetched from the DB.
/// 2. [`OpenedStore`](Self::OpenedStore) after the storgit store has been
///    opened on the lot's current parent + the decrypted existing modules.
/// 3. [`PutRecord`](Self::PutRecord) per record, as it's staged into the
///    store.
/// 4. [`Snapshot`](Self::Snapshot) once, after the single `snapshot()` call.
/// 5. [`SaveRecord`](Self::SaveRecord) once, after the batched insert of
///    every record's ciphertext commits.
/// 6. [`SaveLot`](Self::SaveLot) once, after the lot's parent tarball is
///    repersisted. Only fires if the snapshot advanced the parent.
#[cfg(feature = "db")]
pub enum SaveProgress<'a> {
    LoadedRecords,
    OpenedStore,
    PutRecord(&'a Record),
    Snapshot(&'a storgit::Snapshot),
    SaveRecord,
    SaveLot,
}

#[derive(Encode, Decode)]
pub struct Record {
    pub(crate) uuid: Uuid<Self>,
    pub(crate) lot_uuid: Uuid<Lot>,
    pub(crate) label: Label,
    pub(crate) data: Data,
}

impl Record {
    pub fn new(lot: &Lot, label: Label, data: Data) -> Self {
        Self::with_uuid(Uuid::now(), lot, label, data)
    }

    /// Construct a record with a caller-chosen UUID. Use this when updating
    /// an existing record (e.g. resolved via [`RecordIndex::find`]) so the
    /// subsequent [`Record::save`] appends to the submodule's commit
    /// history rather than starting a new one.
    pub fn with_uuid(uuid: Uuid<Self>, lot: &Lot, label: Label, data: Data) -> Self {
        Record {
            uuid,
            lot_uuid: lot.uuid().clone(),
            label,
            data,
        }
    }

    pub fn uuid(&self) -> &Uuid<Self> {
        &self.uuid
    }

    pub fn lot_uuid(&self) -> &Uuid<Lot> {
        &self.lot_uuid
    }

    pub fn data(&self) -> &Data {
        &self.data
    }

    pub fn label(&self) -> &Label {
        &self.label
    }

    pub fn password(&self) -> &Password {
        self.data.password()
    }

    #[cfg(feature = "db")]
    pub(crate) fn data_aad(record_uuid: &Uuid<Self>, lot_uuid: &Uuid<Lot>) -> Vec<u8> {
        [
            b"d".as_slice(),
            record_uuid.to_uuid().as_bytes(),
            lot_uuid.to_uuid().as_bytes(),
        ]
        .concat()
    }

    /// AAD for the outer encryption wrapping the storgit submodule tarball.
    /// The `b"m"` prefix domain-separates this from [`Record::data_aad`] so a
    /// module ciphertext cannot authenticate as a data ciphertext under the
    /// same lot key.
    #[cfg(feature = "db")]
    pub(crate) fn module_aad(record_uuid: &Uuid<Self>, lot_uuid: &Uuid<Lot>) -> Vec<u8> {
        [
            b"m".as_slice(),
            record_uuid.to_uuid().as_bytes(),
            lot_uuid.to_uuid().as_bytes(),
        ]
        .concat()
    }

    /// Convert a record UUID to the opaque `storgit::Id` used as the entry key
    /// inside a [`storgit::Store`]. The UUID string form is a valid id: no
    /// forbidden characters, no leading `.`, no `.git` suffix.
    #[cfg(feature = "db")]
    pub(crate) fn storgit_id(uuid: &Uuid<Self>) -> storgit::Id {
        storgit::Id::new(uuid.to_string()).expect("uuid string is a valid storgit id")
    }

    /// Save this record to the database and return its uuid.
    ///
    /// Updates the lot's in-memory parent tarball (`lot.store_bytes`) to
    /// reflect the new snapshot; callers holding an older `Lot` will see the
    /// fresh parent after this call returns.
    #[cfg(feature = "db")]
    pub async fn save(&self, db: &Database, lot: &mut Lot) -> Result<Uuid<Self>, Error> {
        // Fetch any existing module bytes for this record so we append to
        // the submodule's history rather than starting a fresh one.
        let existing = self::orm::Entity::find_by_id(self.uuid.to_string())
            .one(db.connection())
            .await?;
        let existing_module = if let Some(model) = existing {
            if model.lot_uuid != self.lot_uuid.to_string() {
                return Err(Error::LotMismatch {
                    expected: self.lot_uuid.clone(),
                    actual: Uuid::<Lot>::parse(&model.lot_uuid)?,
                });
            }
            Some(Record::decrypt_module(lot, &self.uuid, &model.module)?)
        } else {
            None
        };

        let (module_packed, new_parent) = self.encrypt_module(lot, existing_module)?;
        let model = self::orm::Model {
            uuid: self.uuid.to_string(),
            lot_uuid: self.lot_uuid.to_string(),
            module: module_packed,
        };
        let active = model.into_active_model();
        let on_conflict = OnConflict::column(self::orm::Column::Uuid)
            .update_columns([self::orm::Column::LotUuid, self::orm::Column::Module])
            .to_owned();

        // Atomic: records.module and lots.store must advance together, or the
        // parent's gitlinks drift from the submodule tarball and `show` / the
        // label cache read stale state on next load.
        let store_packed = new_parent
            .as_ref()
            .map(|bytes| lot.encrypt_store(bytes))
            .transpose()?;
        let txn = db.connection().begin().await?;
        self::orm::Entity::insert(active)
            .on_conflict(on_conflict)
            .exec_with_returning(&txn)
            .await?;
        if let Some(store_packed) = store_packed {
            crate::lot::orm::Entity::update(crate::lot::orm::ActiveModel {
                uuid: sea_orm::ActiveValue::Unchanged(self.lot_uuid.to_string()),
                store: sea_orm::ActiveValue::Set(store_packed),
            })
            .exec(&txn)
            .await?;
        }
        txn.commit().await?;

        if let Some(parent_bytes) = new_parent {
            lot.set_store_bytes(parent_bytes);
        }

        Ok(self.uuid.clone())
    }

    /// Save many records against a single lot with one storgit snapshot and
    /// one database transaction. Much faster than looping [`Record::save`]
    /// (which reopens the store and round-trips the DB per record), which
    /// matters for bulk imports.
    ///
    /// All records must belong to `lot`. Returns the uuids in the same order
    /// as `records`. The in-memory parent tarball on `lot` is refreshed when
    /// the snapshot advances it.
    ///
    /// `on_progress` fires at each [`SaveProgress`] milestone so callers
    /// can render progress on large imports. Pass `|_| {}` if you don't
    /// care. Final save events fire after the DB transaction commits.
    #[cfg(feature = "db")]
    pub async fn save_many(
        db: &Database,
        lot: &mut Lot,
        records: &[Record],
        mut on_progress: impl FnMut(SaveProgress<'_>),
    ) -> Result<Vec<Uuid<Self>>, Error> {
        if records.is_empty() {
            return Ok(Vec::new());
        }

        for record in records {
            if record.lot_uuid != *lot.uuid() {
                return Err(Error::LotMismatch {
                    expected: lot.uuid().clone(),
                    actual: record.lot_uuid.clone(),
                });
            }
        }

        // Fetch existing modules so we append to their history rather than
        // starting a fresh one.
        let uuid_strs: Vec<String> = records.iter().map(|r| r.uuid.to_string()).collect();
        let existing_models = self::orm::Entity::find()
            .filter(self::orm::Column::Uuid.is_in(uuid_strs))
            .all(db.connection())
            .await?;
        on_progress(SaveProgress::LoadedRecords);

        let mut modules: HashMap<storgit::Id, Vec<u8>> = HashMap::new();
        for model in existing_models {
            let uuid = Uuid::<Self>::parse(&model.uuid)?;
            let model_lot_uuid = Uuid::<Lot>::parse(&model.lot_uuid)?;
            if &model_lot_uuid != lot.uuid() {
                return Err(Error::LotMismatch {
                    expected: lot.uuid().clone(),
                    actual: model_lot_uuid,
                });
            }
            let bytes = Record::decrypt_module(lot, &uuid, &model.module)?;
            modules.insert(Record::storgit_id(&uuid), bytes);
        }

        let mut store = storgit::Store::open(storgit::Parts {
            parent: lot.store_bytes().to_vec(),
            modules,
        })
        .map_err(Error::Storgit)?;
        on_progress(SaveProgress::OpenedStore);

        for record in records {
            let id = Record::storgit_id(&record.uuid);
            let label_bytes = record.label.encode();
            let data_cipher = record
                .data
                .encrypt_with_aad(lot.key(), &Record::data_aad(&record.uuid, &record.lot_uuid))?;
            let data_bytes = data_cipher.pack();
            store
                .put(&id, Some(&label_bytes), Some(&data_bytes))
                .map_err(Error::Storgit)?;
            on_progress(SaveProgress::PutRecord(record));
        }

        let snap = store.snapshot().map_err(Error::Storgit)?;
        on_progress(SaveProgress::Snapshot(&snap));

        let mut active_models = Vec::with_capacity(records.len());
        for record in records {
            let id = Record::storgit_id(&record.uuid);
            let module_bytes = match snap.modules.get(&id) {
                Some(storgit::ModuleChange::Changed(bytes)) => bytes.clone(),
                _ => {
                    return Err(Error::Storgit(storgit::Error::Other(
                        "storgit snapshot missing module for put".into(),
                    )));
                }
            };
            let aad = Record::module_aad(&record.uuid, lot.uuid());
            let encrypted = lot.key().encrypt_with_aad(&module_bytes, &aad)?;
            active_models.push(
                self::orm::Model {
                    uuid: record.uuid.to_string(),
                    lot_uuid: record.lot_uuid.to_string(),
                    module: encrypted.pack(),
                }
                .into_active_model(),
            );
        }

        let store_packed = snap
            .parent
            .as_ref()
            .map(|bytes| lot.encrypt_store(bytes))
            .transpose()?;

        let on_conflict = OnConflict::column(self::orm::Column::Uuid)
            .update_columns([self::orm::Column::LotUuid, self::orm::Column::Module])
            .to_owned();
        let txn = db.connection().begin().await?;
        self::orm::Entity::insert_many(active_models)
            .on_conflict(on_conflict)
            .exec(&txn)
            .await?;
        if let Some(store_packed) = store_packed {
            crate::lot::orm::Entity::update(crate::lot::orm::ActiveModel {
                uuid: sea_orm::ActiveValue::Unchanged(lot.uuid().to_string()),
                store: sea_orm::ActiveValue::Set(store_packed),
            })
            .exec(&txn)
            .await?;
        }
        txn.commit().await?;
        on_progress(SaveProgress::SaveRecord);

        if let Some(parent_bytes) = snap.parent {
            on_progress(SaveProgress::SaveLot); // Was saved by the txn.commit
            lot.set_store_bytes(parent_bytes);
        }

        Ok(records.iter().map(|r| r.uuid.clone()).collect())
    }

    /// Delete this record from the database.
    ///
    /// The storgit submodule is archived (tombstone commit) inside the lot's
    /// store; the `records` row is then removed and the lot's in-memory
    /// parent is refreshed.
    #[cfg(feature = "db")]
    pub async fn delete(&self, db: &Database, lot: &mut Lot) -> Result<(), Error> {
        let id = Record::storgit_id(&self.uuid);

        let Some(model) = self::orm::Entity::find_by_id(self.uuid.to_string())
            .one(db.connection())
            .await?
        else {
            return Ok(());
        };
        if model.lot_uuid != self.lot_uuid.to_string() {
            return Err(Error::LotMismatch {
                expected: self.lot_uuid.clone(),
                actual: Uuid::<Lot>::parse(&model.lot_uuid)?,
            });
        }
        let module_bytes = Record::decrypt_module(lot, &self.uuid, &model.module)?;
        let mut modules = HashMap::new();
        modules.insert(id.clone(), module_bytes);

        let mut store = storgit::Store::open(storgit::Parts {
            parent: lot.store_bytes().to_vec(),
            modules,
        })
        .map_err(Error::Storgit)?;
        store.archive(&id).map_err(Error::Storgit)?;
        let snap = store.snapshot().map_err(Error::Storgit)?;

        // Atomic: dropping the records row and advancing lots.store must
        // land together, or on next load the parent's gitlink set disagrees
        // with the live rows.
        let store_packed = snap
            .parent
            .as_ref()
            .map(|bytes| lot.encrypt_store(bytes))
            .transpose()?;
        let txn = db.connection().begin().await?;
        self::orm::Entity::delete_by_id(self.uuid.to_string())
            .exec(&txn)
            .await?;
        if let Some(store_packed) = store_packed {
            crate::lot::orm::Entity::update(crate::lot::orm::ActiveModel {
                uuid: sea_orm::ActiveValue::Unchanged(self.lot_uuid.to_string()),
                store: sea_orm::ActiveValue::Set(store_packed),
            })
            .exec(&txn)
            .await?;
        }
        txn.commit().await?;

        if let Some(parent_bytes) = snap.parent {
            lot.set_store_bytes(parent_bytes);
        }

        Ok(())
    }

    /// Load a single record by UUID, decrypting both its label and its
    /// password-bearing data.
    ///
    /// This is the only public path that materializes a [`Password`]. Callers
    /// should reach this function exactly when they intend to expose the
    /// secret (e.g. copy-to-clipboard, reveal-in-UI, CLI `get`). Listing and
    /// searching should go through [`RecordIndex`] instead.
    #[cfg(feature = "db")]
    pub async fn show(db: &Database, lot: &Lot, uuid: &Uuid<Self>) -> Result<Option<Self>, Error> {
        let Some(model) = self::orm::Entity::find_by_id(uuid.to_string())
            .one(db.connection())
            .await?
        else {
            return Ok(None);
        };
        let lot_uuid = Uuid::<Lot>::parse(&model.lot_uuid)?;
        if &lot_uuid != lot.uuid() {
            return Ok(None);
        }

        let module_bytes = Record::decrypt_module(lot, uuid, &model.module)?;

        let mut store = storgit::Store::open(storgit::Parts {
            parent: lot.store_bytes().to_vec(),
            modules: HashMap::new(),
        })
        .map_err(Error::Storgit)?;
        let id = Record::storgit_id(uuid);
        store.load_module(id.clone(), module_bytes);
        let entry = store
            .get(&id)
            .map_err(Error::Storgit)?
            .ok_or_else(|| Error::Storgit(storgit::Error::Other("entry missing".into())))?;

        let label_bytes = entry
            .label
            .ok_or_else(|| Error::Storgit(storgit::Error::Other("entry has no label".into())))?;
        let data_bytes = entry
            .data
            .ok_or_else(|| Error::Storgit(storgit::Error::Other("entry has no data".into())))?;

        let label = Label::decode(&label_bytes)?;
        let data_cipher = Encrypted::unpack(&data_bytes);
        let data =
            Data::decrypt_with_aad(&data_cipher, lot.key(), &Record::data_aad(uuid, &lot_uuid))?;

        Ok(Some(Record {
            uuid: uuid.clone(),
            lot_uuid,
            label,
            data,
        }))
    }

    /// Walk every historical revision of the record identified by `uuid`,
    /// newest commit first. Each live commit is decrypted into a
    /// [`Revision`]; tombstone commits (written by [`Record::delete`]) are
    /// skipped. Returns `None` if the `records.module` row is gone or
    /// belongs to a different lot.
    #[cfg(feature = "db")]
    pub async fn history(
        db: &Database,
        lot: &Lot,
        uuid: &Uuid<Self>,
    ) -> Result<Option<Vec<Revision>>, Error> {
        let Some(model) = self::orm::Entity::find_by_id(uuid.to_string())
            .one(db.connection())
            .await?
        else {
            return Ok(None);
        };
        let lot_uuid = Uuid::<Lot>::parse(&model.lot_uuid)?;
        if &lot_uuid != lot.uuid() {
            return Ok(None);
        }

        let module_bytes = Record::decrypt_module(lot, uuid, &model.module)?;

        let mut store = storgit::Store::open(storgit::Parts {
            parent: lot.store_bytes().to_vec(),
            modules: HashMap::new(),
        })
        .map_err(Error::Storgit)?;
        let id = Record::storgit_id(uuid);
        store.load_module(id.clone(), module_bytes);

        let entries = store.history(&id).map_err(Error::Storgit)?;
        let data_aad = Record::data_aad(uuid, &lot_uuid);
        let mut revisions = Vec::with_capacity(entries.len());
        for entry in entries {
            let (Some(label_bytes), Some(data_bytes)) = (entry.label, entry.data) else {
                // Tombstone commit (from `Record::delete` / `Store::archive`).
                continue;
            };
            let label = Label::decode(&label_bytes)?;
            let data =
                Data::decrypt_with_aad(&Encrypted::unpack(&data_bytes), lot.key(), &data_aad)?;
            revisions.push(Revision {
                time: entry.time,
                commit: entry.commit,
                label,
                data,
            });
        }
        Ok(Some(revisions))
    }

    /// Load every record in a lot with full decryption. Used internally for
    /// lot-key rotation; external consumers should use [`RecordIndex`] +
    /// [`Record::show`].
    #[cfg(feature = "db")]
    pub(crate) async fn load_all(db: &Database, lot: &Lot) -> Result<Vec<Self>, Error> {
        let models = self::orm::Entity::find()
            .filter(self::orm::Column::LotUuid.eq(lot.uuid().to_string()))
            .all(db.connection())
            .await?;

        let mut modules: HashMap<storgit::Id, Vec<u8>> = HashMap::new();
        let mut uuids: Vec<Uuid<Self>> = Vec::with_capacity(models.len());
        for model in &models {
            let uuid = Uuid::<Self>::parse(&model.uuid)?;
            let module_bytes = Record::decrypt_module(lot, &uuid, &model.module)?;
            modules.insert(Record::storgit_id(&uuid), module_bytes);
            uuids.push(uuid);
        }

        let store = storgit::Store::open(storgit::Parts {
            parent: lot.store_bytes().to_vec(),
            modules,
        })
        .map_err(Error::Storgit)?;

        let mut records = Vec::with_capacity(uuids.len());
        for uuid in uuids {
            let id = Record::storgit_id(&uuid);
            let entry = store
                .get(&id)
                .map_err(Error::Storgit)?
                .ok_or_else(|| Error::Storgit(storgit::Error::Other("entry missing".into())))?;
            let label_bytes = entry.label.ok_or_else(|| {
                Error::Storgit(storgit::Error::Other("entry has no label".into()))
            })?;
            let data_bytes = entry
                .data
                .ok_or_else(|| Error::Storgit(storgit::Error::Other("entry has no data".into())))?;
            let label = Label::decode(&label_bytes)?;
            let data_cipher = Encrypted::unpack(&data_bytes);
            let data = Data::decrypt_with_aad(
                &data_cipher,
                lot.key(),
                &Record::data_aad(&uuid, lot.uuid()),
            )?;
            records.push(Record {
                uuid,
                lot_uuid: lot.uuid().clone(),
                label,
                data,
            });
        }
        Ok(records)
    }

    /// Fold this record's current `label` + `data` into `lot`'s storgit
    /// store, encrypt the resulting submodule tarball under the lot key
    /// with the module-scoped AAD, and return the packed
    /// `nonce || ciphertext` ready for the `records.module` column.
    ///
    /// Pass `existing_module` as the decrypted module tarball from the
    /// prior `records.module` row if one exists, so history is extended
    /// rather than replaced. Returns the new parent tarball alongside the
    /// encrypted module when the snapshot updated the parent; the caller
    /// is responsible for encrypting that with [`Lot::encrypt_store`] and
    /// writing `lots.store`.
    #[cfg(feature = "db")]
    pub(crate) fn encrypt_module(
        &self,
        lot: &Lot,
        existing_module: Option<Vec<u8>>,
    ) -> Result<(Vec<u8>, Option<Vec<u8>>), Error> {
        let id = Record::storgit_id(&self.uuid);

        let mut modules: HashMap<storgit::Id, Vec<u8>> = HashMap::new();
        if let Some(bytes) = existing_module {
            modules.insert(id.clone(), bytes);
        }

        let mut store = storgit::Store::open(storgit::Parts {
            parent: lot.store_bytes().to_vec(),
            modules,
        })
        .map_err(Error::Storgit)?;

        let label_bytes = self.label.encode();
        let data_cipher = self
            .data
            .encrypt_with_aad(lot.key(), &Record::data_aad(&self.uuid, &self.lot_uuid))?;
        let data_bytes = data_cipher.pack();
        store
            .put(&id, Some(&label_bytes), Some(&data_bytes))
            .map_err(Error::Storgit)?;

        let snap = store.snapshot().map_err(Error::Storgit)?;
        // We just called `put`, which marks the module dirty; storgit must
        // return `Changed(bytes)` for `id` on the next snapshot. Anything
        // else is a storgit invariant violation, not a caller-visible
        // error. TODO: switch to `unreachable!` or a dedicated Error
        // variant instead of smuggling this through `Error::Storgit(Other)`.
        let module_bytes = match snap.modules.get(&id) {
            Some(storgit::ModuleChange::Changed(bytes)) => bytes.clone(),
            _ => {
                return Err(Error::Storgit(storgit::Error::Other(
                    "storgit snapshot missing module for put".into(),
                )));
            }
        };

        let aad = Record::module_aad(&self.uuid, lot.uuid());
        let encrypted = lot.key().encrypt_with_aad(&module_bytes, &aad)?;
        Ok((encrypted.pack(), snap.parent))
    }

    /// Decrypt a packed `records.module` blob back to the storgit submodule
    /// tarball bytes. Inverse of the encryption step inside
    /// [`Record::encrypt_module`]. Associated (not `&self`) so that callers
    /// like [`Record::show`] and [`Record::load_all`], which have only a
    /// record uuid before decryption, can use the same path as `save` /
    /// `delete` (which pass `&self.uuid`).
    #[cfg(feature = "db")]
    pub(crate) fn decrypt_module(
        lot: &Lot,
        record_uuid: &Uuid<Record>,
        packed: &[u8],
    ) -> Result<Vec<u8>, Error> {
        let aad = Record::module_aad(record_uuid, lot.uuid());
        Ok(lot
            .key()
            .decrypt_with_aad(&Encrypted::unpack(packed), &aad)?)
    }
}

impl PartialEq for Record {
    fn eq(&self, other: &Self) -> bool {
        self.uuid == other.uuid
            && self.lot_uuid == other.lot_uuid
            && self.label == other.label
            && self.data == other.data
    }
}
impl Eq for Record {}

impl fmt::Display for Record {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.label)
    }
}

impl fmt::Debug for Record {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Record")
            .field("uuid", &self.uuid)
            .field("lot", &self.lot_uuid)
            .field("label", &self.label)
            .field("data", &self.data)
            .finish()
    }
}

#[derive(Debug)]
pub enum Error {
    MissingLot,
    #[cfg(feature = "db")]
    LotMismatch {
        expected: Uuid<Lot>,
        actual: Uuid<Lot>,
    },
    Uuid(crate::uuid::Error),
    #[cfg(feature = "db")]
    Database(db::Error),
    Encryption(encrypt::Error),
    #[cfg(feature = "db")]
    Storgit(storgit::Error),
}

impl From<encrypt::Error> for Error {
    fn from(err: encrypt::Error) -> Self {
        Error::Encryption(err)
    }
}

impl From<crate::uuid::Error> for Error {
    fn from(err: crate::uuid::Error) -> Self {
        Error::Uuid(err)
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

mod data;
pub use self::data::Data;

pub(crate) mod label;
pub use self::label::{Label, LabelName};

#[cfg(feature = "db")]
mod index;
#[cfg(feature = "db")]
pub use self::index::RecordIndex;

pub mod query;
pub use self::query::{Path, Query};

#[cfg(all(feature = "db", feature = "orm"))]
pub mod orm;
#[cfg(all(feature = "db", not(feature = "orm")))]
pub(crate) mod orm;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lot::Lot;
    #[cfg(feature = "db")]
    use crate::{db::Database, user::User};

    #[test]
    fn new() {
        let lot = Lot::new("test");
        let record = Record::new(
            &lot,
            "foo".parse::<Label>().unwrap(),
            Data::new("bar".try_into().unwrap()),
        );
        assert_eq!(lot.uuid(), &record.lot_uuid);
        assert_eq!(36, record.uuid.to_string().len());
        assert_eq!(record.label(), &"foo".parse::<Label>().unwrap());
        assert_eq!(record.password().to_string(), "bar");
    }

    #[cfg(feature = "db")]
    #[test]
    fn module_and_data_aad_differ() {
        // Different domain-separation prefixes so a module ciphertext cannot
        // authenticate as a data ciphertext under the same key.
        let uuid = Uuid::<Record>::parse("00000000-0000-0000-0000-000000000001").unwrap();
        let lot_uuid = Uuid::<Lot>::parse("00000000-0000-0000-0000-000000000002").unwrap();
        assert_ne!(
            Record::module_aad(&uuid, &lot_uuid),
            Record::data_aad(&uuid, &lot_uuid),
        );
    }

    #[cfg(feature = "db")]
    #[tokio::test]
    async fn show_roundtrip() {
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
        let record = Record::new(
            &lot,
            "foo".parse::<Label>().unwrap(),
            Data::new("bar".try_into().unwrap()),
        );
        let uuid = record
            .save(&db, &mut lot)
            .await
            .expect("failed to save record");
        let loaded = Record::show(&db, &lot, &uuid)
            .await
            .expect("failed to show record")
            .expect("record missing");
        assert_eq!(loaded, record);
    }

    #[cfg(feature = "db")]
    #[tokio::test]
    async fn show_wrong_lot_returns_none() {
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
        let mut lot_b = Lot::new("lot b");
        lot_b.save(&db, &user).await.expect("failed to save lot");
        let uuid = Record::new(
            &lot_a,
            "foo".parse::<Label>().unwrap(),
            Data::new("bar".try_into().unwrap()),
        )
        .save(&db, &mut lot_a)
        .await
        .expect("failed to save record");
        assert!(
            Record::show(&db, &lot_b, &uuid)
                .await
                .expect("failed to show")
                .is_none()
        );
    }

    #[cfg(feature = "db")]
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
        let record = Record::new(
            &lot,
            "foo".parse::<Label>().unwrap(),
            Data::new("bar".try_into().unwrap()),
        );
        let uuid = record
            .save(&db, &mut lot)
            .await
            .expect("failed to save record");
        record
            .delete(&db, &mut lot)
            .await
            .expect("failed to delete record");
        assert!(
            Record::show(&db, &lot, &uuid)
                .await
                .expect("failed to show record")
                .is_none()
        );
    }

    #[cfg(feature = "db")]
    #[tokio::test]
    async fn save_many_roundtrip() {
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
        let parent_before = lot.store_bytes().to_vec();

        let records = vec![
            Record::new(
                &lot,
                "foo".parse::<Label>().unwrap(),
                Data::new("p1".try_into().unwrap()),
            ),
            Record::new(
                &lot,
                "bar".parse::<Label>().unwrap(),
                Data::new("p2".try_into().unwrap()),
            ),
            Record::new(
                &lot,
                "baz".parse::<Label>().unwrap(),
                Data::new("p3".try_into().unwrap()),
            ),
        ];

        let mut events: Vec<&'static str> = Vec::new();
        let uuids = Record::save_many(&db, &mut lot, &records, |ev| {
            events.push(match ev {
                SaveProgress::LoadedRecords => "loaded",
                SaveProgress::OpenedStore => "opened",
                SaveProgress::PutRecord(_) => "put",
                SaveProgress::Snapshot(_) => "snap",
                SaveProgress::SaveRecord => "save_r",
                SaveProgress::SaveLot => "save_l",
            });
        })
        .await
        .expect("failed to save_many");
        assert_eq!(uuids.len(), records.len());
        assert_eq!(
            events,
            vec![
                "loaded", "opened", "put", "put", "put", "snap", "save_r", "save_l"
            ]
        );

        // Lot parent tarball must advance so the new gitlinks are persisted.
        assert_ne!(lot.store_bytes(), parent_before.as_slice());

        for (record, uuid) in records.iter().zip(uuids.iter()) {
            assert_eq!(uuid, record.uuid());
            let loaded = Record::show(&db, &lot, uuid)
                .await
                .expect("failed to show record")
                .expect("record missing");
            assert_eq!(&loaded, record);
        }

        // Re-saving extends history rather than erroring on conflict.
        let updated = vec![Record::with_uuid(
            records[0].uuid().clone(),
            &lot,
            "foo".parse::<Label>().unwrap(),
            Data::new("p1-new".try_into().unwrap()),
        )];
        Record::save_many(&db, &mut lot, &updated, |_| {})
            .await
            .expect("failed to re-save");
        let loaded = Record::show(&db, &lot, records[0].uuid())
            .await
            .expect("failed to show record")
            .expect("record missing");
        assert_eq!(loaded.password().to_string(), "p1-new");
        let history = Record::history(&db, &lot, records[0].uuid())
            .await
            .expect("failed to read history")
            .expect("history missing");
        assert_eq!(history.len(), 2);
    }

    #[cfg(feature = "db")]
    #[tokio::test]
    async fn save_many_empty_is_noop() {
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
        let parent_before = lot.store_bytes().to_vec();
        let uuids = Record::save_many(&db, &mut lot, &[], |_| {})
            .await
            .expect("failed to save_many");
        assert!(uuids.is_empty());
        assert_eq!(lot.store_bytes(), parent_before.as_slice());
    }

    #[cfg(feature = "db")]
    #[tokio::test]
    async fn save_many_rejects_foreign_lot() {
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
        let mut lot_b = Lot::new("lot b");
        lot_b.save(&db, &user).await.expect("failed to save lot");
        let foreign = Record::new(
            &lot_b,
            "foo".parse::<Label>().unwrap(),
            Data::new("p".try_into().unwrap()),
        );
        let err = Record::save_many(&db, &mut lot_a, &[foreign], |_| {})
            .await
            .expect_err("expected LotMismatch");
        assert!(matches!(err, Error::LotMismatch { .. }));
    }
}
