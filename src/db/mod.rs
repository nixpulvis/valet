use sqlx::{Error, Pool, Sqlite, SqlitePool};

pub const DEFAULT_URL: &'static str = "sqlite://valet.sqlite?mode=rwc";

pub struct Database(SqlitePool);

impl Database {
    pub async fn new(url: &str) -> Result<Database, Error> {
        let pool: Pool<Sqlite> = SqlitePool::connect(url).await?;
        println!("Connected to the database!");

        sqlx::migrate!("./migrations").run(&pool).await?;
        println!("Migrations up to date.");

        Ok(Database(pool))
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.0
    }
}

mod users;
pub use self::users::Users;
// pub mod lots;
