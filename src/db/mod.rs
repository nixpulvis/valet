use sqlx::{Pool, Sqlite, SqlitePool};

pub const DEFAULT_URL: &'static str = "sqlite://valet.sqlite?mode=rwc";

pub struct Database(SqlitePool);

impl Database {
    pub async fn new(url: &str) -> Result<Database, Error> {
        let pool: Pool<Sqlite> = SqlitePool::connect(url).await?;

        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .map_err(|e| sqlx::Error::from(e))?;

        Ok(Database(pool))
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.0
    }
}

#[derive(Debug)]
pub enum Error {
    Sqlx(sqlx::Error),
}

impl From<sqlx::Error> for Error {
    fn from(err: sqlx::Error) -> Self {
        Error::Sqlx(err)
    }
}

pub(crate) mod lots;
pub(crate) mod records;
pub(crate) mod users;
