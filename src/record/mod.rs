#[cfg(feature = "db")]
use crate::db;
#[cfg(feature = "db")]
use crate::encrypt::{Encrypted, Stash};
use crate::{encrypt, lot::Lot, password::Password, uuid::Uuid};
use bitcode::{Decode, Encode};
use std::fmt;
#[cfg(feature = "db")]
use storgit::Layout;

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
/// 1. [`OpenedStore`](Self::OpenedStore) once the lot's storgit store is
///    ready to accept puts.
/// 2. [`PutRecord`](Self::PutRecord) per record, as it's written into
///    the store.
/// 3. [`ParentFlushed`](Self::ParentFlushed) once, after the parent
///    commit is materialised.
/// 4. [`SaveRecord`](Self::SaveRecord) once, after every record is
///    persisted.
/// 5. [`SaveLot`](Self::SaveLot) once, only when at least one record
///    in the batch produced a new commit. Skipped on a fully
///    byte-identical batch.
#[cfg(feature = "db")]
pub enum SaveProgress<'a> {
    OpenedStore,
    PutRecord(&'a Record),
    ParentFlushed,
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

    /// AAD for the password ciphertext stored inside each storgit
    /// per-record commit's `data` blob. Bound to the record uuid +
    /// lot uuid so a ciphertext from one record cannot be replayed
    /// onto another within the same lot.
    #[cfg(feature = "db")]
    pub(crate) fn data_aad(record_uuid: &Uuid<Self>, lot_uuid: &Uuid<Lot>) -> Vec<u8> {
        [
            b"d".as_slice(),
            record_uuid.to_uuid().as_bytes(),
            lot_uuid.to_uuid().as_bytes(),
        ]
        .concat()
    }

    /// Convert a record UUID to the opaque `storgit::EntryId` used as the entry key
    /// inside a [`storgit::Store`]. The UUID string form is a valid id: no
    /// forbidden characters, no leading `.`, no `.git` suffix.
    #[cfg(feature = "db")]
    pub(crate) fn storgit_id(uuid: &Uuid<Self>) -> storgit::EntryId {
        storgit::EntryId::new(uuid.to_string()).expect("uuid string is a valid storgit id")
    }

    /// Persist this record into its lot's storgit store and return its
    /// uuid. The password is encrypted under the lot key + record-scoped
    /// AAD; the resulting ciphertext is what storgit stores in the
    /// per-record commit's `data` blob. Labels are stored as-is.
    #[cfg(feature = "db")]
    pub async fn save(&self, lot: &mut Lot) -> Result<Uuid<Self>, Error> {
        if self.lot_uuid != *lot.uuid() {
            return Err(Error::LotMismatch {
                expected: lot.uuid().clone(),
                actual: self.lot_uuid.clone(),
            });
        }
        lot.index()
            .check_name_owner(self.label.name(), &self.uuid)?;

        let label_bytes = self.label.encode();
        let data_ciphertext = self
            .data
            .encrypt_with_aad(lot.key(), &Record::data_aad(&self.uuid, &self.lot_uuid))?;
        let data_bytes = data_ciphertext.pack();
        let storgit_id = Record::storgit_id(&self.uuid);
        let changed = tokio::task::block_in_place(|| -> Result<bool, Error> {
            let commit = lot
                .store_mut()
                .put(&storgit_id, Some(&label_bytes), Some(&data_bytes))
                .map_err(Error::Storgit)?;
            // Flush the parent so the on-disk repo carries the new
            // gitlink + label-cache entry. Cheap when nothing's
            // dirty.
            let _ = lot.store_mut().bundle().map_err(Error::Storgit)?;
            Ok(commit.is_some())
        })?;

        if !changed {
            return Ok(self.uuid.clone());
        }

        lot.index_mut()
            .insert(self.label.clone(), self.uuid.clone());

        Ok(self.uuid.clone())
    }

    /// Save many records against a single lot in one storgit pass.
    /// Useful for bulk imports.
    ///
    /// All records must belong to `lot`. Returns the uuids in the same
    /// order as `records`. `on_progress` fires at each
    /// [`SaveProgress`] milestone so callers can render progress; pass
    /// `|_| {}` if you don't care.
    #[cfg(feature = "db")]
    pub async fn save_many(
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

        on_progress(SaveProgress::OpenedStore);

        struct Prepared {
            storgit_id: storgit::EntryId,
            label_bytes: Vec<u8>,
            data_bytes: Vec<u8>,
        }
        let mut prepared = Vec::with_capacity(records.len());
        for record in records {
            let data_ciphertext = record
                .data
                .encrypt_with_aad(lot.key(), &Record::data_aad(&record.uuid, &record.lot_uuid))?;
            prepared.push(Prepared {
                storgit_id: Record::storgit_id(&record.uuid),
                label_bytes: record.label.encode(),
                data_bytes: data_ciphertext.pack(),
            });
        }

        let changed_ids: std::collections::HashSet<storgit::EntryId> =
            tokio::task::block_in_place(
                || -> Result<std::collections::HashSet<storgit::EntryId>, Error> {
                    let mut changed = std::collections::HashSet::with_capacity(records.len());
                    for (rec, p) in records.iter().zip(&prepared) {
                        let commit = lot
                            .store_mut()
                            .put(&p.storgit_id, Some(&p.label_bytes), Some(&p.data_bytes))
                            .map_err(Error::Storgit)?;
                        if commit.is_some() {
                            changed.insert(p.storgit_id.clone());
                        }
                        on_progress(SaveProgress::PutRecord(rec));
                    }
                    // Flush the parent so the on-disk repo reflects
                    // every put in this batch as one parent commit.
                    let _ = lot.store_mut().bundle().map_err(Error::Storgit)?;
                    Ok(changed)
                },
            )?;
        on_progress(SaveProgress::ParentFlushed);
        on_progress(SaveProgress::SaveRecord);

        for (record, p) in records.iter().zip(&prepared) {
            if changed_ids.contains(&p.storgit_id) {
                lot.index_mut()
                    .insert(record.label.clone(), record.uuid.clone());
            }
        }

        if !changed_ids.is_empty() {
            on_progress(SaveProgress::SaveLot);
        }

        Ok(records.iter().map(|r| r.uuid.clone()).collect())
    }

    /// Archive this record in its lot's storgit store (tombstone
    /// commit, gitlink dropped from the parent) and remove it from
    /// the in-memory index.
    #[cfg(feature = "db")]
    pub async fn delete(&self, lot: &mut Lot) -> Result<(), Error> {
        if self.lot_uuid != *lot.uuid() {
            return Err(Error::LotMismatch {
                expected: lot.uuid().clone(),
                actual: self.lot_uuid.clone(),
            });
        }
        let id = Record::storgit_id(&self.uuid);
        tokio::task::block_in_place(|| -> Result<(), Error> {
            lot.store_mut().archive(&id).map_err(Error::Storgit)?;
            let _ = lot.store_mut().bundle().map_err(Error::Storgit)?;
            Ok(())
        })?;
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
    pub async fn show(lot: &Lot, uuid: &Uuid<Self>) -> Result<Option<Self>, Error> {
        let id = Record::storgit_id(uuid);
        let entry = tokio::task::block_in_place(|| lot.store().get(&id)).map_err(Error::Storgit)?;
        let Some(entry) = entry else {
            return Ok(None);
        };

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
            &Record::data_aad(uuid, lot.uuid()),
        )?;

        Ok(Some(Record {
            uuid: uuid.clone(),
            lot_uuid: lot.uuid().clone(),
            label,
            data,
        }))
    }

    /// Walk every historical revision of the record identified by `uuid`,
    /// newest commit first. Each live commit is decrypted into a
    /// [`Revision`]; tombstone commits (written by [`Record::delete`]) are
    /// skipped. Returns `None` if no such record is present in the lot.
    #[cfg(feature = "db")]
    pub async fn history(lot: &Lot, uuid: &Uuid<Self>) -> Result<Option<Vec<Revision>>, Error> {
        let id = Record::storgit_id(uuid);
        let entries =
            tokio::task::block_in_place(|| lot.store().history(&id)).map_err(Error::Storgit)?;
        if entries.is_empty() {
            return Ok(None);
        }

        let data_aad = Record::data_aad(uuid, lot.uuid());
        let mut revisions = Vec::with_capacity(entries.len());
        for entry in entries {
            let (Some(label_bytes), Some(data_bytes)) = (entry.label, entry.data) else {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lot::Lot;
    #[cfg(feature = "db")]
    use crate::{db::Database, user::User};

    #[cfg(feature = "db")]
    async fn fixture() -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("valet.sqlite");
        let url = format!("sqlite://{}?mode=rwc", db_path.display());
        let db = Database::new(&url).await.unwrap();
        (dir, db)
    }

    #[test]
    fn new() {
        #[cfg(feature = "db")]
        let tmp = tempfile::tempdir().unwrap();
        #[cfg(feature = "db")]
        let lot = Lot::new("test", tmp.path()).unwrap();
        #[cfg(not(feature = "db"))]
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
    #[tokio::test(flavor = "multi_thread")]
    async fn show_roundtrip() {
        let (dir, db) = fixture().await;
        let user = User::new("nixpulvis", "password".try_into().unwrap())
            .expect("failed to make user")
            .register(&db)
            .await
            .expect("failed to register user");
        let mut lot = Lot::new("lot a", dir.path()).unwrap();
        lot.save(&db, &user).await.expect("failed to save lot");
        let record = Record::new(
            &lot,
            "foo".parse::<Label>().unwrap(),
            Data::new("bar".try_into().unwrap()),
        );
        let uuid = record
            .save(&mut lot)
            .await
            .expect("failed to save record");
        let loaded = Record::show(&lot, &uuid)
            .await
            .expect("failed to show record")
            .expect("record missing");
        assert_eq!(loaded, record);
    }

    #[cfg(feature = "db")]
    #[tokio::test(flavor = "multi_thread")]
    async fn show_wrong_lot_returns_none() {
        let (dir, db) = fixture().await;
        let user = User::new("nixpulvis", "password".try_into().unwrap())
            .expect("failed to make user")
            .register(&db)
            .await
            .expect("failed to register user");
        let mut lot_a = Lot::new("lot a", dir.path()).unwrap();
        lot_a.save(&db, &user).await.expect("failed to save lot");
        let mut lot_b = Lot::new("lot b", dir.path()).unwrap();
        lot_b.save(&db, &user).await.expect("failed to save lot");
        let uuid = Record::new(
            &lot_a,
            "foo".parse::<Label>().unwrap(),
            Data::new("bar".try_into().unwrap()),
        )
        .save(&mut lot_a)
        .await
        .expect("failed to save record");
        // The record only lives in lot_a's store. lot_b never saw it.
        assert!(
            Record::show(&lot_b, &uuid)
                .await
                .expect("failed to show")
                .is_none()
        );
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
        let record = Record::new(
            &lot,
            "foo".parse::<Label>().unwrap(),
            Data::new("bar".try_into().unwrap()),
        );
        let uuid = record
            .save(&mut lot)
            .await
            .expect("failed to save record");
        record
            .delete(&mut lot)
            .await
            .expect("failed to delete record");
        assert!(
            Record::show(&lot, &uuid)
                .await
                .expect("failed to show record")
                .is_none()
        );
    }

    #[cfg(feature = "db")]
    #[tokio::test(flavor = "multi_thread")]
    async fn save_many_roundtrip() {
        let (dir, db) = fixture().await;
        let user = User::new("nixpulvis", "password".try_into().unwrap())
            .expect("failed to make user")
            .register(&db)
            .await
            .expect("failed to register user");
        let mut lot = Lot::new("lot a", dir.path()).unwrap();
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
        let uuids = Record::save_many(&mut lot, &records, |ev| {
            events.push(match ev {
                SaveProgress::OpenedStore => "opened",
                SaveProgress::PutRecord(_) => "put",
                SaveProgress::ParentFlushed => "flushed",
                SaveProgress::SaveRecord => "save_r",
                SaveProgress::SaveLot => "save_l",
            });
        })
        .await
        .expect("failed to save_many");
        assert_eq!(uuids.len(), records.len());
        assert_eq!(
            events,
            vec!["opened", "put", "put", "put", "flushed", "save_r", "save_l"]
        );

        for (record, uuid) in records.iter().zip(uuids.iter()) {
            assert_eq!(uuid, record.uuid());
            let loaded = Record::show(&lot, uuid)
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
        Record::save_many(&mut lot, &updated, |_| {})
            .await
            .expect("failed to re-save");
        let loaded = Record::show(&lot, records[0].uuid())
            .await
            .expect("failed to show record")
            .expect("record missing");
        assert_eq!(loaded.password().to_string(), "p1-new");
        let history = Record::history(&lot, records[0].uuid())
            .await
            .expect("failed to read history")
            .expect("history missing");
        assert_eq!(history.len(), 2);
    }

    #[cfg(feature = "db")]
    #[tokio::test(flavor = "multi_thread")]
    async fn save_many_empty_is_noop() {
        let (dir, db) = fixture().await;
        let user = User::new("nixpulvis", "password".try_into().unwrap())
            .expect("failed to make user")
            .register(&db)
            .await
            .expect("failed to register user");
        let mut lot = Lot::new("lot a", dir.path()).unwrap();
        lot.save(&db, &user).await.expect("failed to save lot");
        let uuids = Record::save_many(&mut lot, &[], |_| {})
            .await
            .expect("failed to save_many");
        assert!(uuids.is_empty());
    }

    #[cfg(feature = "db")]
    #[tokio::test(flavor = "multi_thread")]
    async fn save_many_rejects_foreign_lot() {
        let (dir, db) = fixture().await;
        let user = User::new("nixpulvis", "password".try_into().unwrap())
            .expect("failed to make user")
            .register(&db)
            .await
            .expect("failed to register user");
        let mut lot_a = Lot::new("lot a", dir.path()).unwrap();
        lot_a.save(&db, &user).await.expect("failed to save lot");
        let mut lot_b = Lot::new("lot b", dir.path()).unwrap();
        lot_b.save(&db, &user).await.expect("failed to save lot");
        let foreign = Record::new(
            &lot_b,
            "foo".parse::<Label>().unwrap(),
            Data::new("p".try_into().unwrap()),
        );
        let err = Record::save_many(&mut lot_a, &[foreign], |_| {})
            .await
            .expect_err("expected LotMismatch");
        assert!(matches!(err, Error::LotMismatch { .. }));
    }

    #[cfg(feature = "db")]
    #[tokio::test(flavor = "multi_thread")]
    async fn save_rejects_name_collision_with_different_uuid() {
        let (dir, db) = fixture().await;
        let user = User::new("nixpulvis", "password".try_into().unwrap())
            .expect("failed to make user")
            .register(&db)
            .await
            .expect("failed to register user");
        let mut lot = Lot::new("lot a", dir.path()).unwrap();
        lot.save(&db, &user).await.expect("failed to save lot");
        let first = Record::new(
            &lot,
            "acct".parse::<Label>().unwrap(),
            Data::new("p1".try_into().unwrap()),
        );
        let first_uuid = first.save(&mut lot).await.unwrap();

        let collider = Record::new(
            &lot,
            "acct".parse::<Label>().unwrap(),
            Data::new("p2".try_into().unwrap()),
        );
        assert_ne!(&first_uuid, collider.uuid());
        let err = collider
            .save(&mut lot)
            .await
            .expect_err("expected LabelCollision");
        assert!(matches!(
            err,
            Error::LabelCollision { ref existing, .. } if existing == &first_uuid
        ));

        Record::with_uuid(
            first_uuid.clone(),
            &lot,
            "acct".parse::<Label>().unwrap(),
            Data::new("p3".try_into().unwrap()),
        )
        .save(&mut lot)
        .await
        .expect("reuse should succeed");
    }

    #[cfg(feature = "db")]
    #[tokio::test(flavor = "multi_thread")]
    async fn save_many_rejects_name_collision() {
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
            "acct".parse::<Label>().unwrap(),
            Data::new("p1".try_into().unwrap()),
        )
        .save(&mut lot)
        .await
        .unwrap();

        let collider = Record::new(
            &lot,
            "acct".parse::<Label>().unwrap(),
            Data::new("p2".try_into().unwrap()),
        );
        let err = Record::save_many(&mut lot, &[collider], |_| {})
            .await
            .expect_err("expected LabelCollision");
        assert!(matches!(err, Error::LabelCollision { .. }));
    }

    #[cfg(feature = "db")]
    #[tokio::test(flavor = "multi_thread")]
    async fn save_many_rejects_intra_batch_collision() {
        let (dir, db) = fixture().await;
        let user = User::new("nixpulvis", "password".try_into().unwrap())
            .expect("failed to make user")
            .register(&db)
            .await
            .expect("failed to register user");
        let mut lot = Lot::new("lot a", dir.path()).unwrap();
        lot.save(&db, &user).await.expect("failed to save lot");
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
        let err = Record::save_many(&mut lot, &[a, b], |_| {})
            .await
            .expect_err("expected LabelCollision");
        assert!(matches!(err, Error::LabelCollision { .. }));
    }
}
