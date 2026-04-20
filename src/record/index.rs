use crate::{
    db::Database,
    encrypt::Stash,
    lot::Lot,
    record::{Error, Label, Query, Record},
    uuid::Uuid,
};
use sea_orm::entity::prelude::*;
use std::collections::BTreeMap;

/// An in-memory map from `Label` to `Uuid<Record>` for a single lot.
///
/// Built from the label cache carried in the lot's storgit parent tarball.
/// Labels are stored plaintext inside storgit (the parent itself is encrypted
/// at the DB boundary under the lot key), so building the index neither opens
/// any submodules nor decrypts any record-level ciphertext - a password is
/// only materialized by [`Record::show`](crate::record::Record::show).
///
/// The index is not persisted; it's rebuilt from the store on every
/// [`RecordIndex::load`]. This keeps storage as the single source of truth
/// and removes any sync burden on `upsert`/`delete`.
pub struct RecordIndex {
    entries: BTreeMap<Label, Uuid<Record>>,
}

impl RecordIndex {
    /// Build an index for `lot` by reading its storgit parent label cache.
    ///
    /// Re-reads `lots.store` from the database so the index reflects the
    /// current persisted state. Returns [`Error::MissingLot`] if the lot
    /// row is gone (stale `&Lot` after [`Lot::delete`](crate::lot::Lot::delete),
    /// or a lot that was never saved) so callers can distinguish "empty
    /// lot" from "lot does not exist".
    pub async fn load(db: &Database, lot: &Lot) -> Result<Self, Error> {
        let model = crate::lot::orm::Entity::find_by_id(lot.uuid().to_string())
            .one(db.connection())
            .await?
            .ok_or(Error::MissingLot)?;
        let parent_bytes = lot.decrypt_store_bytes(&model.store)?;

        let store = storgit::Store::open(storgit::Parts {
            parent: parent_bytes,
            modules: std::collections::HashMap::new(),
        })
        .map_err(Error::Storgit)?;

        let mut entries = BTreeMap::new();
        for (id, label_bytes) in store.list_labels() {
            let uuid = Uuid::<Record>::parse(id.as_str())?;
            let label = Label::decode(&label_bytes)?;
            entries.insert(label, uuid);
        }
        Ok(RecordIndex { entries })
    }

    /// Look up the UUID of the record with the given label, if one exists.
    pub fn find(&self, label: &Label) -> Option<&Uuid<Record>> {
        self.entries.get(label)
    }

    /// Look up the UUID of the record with the given primary name, ignoring
    /// [`Label::extra`]. Record identity within a lot is the [`LabelName`]
    /// alone; extras are searchable metadata that may change across
    /// revisions of the same record. Returns the first match in
    /// [`Label: Ord`] order if the caller has historically stored multiple
    /// rows with the same name.
    pub fn find_by_name(&self, name: &super::LabelName) -> Option<&Uuid<Record>> {
        self.entries
            .iter()
            .find(|(label, _)| label.name() == name)
            .map(|(_, uuid)| uuid)
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

    /// Return every `(label, uuid)` pair whose label satisfies `query`.
    ///
    /// O(n) scan over the in-memory index. Entries yield in `Label: Ord`
    /// order, which ties-breaks on `extra` — so labels that differ only in
    /// their `extra` contents have an unspecified relative order. Don't
    /// depend on it.
    pub fn search<'a>(
        &'a self,
        query: &'a Query,
    ) -> impl Iterator<Item = (&'a Label, &'a Uuid<Record>)> {
        self.entries
            .iter()
            .filter(move |(label, _)| query.matches_label(label))
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
    use std::str::FromStr;

    async fn setup() -> (Database, User, Lot) {
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
        let (db, _user, mut lot) = setup().await;
        let uuid_a = Record::new(
            &lot,
            "a".parse::<Label>().unwrap(),
            Data::new("1".try_into().unwrap()),
        )
        .upsert(&db, &mut lot)
        .await
        .unwrap();
        let uuid_b = Record::new(
            &lot,
            "b".parse::<Label>().unwrap(),
            Data::new("2".try_into().unwrap()),
        )
        .upsert(&db, &mut lot)
        .await
        .unwrap();

        let index = RecordIndex::load(&db, &lot).await.expect("load index");
        assert_eq!(2, index.len());
        assert_eq!(Some(&uuid_a), index.find(&"a".parse::<Label>().unwrap()));
        assert_eq!(Some(&uuid_b), index.find(&"b".parse::<Label>().unwrap()));
        assert_eq!(None, index.find(&"missing".parse::<Label>().unwrap()));
    }

    #[tokio::test]
    async fn find_by_name_ignores_extras() {
        let (db, _user, mut lot) = setup().await;
        // Two records stored under the same primary name with different
        // extras. Because record identity is the name alone, the second
        // upsert reuses the first uuid — demonstrated here by constructing
        // the second record with `Record::with_uuid`.
        let uuid = Record::new(
            &lot,
            "acct"
                .parse::<Label>()
                .unwrap()
                .add_extra("tag", "foo")
                .unwrap(),
            Data::new("pw1".try_into().unwrap()),
        )
        .upsert(&db, &mut lot)
        .await
        .unwrap();

        let index = RecordIndex::load(&db, &lot).await.unwrap();
        // `find_by_name` locates the record even though the caller uses a
        // label that doesn't carry the same extras.
        let name = "acct".parse::<Label>().unwrap();
        assert_eq!(Some(&uuid), index.find_by_name(name.name()));
        // Overwrite with the bare label (no extras), reusing the uuid.
        Record::with_uuid(
            uuid.clone(),
            &lot,
            name.clone(),
            Data::new("pw2".try_into().unwrap()),
        )
        .upsert(&db, &mut lot)
        .await
        .unwrap();

        let index = RecordIndex::load(&db, &lot).await.unwrap();
        assert_eq!(1, index.len(), "name-identity keeps a single record");
        assert_eq!(Some(&uuid), index.find_by_name(name.name()));

        // The history walks both revisions under the shared uuid.
        let revisions = Record::history(&db, &lot, &uuid)
            .await
            .unwrap()
            .expect("history exists");
        assert_eq!(2, revisions.len());
        let passwords: Vec<String> = revisions
            .iter()
            .map(|r| r.data.password().to_string())
            .collect();
        assert!(passwords.contains(&"pw1".to_string()));
        assert!(passwords.contains(&"pw2".to_string()));
    }

    #[tokio::test]
    async fn index_then_show_resolves_password() {
        let (db, _user, mut lot) = setup().await;
        Record::new(
            &lot,
            "target".parse::<Label>().unwrap(),
            Data::new("s3cret".try_into().unwrap()),
        )
        .upsert(&db, &mut lot)
        .await
        .unwrap();

        let index = RecordIndex::load(&db, &lot).await.unwrap();
        let uuid = index
            .find(&"target".parse::<Label>().unwrap())
            .expect("label in index");
        let record = Record::show(&db, &lot, uuid)
            .await
            .unwrap()
            .expect("record exists");
        assert_eq!("s3cret", record.password().to_string());
    }

    #[tokio::test]
    async fn for_loop_iteration() {
        let (db, _user, mut lot) = setup().await;
        Record::new(
            &lot,
            "a".parse::<Label>().unwrap(),
            Data::new("1".try_into().unwrap()),
        )
        .upsert(&db, &mut lot)
        .await
        .unwrap();
        Record::new(
            &lot,
            "b".parse::<Label>().unwrap(),
            Data::new("2".try_into().unwrap()),
        )
        .upsert(&db, &mut lot)
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

    async fn seed_search_lot() -> (Database, Lot) {
        let (db, _user, mut lot) = setup().await;
        // Mix of domains (so the `.com` regex has something to exclude), a
        // simple label, and realistic extras. Two records share a url so
        // extras filtering has ambiguity to resolve.
        Record::new(
            &lot,
            "nix@example.com"
                .parse::<Label>()
                .unwrap()
                .add_extra("url", "https://example.com")
                .unwrap(),
            Data::new("pw1".try_into().unwrap()),
        )
        .upsert(&db, &mut lot)
        .await
        .unwrap();
        Record::new(
            &lot,
            "alt@example.com"
                .parse::<Label>()
                .unwrap()
                .add_extra("url", "https://example.com")
                .unwrap(),
            Data::new("pw2".try_into().unwrap()),
        )
        .upsert(&db, &mut lot)
        .await
        .unwrap();
        Record::new(
            &lot,
            "nix@other.com"
                .parse::<Label>()
                .unwrap()
                .add_extra("url", "https://other.com")
                .unwrap(),
            Data::new("pw3".try_into().unwrap()),
        )
        .upsert(&db, &mut lot)
        .await
        .unwrap();
        Record::new(
            &lot,
            "bob@company.org"
                .parse::<Label>()
                .unwrap()
                .add_extra("tag", "work")
                .unwrap(),
            Data::new("pw4".try_into().unwrap()),
        )
        .upsert(&db, &mut lot)
        .await
        .unwrap();
        Record::new(
            &lot,
            "github"
                .parse::<Label>()
                .unwrap()
                .add_extra("url", "https://github.com")
                .unwrap()
                .add_extra("note", "devtools")
                .unwrap(),
            Data::new("pw5".try_into().unwrap()),
        )
        .upsert(&db, &mut lot)
        .await
        .unwrap();
        (db, lot)
    }

    fn search_labels(index: &RecordIndex, q: &Query) -> Vec<String> {
        let mut out: Vec<String> = index.search(q).map(|(l, _)| l.name().to_string()).collect();
        out.sort();
        out
    }

    #[tokio::test]
    async fn search_literal_name() {
        let (db, lot) = seed_search_lot().await;
        let index = RecordIndex::load(&db, &lot).await.unwrap();
        let q = Query::from_str("nix@example.com").unwrap();
        assert_eq!(search_labels(&index, &q), vec!["nix@example.com"]);
    }

    #[tokio::test]
    async fn search_regex_name() {
        // Every `*.com` domain record but not `.org` or the simple label.
        let (db, lot) = seed_search_lot().await;
        let index = RecordIndex::load(&db, &lot).await.unwrap();
        let q = Query::from_str(r"~.*@.*\.com").unwrap();
        assert_eq!(
            search_labels(&index, &q),
            vec!["alt@example.com", "nix@example.com", "nix@other.com"]
        );
    }

    #[tokio::test]
    async fn search_extras_only() {
        // Both example.com accounts share a url.
        let (db, lot) = seed_search_lot().await;
        let index = RecordIndex::load(&db, &lot).await.unwrap();
        let q = Query::from_str("~.*<url=https://example.com>").unwrap();
        assert_eq!(
            search_labels(&index, &q),
            vec!["alt@example.com", "nix@example.com"]
        );
    }

    #[tokio::test]
    async fn search_regex_name_and_extras_and_semantics() {
        // Regex covers three records; the extras filter narrows to the one
        // at other.com.
        let (db, lot) = seed_search_lot().await;
        let index = RecordIndex::load(&db, &lot).await.unwrap();
        let q = Query::from_str(r"~.*@.*\.com<url=https://other.com>").unwrap();
        assert_eq!(search_labels(&index, &q), vec!["nix@other.com"]);
    }

    #[tokio::test]
    async fn search_regex_key_presence() {
        // Only the simple label carries a `note` key.
        let (db, lot) = seed_search_lot().await;
        let index = RecordIndex::load(&db, &lot).await.unwrap();
        let q = Query::from_str("~.*<~^note$>").unwrap();
        assert_eq!(search_labels(&index, &q), vec!["github"]);
    }

    #[tokio::test]
    async fn search_regex_key_eq_value() {
        // "Some key matches ^u with value https://example.com" catches the
        // two example.com accounts (their `url` extra) but nothing else.
        let (db, lot) = seed_search_lot().await;
        let index = RecordIndex::load(&db, &lot).await.unwrap();
        let q = Query::from_str("~.*<~^u=https://example.com>").unwrap();
        assert_eq!(
            search_labels(&index, &q),
            vec!["alt@example.com", "nix@example.com"]
        );
    }

    #[tokio::test]
    async fn search_regex_key_regex_value() {
        // Any url-ish key with a value that's an https .com URL. Excludes
        // bob@company.org (no url) and the `tag=work` case, but hits both
        // example.com accounts, nix@other.com, and the github simple label.
        let (db, lot) = seed_search_lot().await;
        let index = RecordIndex::load(&db, &lot).await.unwrap();
        let q = Query::from_str(r"~.*<~^url$~^https://.*\.com$>").unwrap();
        assert_eq!(
            search_labels(&index, &q),
            vec![
                "alt@example.com",
                "github",
                "nix@example.com",
                "nix@other.com"
            ]
        );
    }

    #[tokio::test]
    async fn search_match_all() {
        let (db, lot) = seed_search_lot().await;
        let index = RecordIndex::load(&db, &lot).await.unwrap();
        let q = Query::from_str("~.*").unwrap();
        assert_eq!(index.search(&q).count(), 5);
    }

    #[tokio::test]
    async fn search_no_matches() {
        let (db, lot) = seed_search_lot().await;
        let index = RecordIndex::load(&db, &lot).await.unwrap();
        let q = Query::from_str("nonesuch").unwrap();
        assert!(index.search(&q).next().is_none());
    }

    #[tokio::test]
    async fn deleted_record_absent_from_fresh_index() {
        let (db, _user, mut lot) = setup().await;
        let record = Record::new(
            &lot,
            "ephemeral".parse::<Label>().unwrap(),
            Data::new("x".try_into().unwrap()),
        );
        record.upsert(&db, &mut lot).await.unwrap();
        record.delete(&db, &mut lot).await.unwrap();
        let index = RecordIndex::load(&db, &lot).await.unwrap();
        assert!(index.is_empty());
    }
}
