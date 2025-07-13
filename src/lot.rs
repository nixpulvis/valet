use crate::user::Credential;

/// Storage for encrypted secrets.
#[derive(Default, Debug)]
pub struct Lot();

impl Lot {
    pub fn unlock(&mut self, user: &str, password: &str) {}

    pub fn add(&mut self, index: &str, secret: &str) {}
    pub fn get(&self, index: &str) -> &str {
        "foo"
    }
}

pub struct UnlockedLot(Lot, Credential);

impl UnlockedLot {
    pub fn lock(&self) -> Lot {
        Lot()
    }
    pub fn add(&mut self, index: &str, secret: &str) {}
    pub fn get(&self, index: &str) -> &str {
        "foo"
    }
}
