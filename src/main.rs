use std::env;
use tokio;
use valet::db::{DEFAULT_URL, Database, Lots, Users};
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
            "add" => {
                let username = args.nth(0);
                let password = args.nth(0);
                let data = args.nth(0);
                if let (Some(username), Some(password), Some(data)) = (username, password, data) {
                    let db = Database::new(DEFAULT_URL).await?;
                    let registration = Users::registration(&db, &username).await?;
                    let valid = registration.validate(&password).expect("validation failed");
                    if valid {
                        // TODO: Avoid regenerating the credential after validation...
                        let credential = registration
                            .credential(&password)
                            .expect("credential generation failed");
                        let encrypted = credential
                            .encrypt(data.as_bytes())
                            .expect("failed to encrypt");
                        Lots::create(&db, &registration.username, &encrypted).await?;
                    } else {
                        println!("Invalid password.")
                    }
                }
            }
            "get" => {
                let username = args.nth(0);
                let password = args.nth(0);
                if let (Some(username), Some(password)) = (username, password) {
                    let db = Database::new(DEFAULT_URL).await?;
                    let registration = Users::registration(&db, &username).await?;
                    let valid = registration.validate(&password).expect("validation failed");
                    if valid {
                        // TODO: Avoid regenerating the credential after validation...
                        let credential = registration
                            .credential(&password)
                            .expect("credential generation failed");
                        let encrypted = Lots::encrypted(&db, &registration.username).await?;
                        let bytes = credential.decrypt(&encrypted).expect("failed to decrypt");
                        let data = std::str::from_utf8(&bytes).expect("failed to parse data");
                        println!("{}", data);
                    } else {
                        println!("Invalid password.")
                    }
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
