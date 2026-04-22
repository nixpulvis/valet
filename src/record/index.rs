use crate::{
    encrypt::Stash,
    record::{Error, Label, Query, Record},
    uuid::Uuid,
};
use std::collections::BTreeMap;

/// An in-memory map from `Label` to `Uuid<Record>` for a single lot.
///
/// Built from the label cache carried in the lot's live storgit store.
/// Labels live plaintext inside the store's parent tree (the parent
/// itself is encrypted at the DB boundary under the lot key), so
/// building the index neither opens any submodules nor decrypts any
/// record-level ciphertext - a password is only materialized by
/// [`Record::show`](crate::record::Record::show).
///
/// The owning [`Lot`](crate::lot::Lot) keeps the index in sync with
/// the live store across [`Record::save`](crate::record::Record::save)
/// and [`Record::delete`](crate::record::Record::delete), so callers
/// can treat `lot.index()` as an authoritative listing of the lot's
/// current labels.
#[derive(Default)]
pub struct RecordIndex {
    entries: BTreeMap<Label, Uuid<Record>>,
}

impl RecordIndex {
    /// Build an index from a live storgit store's label cache.
    #[cfg(feature = "db")]
    pub(crate) fn from_store(store: &storgit::Store) -> Result<Self, Error> {
        let mut entries = BTreeMap::new();
        for (id, label_bytes) in store.list_labels() {
            let uuid = Uuid::<Record>::parse(id.as_str())?;
            let label = Label::decode(&label_bytes)?;
            entries.insert(label, uuid);
        }
        Ok(RecordIndex { entries })
    }

    /// Insert or replace an entry for `uuid`. Called by
    /// [`Record::save`](crate::record::Record::save) right after the
    /// storgit put so the index mirrors the store.
    pub(crate) fn insert(&mut self, label: Label, uuid: Uuid<Record>) {
        // Record identity is the label name; clear any prior entry
        // under this uuid so a subsequent `find_by_name` returns only
        // the current revision even when extras changed.
        self.entries.retain(|_, v| v != &uuid);
        self.entries.insert(label, uuid);
    }

    /// Remove any entry mapped to `uuid`. Called by
    /// [`Record::delete`](crate::record::Record::delete).
    pub(crate) fn remove(&mut self, uuid: &Uuid<Record>) {
        self.entries.retain(|_, v| v != uuid);
    }

    /// Look up the UUID of the record with the given label, if one exists.
    pub fn find(&self, label: &Label) -> Option<&Uuid<Record>> {
        self.entries.get(label)
    }

    /// Reject if `name` is already owned by a record with a different
    /// uuid. Record identity within a lot is the [`LabelName`] alone,
    /// so two records sharing a name are unrepresentable in the
    /// index. To update the existing record, resolve its uuid via
    /// `lot.index().find_by_name(name)` and construct the new
    /// revision with
    /// [`Record::with_uuid`](crate::record::Record::with_uuid).
    #[cfg(feature = "db")]
    pub(crate) fn check_name_owner(
        &self,
        name: &super::LabelName,
        uuid: &Uuid<Record>,
    ) -> Result<(), Error> {
        if let Some(existing) = self.find_by_name(name)
            && existing != uuid
        {
            return Err(Error::LabelCollision {
                name: name.clone(),
                existing: existing.clone(),
                attempted: uuid.clone(),
            });
        }
        Ok(())
    }

    /// Look up the UUID of the record with the given primary name, ignoring
    /// [`Label::extra`]. Record identity within a lot is the [`LabelName`](super::LabelName)
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
    /// order, which ties-breaks on `extra` - so labels that differ only in
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
    use crate::{
        db::Database,
        lot::Lot,
        record::{Data, Label, Query, Record},
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

    #[tokio::test(flavor = "multi_thread")]
    async fn empty_index() {
        let (_db, _user, lot) = setup().await;
        assert!(lot.index().is_empty());
        assert_eq!(0, lot.index().len());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn index_contains_inserted_labels() {
        let (db, _user, mut lot) = setup().await;
        let uuid_a = Record::new(
            &lot,
            "a".parse::<Label>().unwrap(),
            Data::new("1".try_into().unwrap()),
        )
        .save(&db, &mut lot)
        .await
        .unwrap();
        let uuid_b = Record::new(
            &lot,
            "b".parse::<Label>().unwrap(),
            Data::new("2".try_into().unwrap()),
        )
        .save(&db, &mut lot)
        .await
        .unwrap();

        assert_eq!(2, lot.index().len());
        assert_eq!(
            Some(&uuid_a),
            lot.index().find(&"a".parse::<Label>().unwrap())
        );
        assert_eq!(
            Some(&uuid_b),
            lot.index().find(&"b".parse::<Label>().unwrap())
        );
        assert_eq!(None, lot.index().find(&"missing".parse::<Label>().unwrap()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn find_by_name_ignores_extras() {
        let (db, _user, mut lot) = setup().await;
        let uuid = Record::new(
            &lot,
            "acct"
                .parse::<Label>()
                .unwrap()
                .add_extra("tag", "foo")
                .unwrap(),
            Data::new("pw1".try_into().unwrap()),
        )
        .save(&db, &mut lot)
        .await
        .unwrap();

        let name = "acct".parse::<Label>().unwrap();
        assert_eq!(Some(&uuid), lot.index().find_by_name(name.name()));

        Record::with_uuid(
            uuid.clone(),
            &lot,
            name.clone(),
            Data::new("pw2".try_into().unwrap()),
        )
        .save(&db, &mut lot)
        .await
        .unwrap();

        assert_eq!(1, lot.index().len(), "name-identity keeps a single record");
        assert_eq!(Some(&uuid), lot.index().find_by_name(name.name()));

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

    #[tokio::test(flavor = "multi_thread")]
    async fn index_then_show_resolves_password() {
        let (db, _user, mut lot) = setup().await;
        Record::new(
            &lot,
            "target".parse::<Label>().unwrap(),
            Data::new("s3cret".try_into().unwrap()),
        )
        .save(&db, &mut lot)
        .await
        .unwrap();

        let uuid = lot
            .index()
            .find(&"target".parse::<Label>().unwrap())
            .expect("label in index")
            .clone();
        let record = Record::show(&db, &lot, &uuid)
            .await
            .unwrap()
            .expect("record exists");
        assert_eq!("s3cret", record.password().to_string());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn for_loop_iteration() {
        let (db, _user, mut lot) = setup().await;
        Record::new(
            &lot,
            "a".parse::<Label>().unwrap(),
            Data::new("1".try_into().unwrap()),
        )
        .save(&db, &mut lot)
        .await
        .unwrap();
        Record::new(
            &lot,
            "b".parse::<Label>().unwrap(),
            Data::new("2".try_into().unwrap()),
        )
        .save(&db, &mut lot)
        .await
        .unwrap();

        let mut seen = Vec::new();
        for (label, _uuid) in lot.index() {
            seen.push(label.to_string());
        }
        seen.sort();
        assert_eq!(vec!["a".to_string(), "b".to_string()], seen);
    }

    async fn seed_search_lot() -> Lot {
        let (db, _user, mut lot) = setup().await;
        Record::new(
            &lot,
            "nix@example.com"
                .parse::<Label>()
                .unwrap()
                .add_extra("url", "https://example.com")
                .unwrap(),
            Data::new("pw1".try_into().unwrap()),
        )
        .save(&db, &mut lot)
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
        .save(&db, &mut lot)
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
        .save(&db, &mut lot)
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
        .save(&db, &mut lot)
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
        .save(&db, &mut lot)
        .await
        .unwrap();
        lot
    }

    fn search_labels(lot: &Lot, q: &Query) -> Vec<String> {
        let mut out: Vec<String> = lot
            .index()
            .search(q)
            .map(|(l, _)| l.name().to_string())
            .collect();
        out.sort();
        out
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn search_literal_name() {
        let lot = seed_search_lot().await;
        let q = Query::from_str("nix@example.com").unwrap();
        assert_eq!(search_labels(&lot, &q), vec!["nix@example.com"]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn search_regex_name() {
        let lot = seed_search_lot().await;
        let q = Query::from_str(r"~.*@.*\.com").unwrap();
        assert_eq!(
            search_labels(&lot, &q),
            vec!["alt@example.com", "nix@example.com", "nix@other.com"]
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn search_extras_only() {
        let lot = seed_search_lot().await;
        let q = Query::from_str("~.*<url=https://example.com>").unwrap();
        assert_eq!(
            search_labels(&lot, &q),
            vec!["alt@example.com", "nix@example.com"]
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn search_regex_name_and_extras_and_semantics() {
        let lot = seed_search_lot().await;
        let q = Query::from_str(r"~.*@.*\.com<url=https://other.com>").unwrap();
        assert_eq!(search_labels(&lot, &q), vec!["nix@other.com"]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn search_regex_key_presence() {
        let lot = seed_search_lot().await;
        let q = Query::from_str("~.*<~^note$>").unwrap();
        assert_eq!(search_labels(&lot, &q), vec!["github"]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn search_regex_key_eq_value() {
        let lot = seed_search_lot().await;
        let q = Query::from_str("~.*<~^u=https://example.com>").unwrap();
        assert_eq!(
            search_labels(&lot, &q),
            vec!["alt@example.com", "nix@example.com"]
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn search_regex_key_regex_value() {
        let lot = seed_search_lot().await;
        let q = Query::from_str(r"~.*<~^url$~^https://.*\.com$>").unwrap();
        assert_eq!(
            search_labels(&lot, &q),
            vec![
                "alt@example.com",
                "github",
                "nix@example.com",
                "nix@other.com"
            ]
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn search_match_all() {
        let lot = seed_search_lot().await;
        let q = Query::from_str("~.*").unwrap();
        assert_eq!(lot.index().search(&q).count(), 5);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn search_no_matches() {
        let lot = seed_search_lot().await;
        let q = Query::from_str("nonesuch").unwrap();
        assert!(lot.index().search(&q).next().is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn deleted_record_absent_from_index() {
        let (db, _user, mut lot) = setup().await;
        let record = Record::new(
            &lot,
            "ephemeral".parse::<Label>().unwrap(),
            Data::new("x".try_into().unwrap()),
        );
        record.save(&db, &mut lot).await.unwrap();
        assert_eq!(1, lot.index().len());
        record.delete(&db, &mut lot).await.unwrap();
        assert!(lot.index().is_empty());
    }
}
