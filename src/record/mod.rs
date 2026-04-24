#[cfg(feature = "db")]
use crate::db::{self, Database};
#[cfg(feature = "db")]
use crate::encrypt::{Encrypted, Stash};
use crate::{encrypt, lot::Lot, password::Password, uuid::Uuid};
#[cfg(feature = "db")]
use storgit::layout::submodule::{ModuleChange, Snapshot};
use bitcode::{Decode, Encode};
#[cfg(feature = "db")]
use sea_orm::{IntoActiveModel, TransactionTrait, entity::prelude::*, sea_query::OnConflict};
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
/// 1. [`OpenedStore`](Self::OpenedStore) after the lot's storgit store
///    is ready to accept puts (already live on the lot; this fires
///    once up front so callers can render a steady baseline).
/// 2. [`PutRecord`](Self::PutRecord) per record, as it's staged into the
///    store. Modules missing from storgit's scratch are lazily
///    decrypted via the lot's fetcher inside `put`.
/// 3. [`Snapshot`](Self::Snapshot) once, after the single `snapshot()` call.
/// 4. [`SaveRecord`](Self::SaveRecord) once, after the batched insert of
///    every record's ciphertext commits.
/// 5. [`SaveLot`](Self::SaveLot) once, after the lot's parent tarball is
///    repersisted. Only fires if the snapshot advanced the parent.
#[cfg(feature = "db")]
pub enum SaveProgress<'a> {
    OpenedStore,
    PutRecord(&'a Record),
    Snapshot(&'a Snapshot),
    SaveRecord,
    SaveLot,
}

/// What [`Record::save`] returns from the `block_in_place` closure
/// when it commits a single record: `(module_bytes, parent_bytes)`.
#[cfg(feature = "db")]
type SaveSingle = Option<(Vec<u8>, Option<Vec<u8>>)>;

/// What [`Record::save_many`] returns from the `block_in_place`
/// closure: `(active_models, changed_ids, new_parent)`.
#[cfg(feature = "db")]
type SaveBatch = (
    Vec<self::orm::ActiveModel>,
    std::collections::HashSet<storgit::Id>,
    Option<Vec<u8>>,
);

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
    #[cfg(feature = "db")]
    pub async fn save(&self, db: &Database, lot: &mut Lot) -> Result<Uuid<Self>, Error> {
        lot.index()
            .check_name_owner(self.label.name(), &self.uuid)?;

        // Integrity check: if a row already exists for this uuid
        // under a different lot, the INSERT ... ON CONFLICT below
        // would silently rewrite its lot_uuid and claim it for ours.
        // Bail cleanly instead.
        if let Some(existing) = self::orm::Entity::find_by_id(self.uuid.to_string())
            .one(db.connection())
            .await?
            && existing.lot_uuid != self.lot_uuid.to_string()
        {
            return Err(Error::LotMismatch {
                expected: self.lot_uuid.clone(),
                actual: Uuid::<Lot>::parse(&existing.lot_uuid)?,
            });
        }

        // Storgit work runs under `block_in_place`: put/snapshot are
        // synchronous and the fetcher may `Handle::block_on` the DB
        // for a module miss (append-to-history case), which is only
        // legal from a blocking-aware context. `put` returns Ok(None)
        // for a byte-identical no-op; we skip the DB round trip in
        // that case, trusting Store and records table stay in sync
        // through this path.
        let label_bytes = self.label.encode();
        let data_ciphertext = self
            .data
            .encrypt_with_aad(lot.key(), &Record::data_aad(&self.uuid, &self.lot_uuid))?;
        let data_bytes = data_ciphertext.pack();
        let storgit_id = Record::storgit_id(&self.uuid);
        let changed = tokio::task::block_in_place(|| -> Result<SaveSingle, Error> {
            let commit = lot
                .store_mut()
                .put(&storgit_id, Some(&label_bytes), Some(&data_bytes))
                .map_err(Error::Storgit)?;
            if commit.is_none() {
                return Ok(None);
            }
            // `put` returned Ok(Some(_)), so storgit marked the
            // module dirty; the next snapshot must carry it as
            // ModuleChange::Changed. Anything else is storgit
            // violating its own invariant.
            let snap = lot.store_mut().snapshot().map_err(Error::Storgit)?;
            let module_bytes = match snap.modules.get(&storgit_id) {
                Some(ModuleChange::Changed(bytes)) => bytes.clone(),
                other => unreachable!(
                    "storgit invariant: snapshot after put(Some) must yield Changed for {}; got {:?}",
                    storgit_id, other
                ),
            };
            let aad = Record::module_aad(&self.uuid, lot.uuid());
            let encrypted = lot.key().encrypt_with_aad(&module_bytes, &aad)?;
            Ok(Some((encrypted.pack(), snap.parent)))
        })?;

        let Some((module_packed, new_parent)) = changed else {
            return Ok(self.uuid.clone());
        };

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

        lot.index_mut()
            .insert(self.label.clone(), self.uuid.clone());

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

        let mut batch_names: std::collections::HashMap<&LabelName, &Uuid<Self>> =
            std::collections::HashMap::with_capacity(records.len());
        for record in records {
            if record.lot_uuid != *lot.uuid() {
                return Err(Error::LotMismatch {
                    expected: lot.uuid().clone(),
                    actual: record.lot_uuid.clone(),
                });
            }
            lot.index()
                .check_name_owner(record.label.name(), &record.uuid)?;
            if let Some(prior) = batch_names.insert(record.label.name(), &record.uuid)
                && prior != &record.uuid
            {
                return Err(Error::LabelCollision {
                    name: record.label.name().clone(),
                    existing: prior.clone(),
                    attempted: record.uuid.clone(),
                });
            }
        }

        // Integrity check: no row may claim any of these uuids under
        // a different lot. Scanning up front is cheaper than failing
        // part-way through the storgit work.
        let uuid_strs: Vec<String> = records.iter().map(|r| r.uuid.to_string()).collect();
        let existing_models = self::orm::Entity::find()
            .filter(self::orm::Column::Uuid.is_in(uuid_strs))
            .all(db.connection())
            .await?;
        for model in &existing_models {
            let model_lot_uuid = Uuid::<Lot>::parse(&model.lot_uuid)?;
            if &model_lot_uuid != lot.uuid() {
                return Err(Error::LotMismatch {
                    expected: lot.uuid().clone(),
                    actual: model_lot_uuid,
                });
            }
        }
        on_progress(SaveProgress::OpenedStore);

        // Encrypt each record's data outside `block_in_place` so the
        // sync section does only storgit work.
        struct Prepared {
            uuid: Uuid<Record>,
            storgit_id: storgit::Id,
            label_bytes: Vec<u8>,
            data_bytes: Vec<u8>,
        }
        let mut prepared = Vec::with_capacity(records.len());
        for record in records {
            let data_ciphertext = record
                .data
                .encrypt_with_aad(lot.key(), &Record::data_aad(&record.uuid, &record.lot_uuid))?;
            prepared.push(Prepared {
                uuid: record.uuid.clone(),
                storgit_id: Record::storgit_id(&record.uuid),
                label_bytes: record.label.encode(),
                data_bytes: data_ciphertext.pack(),
            });
        }

        // One snapshot for the whole batch. Misses for existing
        // modules go through the fetcher (decrypt under lot key); a
        // byte-identical put returns Ok(None) and contributes no
        // dirty module to the snapshot, so we skip persisting it.
        let (active_models, changed_ids, new_parent) = tokio::task::block_in_place(
            || -> Result<SaveBatch, Error> {
                for (rec, p) in records.iter().zip(&prepared) {
                    lot.store_mut()
                        .put(&p.storgit_id, Some(&p.label_bytes), Some(&p.data_bytes))
                        .map_err(Error::Storgit)?;
                    on_progress(SaveProgress::PutRecord(rec));
                }
                let snap = lot.store_mut().snapshot().map_err(Error::Storgit)?;
                on_progress(SaveProgress::Snapshot(&snap));

                let mut active_models = Vec::with_capacity(records.len());
                let mut changed_ids = std::collections::HashSet::with_capacity(records.len());
                for p in &prepared {
                    let Some(change) = snap.modules.get(&p.storgit_id) else {
                        // Byte-identical put; nothing to persist for
                        // this record.
                        continue;
                    };
                    let module_bytes = match change {
                        ModuleChange::Changed(bytes) => bytes,
                        other => unreachable!(
                            "storgit invariant: snapshot after put must yield Changed for {}; got {:?}",
                            p.storgit_id, other
                        ),
                    };
                    let aad = Record::module_aad(&p.uuid, lot.uuid());
                    let encrypted = lot.key().encrypt_with_aad(module_bytes, &aad)?;
                    active_models.push(
                        self::orm::Model {
                            uuid: p.uuid.to_string(),
                            lot_uuid: lot.uuid().to_string(),
                            module: encrypted.pack(),
                        }
                        .into_active_model(),
                    );
                    changed_ids.insert(p.storgit_id.clone());
                }
                Ok((active_models, changed_ids, snap.parent))
            },
        )?;

        let store_packed = new_parent
            .as_ref()
            .map(|bytes| lot.encrypt_store(bytes))
            .transpose()?;

        let on_conflict = OnConflict::column(self::orm::Column::Uuid)
            .update_columns([self::orm::Column::LotUuid, self::orm::Column::Module])
            .to_owned();
        let txn = db.connection().begin().await?;
        if !active_models.is_empty() {
            self::orm::Entity::insert_many(active_models)
                .on_conflict(on_conflict)
                .exec(&txn)
                .await?;
        }
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

        // Only records whose put marked the module dirty need an
        // index update; byte-identical repeats already have the same
        // (label, uuid) in the index from a prior save. Matches the
        // single `Record::save` path, which skips the insert on the
        // byte-identical early return.
        for (record, p) in records.iter().zip(&prepared) {
            if changed_ids.contains(&p.storgit_id) {
                lot.index_mut()
                    .insert(record.label.clone(), record.uuid.clone());
            }
        }

        if new_parent.is_some() {
            on_progress(SaveProgress::SaveLot);
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

        // Integrity check before we touch storgit: if the row
        // belongs to a different lot, don't archive in our store.
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

        // archive() needs the module on disk; storgit's fetcher will
        // pull it in on the ensure_loaded path inside archive. Wrap
        // in block_in_place for the fetcher's Handle::block_on.
        let new_parent = tokio::task::block_in_place(|| -> Result<Option<Vec<u8>>, Error> {
            lot.store_mut().archive(&id).map_err(Error::Storgit)?;
            let snap = lot.store_mut().snapshot().map_err(Error::Storgit)?;
            Ok(snap.parent)
        })?;

        // Atomic: dropping the records row and advancing lots.store must
        // land together, or on next load the parent's gitlink set disagrees
        // with the live rows.
        let store_packed = new_parent
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

        lot.index_mut().remove(&self.uuid);

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
        // Lot check: return None rather than decode a record that
        // doesn't belong to this lot (don't leak cross-lot state).
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

        // storgit's get consults the parent's gitlink set and opens
        // the module tree; ensure_loaded invokes the fetcher for
        // misses, which Handle::block_on's the DB. Hence
        // block_in_place.
        let id = Record::storgit_id(uuid);
        let entry = tokio::task::block_in_place(|| lot.store().get(&id)).map_err(Error::Storgit)?;
        let entry =
            entry.ok_or_else(|| Error::Storgit(storgit::Error::Other("entry missing".into())))?;

        let label_bytes = entry
            .label
            .ok_or_else(|| Error::Storgit(storgit::Error::Other("entry has no label".into())))?;
        let data_bytes = entry
            .data
            .ok_or_else(|| Error::Storgit(storgit::Error::Other("entry has no data".into())))?;

        let label = Label::decode(&label_bytes)?;
        let data_ciphertext = Encrypted::unpack(&data_bytes);
        let data = Data::decrypt_with_aad(
            &data_ciphertext,
            lot.key(),
            &Record::data_aad(uuid, &lot_uuid),
        )?;

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

        let id = Record::storgit_id(uuid);
        let entries =
            tokio::task::block_in_place(|| lot.store().history(&id)).map_err(Error::Storgit)?;

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
    #[cfg(feature = "db")]
    LotMismatch {
        expected: Uuid<Lot>,
        actual: Uuid<Lot>,
    },
    /// A different record already owns the label's name in this lot.
    /// Record identity within a lot is the [`LabelName`] alone; callers
    /// who want to update the existing record must reuse its uuid via
    /// [`Record::with_uuid`] (resolved through
    /// [`RecordIndex::find_by_name`]). Two records with the same name
    /// are unrepresentable in [`RecordIndex`].
    #[cfg(feature = "db")]
    LabelCollision {
        name: LabelName,
        existing: Uuid<Record>,
        attempted: Uuid<Record>,
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
    #[tokio::test(flavor = "multi_thread")]
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
    #[tokio::test(flavor = "multi_thread")]
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
    #[tokio::test(flavor = "multi_thread")]
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
            vec!["opened", "put", "put", "put", "snap", "save_r", "save_l"]
        );

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
    #[tokio::test(flavor = "multi_thread")]
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
        let uuids = Record::save_many(&db, &mut lot, &[], |_| {})
            .await
            .expect("failed to save_many");
        assert!(uuids.is_empty());
    }

    #[cfg(feature = "db")]
    #[tokio::test(flavor = "multi_thread")]
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

    #[cfg(feature = "db")]
    #[tokio::test(flavor = "multi_thread")]
    async fn save_rejects_name_collision_with_different_uuid() {
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
        let first = Record::new(
            &lot,
            "acct".parse::<Label>().unwrap(),
            Data::new("p1".try_into().unwrap()),
        );
        let first_uuid = first.save(&db, &mut lot).await.unwrap();

        // A fresh Record::new for the same name mints a new uuid and
        // must be rejected; the name is already owned.
        let collider = Record::new(
            &lot,
            "acct".parse::<Label>().unwrap(),
            Data::new("p2".try_into().unwrap()),
        );
        assert_ne!(&first_uuid, collider.uuid());
        let err = collider
            .save(&db, &mut lot)
            .await
            .expect_err("expected LabelCollision");
        assert!(matches!(
            err,
            Error::LabelCollision { ref existing, .. } if existing == &first_uuid
        ));

        // Reusing the existing uuid is the supported update path.
        Record::with_uuid(
            first_uuid.clone(),
            &lot,
            "acct".parse::<Label>().unwrap(),
            Data::new("p3".try_into().unwrap()),
        )
        .save(&db, &mut lot)
        .await
        .expect("reuse should succeed");
    }

    #[cfg(feature = "db")]
    #[tokio::test(flavor = "multi_thread")]
    async fn save_many_rejects_name_collision() {
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
            "acct".parse::<Label>().unwrap(),
            Data::new("p1".try_into().unwrap()),
        )
        .save(&db, &mut lot)
        .await
        .unwrap();

        let collider = Record::new(
            &lot,
            "acct".parse::<Label>().unwrap(),
            Data::new("p2".try_into().unwrap()),
        );
        let err = Record::save_many(&db, &mut lot, &[collider], |_| {})
            .await
            .expect_err("expected LabelCollision");
        assert!(matches!(err, Error::LabelCollision { .. }));
    }

    #[cfg(feature = "db")]
    #[tokio::test(flavor = "multi_thread")]
    async fn save_many_rejects_intra_batch_collision() {
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
        // Two fresh records minted with different uuids but the same
        // label name. Neither is in the index yet, so the per-record
        // check_name_owner passes for both; the intra-batch guard has
        // to catch it.
        let a = Record::new(
            &lot,
            "dup".parse::<Label>().unwrap(),
            Data::new("p1".try_into().unwrap()),
        );
        let b = Record::new(
            &lot,
            "dup".parse::<Label>().unwrap(),
            Data::new("p2".try_into().unwrap()),
        );
        assert_ne!(a.uuid(), b.uuid());
        let err = Record::save_many(&db, &mut lot, &[a, b], |_| {})
            .await
            .expect_err("expected LabelCollision");
        assert!(matches!(err, Error::LabelCollision { .. }));
    }
}
