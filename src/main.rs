use std::env;
use tokio;
use valet::db::{DEFAULT_URL, Database, Users};
use valet::prelude::*;

#[tokio::main]
async fn main() -> Result<(), sqlx::Error> {
    let mut args = env::args();

    // TODO: Import a proper CLI arg parser.
    if let Some(command) = args.nth(1) {
        match command.as_str() {
            "register" => {
                if let Some(username) = args.nth(0) {
                    let db = Database::new(DEFAULT_URL).await?;
                    let registration =
                        Registration::new(&username).expect("error generating registering");
                    Users::create(&db, &registration).await?;
                } else {
                    println!("No username given.")
                }
            }
            _ => {
                println!("Unknown command.")
            }
        }
    } else {
        println!("No command given.")
    }

    Ok(())
}
