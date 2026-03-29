use sea_orm::DatabaseConnection;
use sqlx::{SqlitePool, sqlite::SqliteConnectOptions};
use std::str::FromStr;
use url::Url;

pub const DEFAULT_URL: &'static str = "valet.sqlite";

pub struct Database(DatabaseConnection);

impl Database {
    pub async fn new(input: &str) -> Result<Database, Error> {
        let url = Self::parse_url(input)?;

        // Create the sqlx pool and run migrations on it.
        let opts = SqliteConnectOptions::from_str(&url)?.pragma("foreign_keys", "ON");
        let pool = SqlitePool::connect_with(opts).await?;
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .map_err(|e| sqlx::Error::from(e))?;

        // Convert to a sea-orm connection backed by the same pool.
        let db = sea_orm::SqlxSqliteConnector::from_sqlx_sqlite_pool(pool);
        Ok(Database(db))
    }

    pub(crate) fn connection(&self) -> &DatabaseConnection {
        &self.0
    }

    fn parse_url(input: &str) -> Result<String, Error> {
        // Apply default base.
        let result = Url::parse(input).or_else(|err| match err {
            url::ParseError::RelativeUrlWithoutBase => {
                Ok(Url::parse(&format!("sqlite://{}", input))?)
            }
            _ => Err(Error::Url(err)),
        });

        // Apply default mode.
        let result = result.map(|mut url| {
            if !url.query_pairs().any(|(k, _)| k == "mode") {
                url.query_pairs_mut().append_pair("mode", "rwc");
            }
            url
        });

        result.map(|url| url.to_string()).or_else(|err| match err {
            Error::Url(url::ParseError::EmptyHost) => Ok("sqlite://:memory:".into()),
            _ => Err(err),
        })
    }
}

#[derive(Debug)]
pub enum Error {
    SeaOrm(sea_orm::DbErr),
    Sqlx(sqlx::Error),
    Url(url::ParseError),
}

impl From<sea_orm::DbErr> for Error {
    fn from(err: sea_orm::DbErr) -> Self {
        Error::SeaOrm(err)
    }
}

impl From<sqlx::Error> for Error {
    fn from(err: sqlx::Error) -> Self {
        Error::Sqlx(err)
    }
}

impl From<url::ParseError> for Error {
    fn from(err: url::ParseError) -> Self {
        Error::Url(err)
    }
}
