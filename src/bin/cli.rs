use clap::{Parser, Subcommand};
use tokio;
use valet::prelude::*;

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    // TODO: Remove password from CLI, it should be prompted.
    Validate {
        username: String,
        password: String,
    },
    Register {
        username: String,
        password: String,
    },

    // TODO: Move rest of commands into baked in REPL under unlock subcommand.
    // Unlock {
    //     username: String,
    // },
    Put {
        username: String,
        password: String,
        data: String,
    },
    Get {
        username: String,
        password: String,
    },
}

#[tokio::main]
async fn main() -> Result<(), valet::db::Error> {
    let cli = Cli::parse();
    let db = Database::new(valet::db::DEFAULT_URL).await?;

    match &cli.command {
        Command::Validate { username, password } => {
            let user = valet::db::Users::get(&db, &username, &password).await?;
            println!("{} validated", user.username);
        }
        Command::Register { username, password } => {
            let user = valet::db::Users::register(&db, &username, &password).await?;
            println!("{} registered", user.username);
        }
        Command::Put {
            username,
            password,
            data,
        } => {
            let user = valet::db::Users::get(&db, &username, &password).await?;
            let encrypted = user
                .key()
                .encrypt(data.as_bytes())
                .expect("failed to encrypt");
            valet::db::Lots::create(&db, &user.username, &encrypted).await?;
        }
        Command::Get { username, password } => {
            let user = valet::db::Users::get(&db, &username, &password).await?;
            let encrypted = valet::db::Lots::get(&db, &user.username).await?;
            let bytes = user.key().decrypt(&encrypted).expect("failed to decrypt");
            let data = std::str::from_utf8(&bytes).expect("failed to parse data");
            println!("{}", data);
        }
    }

    Ok(())
}
