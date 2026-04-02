use serde::{Deserialize, Serialize};
use std::fmt;

/// An exportable representation of a Lot with all records in plaintext.
///
/// This format contains the lot's metadata and all records in plaintext.
/// It can be serialized to JSON for export or encrypted with a user key
/// for backup purposes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExportedLot {
    /// UUID of the lot
    pub uuid: String,
    /// Name of the lot
    pub name: String,
    /// The lot key in hex encoding for JSON compatibility
    pub key: String,
    /// All records in the lot, with plaintext data
    pub records: Vec<ExportedRecord>,
}

/// An individual record as it appears in an export.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExportedRecord {
    /// UUID of the record
    pub uuid: String,
    /// UUID of the lot this record belongs to
    pub lot_uuid: String,
    /// The label of the record
    pub label: String,
    /// The password/secret value
    pub value: String,
}

#[derive(Debug)]
pub enum Error {
    Serialization(serde_json::Error),
}

impl From<serde_json::Error> for Error {
    fn from(err: serde_json::Error) -> Self {
        Error::Serialization(err)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Serialization(e) => write!(f, "serialization error: {}", e),
        }
    }
}

impl ExportedLot {
    /// Serialize the exported lot to JSON.
    pub fn to_json(&self) -> Result<Vec<u8>, Error> {
        Ok(serde_json::to_vec_pretty(self)?)
    }

    /// Deserialize an exported lot from JSON.
    pub fn from_json(json: &[u8]) -> Result<Self, Error> {
        Ok(serde_json::from_slice(json)?)
    }
}
