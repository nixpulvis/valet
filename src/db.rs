use sea_orm::DatabaseConnection;
use sqlx::SqlitePool;
use std::path::PathBuf;
use url::Url;

/// Default SQLite path: `$XDG_DATA_HOME/valet/valet.sqlite`, falling back to
/// `$HOME/.local/share/valet/valet.sqlite` per the XDG Base Directory spec.
/// Returns an absolute filesystem path (not a `sqlite://` URL).
pub fn default_path() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."));
            home.join(".local").join("share")
        });
    base.join("valet").join("valet.sqlite")
}

pub fn default_url() -> String {
    default_path().to_string_lossy().into_owned()
}

#[derive(Clone)]
pub struct Database(DatabaseConnection);

impl Database {
    pub async fn new(input: &str) -> Result<Database, Error> {
        let url = Self::parse_url(input)?;

        // Make sure the directory the sqlite file lives in exists, otherwise
        // sqlx errors out even with mode=rwc.
        if let Ok(parsed) = Url::parse(&url) {
            let path = parsed.path();
            if !path.is_empty() && path != "/" && path != "/:memory:" {
                if let Some(parent) = std::path::Path::new(path).parent() {
                    if !parent.as_os_str().is_empty() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                }
            }
        }

        // Create the sqlx pool and run migrations on it.
        let pool = SqlitePool::connect(&url).await?;
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
