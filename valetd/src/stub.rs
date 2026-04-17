//! In-process stub that returns hardcoded records without going through a
//! socket. Used by the macOS extension until the daemon lands, and handy for
//! tests.

use crate::client::Error;
use valet::{
    Lot, Record,
    password::Password,
    record::{Data, Label, LabelName, Query},
    uuid::Uuid,
};

pub struct StubClient {
    lot_name: String,
    records: Vec<Record>,
}

// Fixed UUIDs so the App and Extension processes, which each instantiate
// their own StubClient, agree on record identity. Without this, the
// credential identity store the App writes cannot be resolved from the
// Extension's fetch path.
const LOT_NAME: &str = "stub";
const YCOMBINATOR_UUID: &str = "01900000-0000-7000-8000-00000000a1c0";
const EXAMPLE_UUID: &str = "01900000-0000-7000-8000-00000000e8a3";

impl StubClient {
    pub fn new() -> Self {
        let lot = Lot::new(LOT_NAME);
        let records = vec![
            Record::with_uuid(
                Uuid::parse(YCOMBINATOR_UUID).unwrap(),
                &lot,
                Label::from(LabelName::Simple("ycombinator.com".into()))
                    .add_extra("username", "alice")
                    .unwrap()
                    .add_extra("url", "https://news.ycombinator.com")
                    .unwrap(),
                Data::new("hunter22".try_into().unwrap()),
            ),
            Record::with_uuid(
                Uuid::parse(EXAMPLE_UUID).unwrap(),
                &lot,
                Label::from(LabelName::Simple("example.com".into()))
                    .add_extra("username", "bob")
                    .unwrap()
                    .add_extra("url", "https://example.com")
                    .unwrap(),
                Data::new("correct horse battery".try_into().unwrap()),
            ),
        ];
        StubClient {
            lot_name: lot.name().to_string(),
            records,
        }
    }

    pub fn unlock(&mut self, _username: &str, _password: Password) -> Result<String, Error> {
        Ok("stub-session".to_string())
    }

    pub fn list(&mut self, queries: &[String]) -> Result<Vec<(Uuid<Record>, Label)>, Error> {
        let parsed = queries
            .iter()
            .map(|s| s.parse::<Query>())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| Error::Remote(format!("invalid query: {e}")))?;
        Ok(self
            .records
            .iter()
            .filter(|r| {
                parsed.is_empty()
                    || parsed
                        .iter()
                        .any(|q| q.matches_lot(&self.lot_name) && q.matches_label(r.label()))
            })
            .map(|r| (r.uuid().clone(), r.label().clone()))
            .collect())
    }

    pub fn fetch(&mut self, uuid: &Uuid<Record>) -> Result<Record, Error> {
        let needle = uuid.to_uuid();
        for r in &self.records {
            if r.uuid().to_uuid() == needle {
                return Ok(clone_record(r));
            }
        }
        Err(Error::Remote(format!("no record with uuid {uuid}")))
    }
}

impl Default for StubClient {
    fn default() -> Self {
        Self::new()
    }
}

// Record doesn't implement Clone, but it does derive bitcode Encode/Decode,
// so round-tripping through a buffer gives us a deep copy.
fn clone_record(r: &Record) -> Record {
    bitcode::decode(&bitcode::encode(r)).expect("record round-trip")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_empty_queries_returns_all() {
        let mut stub = StubClient::new();
        assert_eq!(stub.list(&[]).unwrap().len(), 2);
    }

    #[test]
    fn list_filters_by_literal_query() {
        let mut stub = StubClient::new();
        let hits = stub.list(&["stub::ycombinator.com".to_string()]).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(
            hits[0].1.name(),
            &LabelName::Simple("ycombinator.com".into())
        );
    }

    #[test]
    fn list_filters_by_regex_query() {
        let mut stub = StubClient::new();
        // Match every label in lot "stub".
        let hits = stub.list(&["stub::~.*".to_string()]).unwrap();
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn list_non_matching_query_returns_empty() {
        let mut stub = StubClient::new();
        assert!(stub.list(&["stub::nope".to_string()]).unwrap().is_empty());
    }

    #[test]
    fn list_invalid_query_errors() {
        let mut stub = StubClient::new();
        match stub.list(&["foo<k=v".to_string()]) {
            Err(Error::Remote(msg)) => assert!(msg.contains("invalid query")),
            other => panic!("expected Remote error, got {other:?}"),
        }
    }

    #[test]
    fn fetch_returns_matching_record() {
        let mut stub = StubClient::new();
        let all = stub.list(&[]).unwrap();
        let uuid = all[0].0.clone();
        let got = stub.fetch(&uuid).unwrap();
        assert_eq!(got.uuid().to_uuid(), uuid.to_uuid());
    }

    #[test]
    fn fetch_missing_returns_error() {
        let mut stub = StubClient::new();
        let bogus: Uuid<Record> = Uuid::parse("00000000-0000-0000-0000-000000000000").unwrap();
        assert!(matches!(stub.fetch(&bogus), Err(Error::Remote(_))));
    }
}
