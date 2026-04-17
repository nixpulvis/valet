//! In-process stub that returns hardcoded records without going through a
//! socket. Used by the macOS extension until the daemon lands, and handy for
//! tests.

use crate::client::Error;
use valet::{
    Lot, Record,
    password::Password,
    record::{Data, Label},
    uuid::Uuid,
};


pub struct StubClient {
    records: Vec<Record>,
}

impl StubClient {
    pub fn new() -> Self {
        let lot = Lot::new("stub");
        let records = vec![
            Record::new(
                &lot,
                Data::new(
                    Label::Simple("ycombinator.com".into()),
                    "hunter22".try_into().unwrap(),
                )
                .add_extra("username".into(), "alice".into())
                .add_extra("url".into(), "https://news.ycombinator.com".into()),
            ),
            Record::new(
                &lot,
                Data::new(
                    Label::Simple("example.com".into()),
                    "correct horse battery".try_into().unwrap(),
                )
                .add_extra("username".into(), "bob".into())
                .add_extra("url".into(), "https://example.com".into()),
            ),
        ];
        StubClient { records }
    }

    pub fn unlock(&mut self, _username: &str, _password: Password) -> Result<String, Error> {
        Ok("stub-session".to_string())
    }

    pub fn list(&mut self, _service_identifiers: &[String]) -> Result<Vec<Record>, Error> {
        Ok(self.records.iter().map(clone_record).collect())
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
    fn list_returns_all_records_regardless_of_identifier() {
        let mut stub = StubClient::new();
        assert_eq!(stub.list(&[]).unwrap().len(), 2);
        assert_eq!(stub.list(&["ycombinator.com".to_string()]).unwrap().len(), 2);
        assert_eq!(
            stub.list(&["https://w3schools.com/foo".to_string()])
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn fetch_returns_matching_record() {
        let mut stub = StubClient::new();
        let all = stub.list(&[]).unwrap();
        let uuid = all[0].uuid().clone();
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
