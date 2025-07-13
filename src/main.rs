use rand::RngCore;
use rand::rngs::OsRng;
use sqlx::{Error, Pool, Sqlite, SqlitePool};
use tokio;
use valet::{prelude::*, user};

#[tokio::main]
async fn main() -> Result<(), Error> {
    // Database URL (replace with your actual database URL)
    let database_url = "sqlite://valet.sqlite?mode=rwc";

    // Create a connection pool
    let pool: Pool<Sqlite> = SqlitePool::connect(database_url).await?;

    println!("Connected to the database!");

    sqlx::migrate!("./migrations").run(&pool).await?;

    println!("Migrations up to date.");

    let mut rng = rand::thread_rng();
    let username = rng.next_u32();
    sqlx::query(
        r"
        INSERT INTO users (username, password_hash, password_salt)
        VALUES (?, 'asd', 'asd')
    ",
    )
    .bind(username)
    .execute(&pool)
    .await?;

    let mut rng = rand::thread_rng();
    let uuid = rng.next_u32();
    sqlx::query(
        r"
        INSERT INTO lots (uuid, username, label)
        VALUES (?, ?, 'example')
    ",
    )
    .bind(uuid)
    .bind(username)
    .execute(&pool)
    .await?;

    println!("Inserted!");

    // Example query
    let rows: Vec<(String,)> = sqlx::query_as("SELECT username FROM users")
        .fetch_all(&pool)
        .await?;

    // Iterate and print results
    for row in rows {
        dbg!(row);
    }

    Ok(())
}
