use crate::{
    db::Database,
    encrypt::{Stash, Encrypted},
    lot::Lot,
    record::{Error, Label, Record},
    uuid::Uuid,
};
use sea_orm::{DerivePartialModel, entity::prelude::*};
use std::collections::BTreeMap;

#[derive(DerivePartialModel)]
#[sea_orm(entity = "super::orm::Entity")]
struct LabelRow {
    uuid: String,
    label: Vec<u8>,
    label_nonce: Vec<u8>,
}

/// An in-memory map from `Label` to `Uuid<Record>` for a single lot.
///
/// Built by decrypting only the `label` column of each row in the lot, which
/// avoids materializing any passwords. Use [`RecordIndex::find`] to resolve a
/// label to a UUID, then call [`Record::show`](crate::record::Record::show) to
/// pull the password for that one record.
///
/// The index is not persisted; it's rebuilt from the encrypted label column
/// on every [`RecordIndex::load`]. This keeps the DB as the single source of
/// truth and removes any sync burden on `upsert`/`delete`.
pub struct RecordIndex {
    entries: BTreeMap<Label, Uuid<Record>>,
}

impl RecordIndex {
    /// Build an index for `lot` by fetching and decrypting every record's
    /// label column.
    ///
    /// The password-bearing `data` column isn't loaded.
    pub async fn load(db: &Database, lot: &Lot) -> Result<Self, Error> {
        let rows = super::orm::Entity::find()
            .filter(super::orm::Column::LotUuid.eq(lot.uuid().to_string()))
            .into_partial_model::<LabelRow>()
            .all(db.connection())
            .await?;

        let mut entries = BTreeMap::new();
        for row in rows {
            let uuid = Uuid::<Record>::parse(&row.uuid)?;
            let aad = Record::label_aad(&uuid, lot.uuid());
            let encrypted = Encrypted {
                data: row.label,
                nonce: row.label_nonce,
            };
            let label = Label::decrypt_with_aad(&encrypted, lot.key(), &aad)?;
            entries.insert(label, uuid);
        }
        Ok(RecordIndex { entries })
    }

    /// Look up the UUID of the record with the given label, if one exists.
    pub fn find(&self, label: &Label) -> Option<&Uuid<Record>> {
        self.entries.get(label)
    }

    /// Return an iterator over every label in the index.
    ///
    /// Iteration follows `Label: Ord`.
    pub fn labels(&self) -> impl Iterator<Item = &Label> {
        self.entries.keys()
    }

    /// Return an iterator over every `(label, uuid)` pair in the index.
    ///
    /// Iteration follows `Label: Ord`.
    ///
    /// Also available via `IntoIterator` for `&RecordIndex`, so `for entry in
    /// &index` works directly.
    pub fn iter(&self) -> impl Iterator<Item = (&Label, &Uuid<Record>)> {
        self.into_iter()
    }

    /// True if the lot has no records.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Number of records in the lot.
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

impl<'a> IntoIterator for &'a RecordIndex {
    type Item = (&'a Label, &'a Uuid<Record>);
    type IntoIter = std::collections::btree_map::Iter<'a, Label, Uuid<Record>>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter()
    }
}

impl IntoIterator for RecordIndex {
    type Item = (Label, Uuid<Record>);
    type IntoIter = std::collections::btree_map::IntoIter<Label, Uuid<Record>>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.into_iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        db::Database,
        record::{Data, Label},
        user::User,
    };

    async fn setup() -> (Database, User, Lot) {
        let db = Database::new("sqlite://:memory:")
            .await
            .expect("failed to create database");
        let user = User::new("nixpulvis", "password".try_into().unwrap())
            .expect("failed to make user")
            .register(&db)
            .await
            .expect("failed to register user");
        let lot = Lot::new("lot a");
        lot.save(&db, &user).await.expect("failed to save lot");
        (db, user, lot)
    }

    #[tokio::test]
    async fn empty_index() {
        let (db, _user, lot) = setup().await;
        let index = RecordIndex::load(&db, &lot)
            .await
            .expect("load empty index");
        assert!(index.is_empty());
        assert_eq!(0, index.len());
    }

    #[tokio::test]
    async fn index_contains_inserted_labels() {
        let (db, _user, lot) = setup().await;
        let uuid_a = Record::new(
            &lot,
            Label::Simple("a".into()),
            Data::new("1".try_into().unwrap()),
        )
        .upsert(&db, &lot)
        .await
        .unwrap();
        let uuid_b = Record::new(
            &lot,
            Label::Simple("b".into()),
            Data::new("2".try_into().unwrap()),
        )
        .upsert(&db, &lot)
        .await
        .unwrap();

        let index = RecordIndex::load(&db, &lot).await.expect("load index");
        assert_eq!(2, index.len());
        assert_eq!(Some(&uuid_a), index.find(&Label::Simple("a".into())));
        assert_eq!(Some(&uuid_b), index.find(&Label::Simple("b".into())));
        assert_eq!(None, index.find(&Label::Simple("missing".into())));
    }

    #[tokio::test]
    async fn index_then_show_resolves_password() {
        let (db, _user, lot) = setup().await;
        Record::new(
            &lot,
            Label::Simple("target".into()),
            Data::new("s3cret".try_into().unwrap()),
        )
        .upsert(&db, &lot)
        .await
        .unwrap();

        let index = RecordIndex::load(&db, &lot).await.unwrap();
        let uuid = index
            .find(&Label::Simple("target".into()))
            .expect("label in index");
        let record = Record::show(&db, &lot, uuid)
            .await
            .unwrap()
            .expect("record exists");
        assert_eq!("s3cret", record.password().to_string());
    }

    #[tokio::test]
    async fn for_loop_iteration() {
        let (db, _user, lot) = setup().await;
        Record::new(
            &lot,
            Label::Simple("a".into()),
            Data::new("1".try_into().unwrap()),
        )
        .upsert(&db, &lot)
        .await
        .unwrap();
        Record::new(
            &lot,
            Label::Simple("b".into()),
            Data::new("2".try_into().unwrap()),
        )
        .upsert(&db, &lot)
        .await
        .unwrap();

        let index = RecordIndex::load(&db, &lot).await.unwrap();
        let mut seen = Vec::new();
        for (label, _uuid) in &index {
            seen.push(label.to_string());
        }
        seen.sort();
        assert_eq!(vec!["a".to_string(), "b".to_string()], seen);

        let mut owned: Vec<_> = index.into_iter().map(|(l, _)| l.to_string()).collect();
        owned.sort();
        assert_eq!(vec!["a".to_string(), "b".to_string()], owned);
    }

    #[tokio::test]
    async fn deleted_record_absent_from_fresh_index() {
        let (db, _user, lot) = setup().await;
        let record = Record::new(
            &lot,
            Label::Simple("ephemeral".into()),
            Data::new("x".try_into().unwrap()),
        );
        record.upsert(&db, &lot).await.unwrap();
        record.delete(&db).await.unwrap();
        let index = RecordIndex::load(&db, &lot).await.unwrap();
        assert!(index.is_empty());
    }
}
