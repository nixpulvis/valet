use sqlx::{Pool, Sqlite, SqlitePool};
use url::Url;

pub const DEFAULT_URL: &'static str = "valet.sqlite";

pub struct Database(SqlitePool);

impl Database {
    pub async fn new(input: &str) -> Result<Database, Error> {
        let url = Self::parse_url(input)?;
        let pool: Pool<Sqlite> = SqlitePool::connect(&url).await?;

        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .map_err(|e| sqlx::Error::from(e))?;

        Ok(Database(pool))
    }

    pub(crate) fn pool(&self) -> &SqlitePool {
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
    Sqlx(sqlx::Error),
    Url(url::ParseError),
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

pub(crate) mod lots;
pub(crate) mod records;
pub(crate) mod user_lots;
pub(crate) mod users;
