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
                let username = args.nth(0);
                let password = args.nth(0);
                if let (Some(username), Some(password)) = (username, password) {
                    let db = Database::new(DEFAULT_URL).await?;
                    let registration = Registration::new(&username, &password)
                        .expect("error generating registering");
                    Users::create(&db, &registration).await?;
                } else {
                    println!("No username/password given.")
                }
            }
            "validate" => {
                let username = args.nth(0);
                let password = args.nth(0);
                if let (Some(username), Some(password)) = (username, password) {
                    let db = Database::new(DEFAULT_URL).await?;
                    let registration = Users::registration(&db, &username).await?;
                    let valid = registration.validate(&password).expect("validation failed");
                    println!("{}", valid);
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
