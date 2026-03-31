use bitcode::{Decode, Encode};
use std::{fmt, str::FromStr};

#[derive(Encode, Decode, Debug, Eq, PartialEq)]
pub enum Label {
    Simple(String),
    Domain { id: String, domain: String },
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
