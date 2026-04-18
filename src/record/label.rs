use crate::{encrypt::Stash, lot::Lot};
use bitcode::{Decode, Encode};
use std::{cmp::Ordering, fmt, str::FromStr};

#[derive(Encode, Decode, Debug, Eq, PartialEq, Hash, Clone)]
pub enum Label {
    Simple(String),
    Domain { id: String, domain: String },
}

impl Stash<Lot> for Label {}

impl Ord for Label {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Label::Simple(a), Label::Simple(b)) => a.cmp(b),
            (
                Label::Domain {
                    id: a_id,
                    domain: a_domain,
                },
                Label::Domain {
                    id: b_id,
                    domain: b_domain,
                },
            ) => a_domain.cmp(b_domain).then_with(|| a_id.cmp(b_id)),
            (Label::Domain { .. }, Label::Simple(_)) => Ordering::Less,
            (Label::Simple(_), Label::Domain { .. }) => Ordering::Greater,
        }
    }
}

impl PartialOrd for Label {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl FromStr for Label {
    type Err = Error;

    fn from_str(label: &str) -> Result<Self, Self::Err> {
        if let Some((id, domain)) = label.rsplit_once("@") {
            if id.trim().is_empty() {
                Err(Error::MissingId)
            } else if domain.trim().is_empty() {
                Err(Error::MissingDomain)
            } else {
                if id.contains(char::is_whitespace) {
                    Err(Error::InvalidId)
                } else if domain.contains(char::is_whitespace) {
                    Err(Error::InvalidDomain)
                } else {
                    Ok(Label::Domain {
                        id: id.into(),
                        domain: domain.into(),
                    })
                }
            }
        } else {
            if label.trim().is_empty() {
                Err(Error::MissingId)
            } else {
                Ok(Label::Simple(label.into()))
            }
        }
    }
}

impl fmt::Display for Label {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Label::Simple(label) => write!(f, "{label}"),
            Label::Domain { id, domain } => write!(f, "{id}@{domain}"),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    MissingId,
    InvalidId,
    MissingDomain,
    InvalidDomain,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lot::Lot;

    #[test]
    fn encode_decode() {
        let label = Label::Simple("foo".into());
        let encoded = label.encode();
        let decoded = Label::decode(&encoded).expect("failed to decode");
        assert_eq!(label, decoded);
    }

    #[test]
    fn encrypt_decrypt() {
        let lot = Lot::new("test");
        let label = Label::Simple("foo".into());
        let encrypted = label.encrypt(lot.key()).expect("failed to encrypt");
        let decrypted = Label::decrypt(&encrypted, lot.key()).expect("failed to decrypt");
        assert_eq!(label, decrypted);
    }

    #[test]
    fn encrypt_decrypt_with_aad() {
        let lot = Lot::new("test");
        let label = Label::Simple("foo".into());
        let aad = [1, 2, 3];
        let encrypted = label
            .encrypt_with_aad(lot.key(), &aad)
            .expect("failed to encrypt");
        let decrypted =
            Label::decrypt_with_aad(&encrypted, lot.key(), &aad).expect("failed to decrypt");
        assert_eq!(label, decrypted);
    }

    #[test]
    fn ord_domain_sorts_by_domain_then_id() {
        let mut labels = vec![
            Label::Domain {
                id: "bob".into(),
                domain: "example.com".into(),
            },
            Label::Domain {
                id: "alice".into(),
                domain: "zeta.com".into(),
            },
            Label::Domain {
                id: "alice".into(),
                domain: "example.com".into(),
            },
        ];
        labels.sort();
        assert_eq!(
            labels,
            vec![
                Label::Domain {
                    id: "alice".into(),
                    domain: "example.com".into(),
                },
                Label::Domain {
                    id: "bob".into(),
                    domain: "example.com".into(),
                },
                Label::Domain {
                    id: "alice".into(),
                    domain: "zeta.com".into(),
                },
            ]
        );
    }

    #[test]
    fn parse_simple() {
        let label = "label".parse::<Label>().unwrap();
        assert_eq!(label, Label::Simple("label".into()));
    }

    #[test]
    fn parse_domain() {
        let username = "user@example.com".parse::<Label>().unwrap();
        assert!(
            matches!(username, Label::Domain { id, domain } if id == "user" && domain == "example.com")
        );
        let email = "user@email.com@example.com".parse::<Label>().unwrap();
        assert!(
            matches!(email, Label::Domain { id, domain } if id == "user@email.com" && domain == "example.com")
        );

        assert_eq!("".parse::<Label>(), Err(Error::MissingId));
        assert_eq!(" \t".parse::<Label>(), Err(Error::MissingId));
        assert_eq!(" @\t".parse::<Label>(), Err(Error::MissingId));
        assert_eq!("user@".parse::<Label>(), Err(Error::MissingDomain));
        assert_eq!("user@\t".parse::<Label>(), Err(Error::MissingDomain));
        assert_eq!("user@@".parse::<Label>(), Err(Error::MissingDomain));
        assert_eq!("user@@ ".parse::<Label>(), Err(Error::MissingDomain));
        assert_eq!("user @email.com".parse::<Label>(), Err(Error::InvalidId));
        assert_eq!(
            "user@\temail.com".parse::<Label>(),
            Err(Error::InvalidDomain)
        );
        assert_eq!(
            "user @email.com@example.com".parse::<Label>(),
            Err(Error::InvalidId)
        );
        assert_eq!(
            "user@ email.com@example.com".parse::<Label>(),
            Err(Error::InvalidId)
        );
    }
}
