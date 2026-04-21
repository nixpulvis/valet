//! In-process stub that returns hardcoded records without going through a
//! socket. Used by the macOS extension until the daemon lands, and handy for
//! tests.
//!
//! The stub implements the same method surface as [`crate::client::Client`]
//! so the two can be swapped behind the FFI layer via a cargo feature. Not
//! every method is useful in the stub. Record creation is a no-op that
//! echoes back what was asked for, idle timeouts do not exist, etc.

use crate::client::Error;
use std::collections::HashMap;
use valet::{
    Lot, Record,
    password::Password,
    record::{Data, Label, LabelName, Query},
    uuid::Uuid,
};

pub struct StubClient {
    lot_name: String,
    records: Vec<Record>,
    active_user: Option<String>,
}

const LOT_NAME: &str = "stub";
const STUB_USER: &str = "stub-user";
// Fixed UUIDs so the macOS App and Extension processes, which each
// instantiate their own StubClient, agree on record identity. The App
// writes these uuids into ASCredentialIdentityStore; the Extension's
// fetch path resolves an autofill request back to the same record by
// looking them up here. Randomizing per process would break autofill.
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
            active_user: None,
        }
    }

    pub fn status(&mut self) -> Result<Vec<String>, Error> {
        Ok(self.active_user.iter().cloned().collect())
    }

    pub fn list_users(&mut self) -> Result<Vec<String>, Error> {
        Ok(vec![STUB_USER.to_owned()])
    }

    pub fn unlock(&mut self, username: &str, _password: Password) -> Result<(), Error> {
        self.active_user = Some(username.to_owned());
        Ok(())
    }

    pub fn lock(&mut self, username: &str) -> Result<(), Error> {
        if self.active_user.as_deref() == Some(username) {
            self.active_user = None;
        }
        Ok(())
    }

    pub fn lock_all(&mut self) -> Result<(), Error> {
        self.active_user = None;
        Ok(())
    }

    pub fn list(
        &mut self,
        _username: &str,
        queries: &[String],
    ) -> Result<Vec<(Uuid<Record>, Label)>, Error> {
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

    pub fn fetch(&mut self, _username: &str, uuid: &Uuid<Record>) -> Result<Record, Error> {
        let needle = uuid.to_uuid();
        for r in &self.records {
            if r.uuid().to_uuid() == needle {
                return Ok(clone_record(r));
            }
        }
        Err(Error::Remote(format!("no record with uuid {uuid}")))
    }

    pub fn find_records(
        &mut self,
        _username: &str,
        lot: &str,
        query: &str,
    ) -> Result<Vec<(Uuid<Record>, Label)>, Error> {
        if lot != self.lot_name {
            return Ok(Vec::new());
        }
        Ok(self
            .records
            .iter()
            .filter(|r| crate::request::label_matches_domain(r.label(), query))
            .map(|r| (r.uuid().clone(), r.label().clone()))
            .collect())
    }

    pub fn get_record(
        &mut self,
        username: &str,
        _lot: &str,
        uuid: &Uuid<Record>,
    ) -> Result<Record, Error> {
        self.fetch(username, uuid)
    }

    pub fn create_record(
        &mut self,
        _username: &str,
        _lot: &str,
        _label: Label,
        _password: Password,
        _extra: HashMap<String, String>,
    ) -> Result<Record, Error> {
        Err(Error::Remote("stub: create_record not supported".into()))
    }

    pub fn generate_record(
        &mut self,
        _username: &str,
        _lot: &str,
        _label: Label,
    ) -> Result<Record, Error> {
        Err(Error::Remote("stub: generate_record not supported".into()))
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
        assert_eq!(stub.list(STUB_USER, &[]).unwrap().len(), 2);
    }

    #[test]
    fn list_filters_by_literal_query() {
        let mut stub = StubClient::new();
        let hits = stub
            .list(STUB_USER, &["stub::ycombinator.com".to_string()])
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(
            hits[0].1.name(),
            &LabelName::Simple("ycombinator.com".into())
        );
    }

    #[test]
    fn list_filters_by_regex_query() {
        let mut stub = StubClient::new();
        let hits = stub.list(STUB_USER, &["stub::~.*".to_string()]).unwrap();
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn list_non_matching_query_returns_empty() {
        let mut stub = StubClient::new();
        assert!(
            stub.list(STUB_USER, &["stub::nope".to_string()])
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn list_invalid_query_errors() {
        let mut stub = StubClient::new();
        match stub.list(STUB_USER, &["foo<k=v".to_string()]) {
            Err(Error::Remote(msg)) => assert!(msg.contains("invalid query")),
            other => panic!("expected Remote error, got {other:?}"),
        }
    }

    #[test]
    fn fetch_returns_matching_record() {
        let mut stub = StubClient::new();
        let all = stub.list(STUB_USER, &[]).unwrap();
        let uuid = all[0].0.clone();
        let got = stub.fetch(STUB_USER, &uuid).unwrap();
        assert_eq!(got.uuid().to_uuid(), uuid.to_uuid());
    }

    #[test]
    fn fetch_missing_returns_error() {
        let mut stub = StubClient::new();
        let bogus: Uuid<Record> = Uuid::parse("00000000-0000-0000-0000-000000000000").unwrap();
        assert!(matches!(
            stub.fetch(STUB_USER, &bogus),
            Err(Error::Remote(_))
        ));
    }
}
