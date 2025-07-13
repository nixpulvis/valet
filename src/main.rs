use std::env;
use tokio;
use valet::db::{DEFAULT_URL, Database, Lots, Users};

#[tokio::main]
async fn main() -> Result<(), valet::db::Error> {
    let mut args = env::args();

    // TODO: Import a proper CLI arg parser.
    if let Some(command) = args.nth(1) {
        match command.as_str() {
            "register" => {
                let username = args.nth(0);
                let password = args.nth(0);
                if let (Some(username), Some(password)) = (username, password) {
                    let db = Database::new(DEFAULT_URL).await?;
                    Users::register(&db, &username, &password).await?;
                } else {
                    println!("No username/password given.")
                }
            }
            "validate" => {
                let username = args.nth(0);
                let password = args.nth(0);
                if let (Some(username), Some(password)) = (username, password) {
                    let db = Database::new(DEFAULT_URL).await?;
                    let user = Users::get(&db, &username, &password).await?;
                    println!("{} validated", user.username);
                }
            }
            "put" => {
                let username = args.nth(0);
                let password = args.nth(0);
                let data = args.nth(0);
                if let (Some(username), Some(password), Some(data)) = (username, password, data) {
                    let db = Database::new(DEFAULT_URL).await?;
                    let user = Users::get(&db, &username, &password).await?;
                    let encrypted = user
                        .credential()
                        .encrypt(data.as_bytes())
                        .expect("failed to encrypt");
                    Lots::create(&db, &user.username, &encrypted).await?;
                } else {
                    println!("No username/password/data given.")
                }
            }
            "get" => {
                let username = args.nth(0);
                let password = args.nth(0);
                if let (Some(username), Some(password)) = (username, password) {
                    let db = Database::new(DEFAULT_URL).await?;
                    let user = Users::get(&db, &username, &password).await?;
                    let encrypted = Lots::get(&db, &user.username).await?;
                    let bytes = user
                        .credential()
                        .decrypt(&encrypted)
                        .expect("failed to decrypt");
                    let data = std::str::from_utf8(&bytes).expect("failed to parse data");
                    println!("{}", data);
                } else {
                    println!("No username/password given.")
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
