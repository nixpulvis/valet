use crate::encrypt::{self, Encrypted, Key};
use crate::record::Record;
use std::collections::HashMap;

/// An encrypted collection of secrets.
pub struct Lot {
    username: String,
    uuid: String, // TODO: Uuid type
    main: bool,
    encrypted: Encrypted,
}

impl Lot {
    pub fn unlock(&self, key: &Key) -> Result<UnlockedLot, encrypt::Error> {
        let _bytes = key.decrypt(&self.encrypted)?;
        // TODO: uncompress/deserialize
        let records = HashMap::new();

        Ok(UnlockedLot {
            username: self.username.clone(),
            uuid: self.uuid.clone(),
            main: self.main.clone(),
            records,
        })
    }
}

/// A decrypted collections of secrets.
///
/// Records are indexed by their label.
pub struct UnlockedLot {
    username: String,
    uuid: String,
    main: bool,
    records: HashMap<String, Record>,
}

impl UnlockedLot {
    pub fn new(username: &str) -> Self {
        UnlockedLot {
            username: username.into(),
            // TODO: Generate actual Uuid
            uuid: "1".into(),
            main: false,
            records: HashMap::new(),
        }
    }

    pub fn lock(&self, key: &Key) -> Result<Lot, encrypt::Error> {
        // TODO: serialize/compress
        let encrypted = key.encrypt(b"TODO")?;
        Ok(Lot {
            username: self.username.clone(),
            uuid: self.uuid.clone(),
            main: self.main.clone(),
            encrypted,
        })
    }
    pub fn put(&mut self, record: Record) {
        self.records.insert(record.label().into(), record);
    }

    pub fn get(&self, index: &str) -> &Record {
        &self.records[index]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::Record;
    use crate::user::User;

    #[test]
    fn put_get() {
        let user = User::new("nixpulvis", "password").expect("failed to make user");
        let mut lot = UnlockedLot::new(&user.username);

        let records = [
            Record::plain("a", "b"),
            Record::domain("a.com", HashMap::from([("foo".into(), "bar".into())])),
        ];

        for record in records.iter().cloned() {
            lot.put(record);
        }

        for record in records.iter() {
            assert_eq!(record, lot.get(record.label()));
        }
    }

    #[test]
    fn lock_unlock() {
        let user = User::new("nixpulvis", "password").expect("failed to make user");
        let mut unlocked = UnlockedLot::new(&user.username);

        unlocked.put(Record::plain("test", "secret"));
        unlocked.put(Record::domain(
            "test",
            HashMap::from_iter([("a".into(), "y".into()), ("b".into(), "z".into())]),
        ));

        let locked = unlocked.lock(&user.key()).expect("failed to lock lot");
        assert_eq!(unlocked.username, locked.username);
        assert_eq!(unlocked.uuid, locked.uuid);
        assert_eq!(unlocked.main, locked.main);

        let reunlocked = locked.unlock(&user.key()).expect("failed to unlock lot");
        assert_eq!(unlocked.username, reunlocked.username);
        assert_eq!(unlocked.uuid, reunlocked.uuid);
        assert_eq!(unlocked.main, reunlocked.main);
        assert_eq!(unlocked.records, reunlocked.records);
    }
}
