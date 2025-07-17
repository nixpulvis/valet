use clap::{Parser, Subcommand};
use clap_repl::ClapEditor;
use clap_repl::reedline::{DefaultPrompt, DefaultPromptSegment};
use std::io::{self, Write};
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
    Validate { username: String, password: String },
    Register { username: String, password: String },
    Unlock { username: String },
}

#[derive(Parser)]
enum Repl {
    Put { data: String },
    Get,
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
        Command::Unlock { username } => {
            print!("Password: ");
            // TODO: Error handling.
            io::stdout().flush().ok();
            // TODO: Is there a better way to try to hide the password in memory?
            let password = Box::new(rpassword::read_password().unwrap());
            let user;
            if let Ok(u) = valet::db::Users::get(&db, &username, &password).await {
                user = u;
            } else {
                println!("Error getting user.");
                return Ok(());
            }
            drop(password);

            let prompt = DefaultPrompt {
                left_prompt: DefaultPromptSegment::Basic("valet".to_owned()),
                ..DefaultPrompt::default()
            };
            let rl = ClapEditor::<Repl>::builder()
                .with_prompt(Box::new(prompt.clone()))
                .build();

            rl.repl_async(async |command| match &command {
                Repl::Put { data } => {
                    let encrypted = user
                        .key()
                        .encrypt(data.as_bytes())
                        .expect("failed to encrypt");
                    valet::db::Lots::create(&db, &user.username, &encrypted)
                        .await
                        .ok();
                }
                Repl::Get => {
                    if let Ok(encrypted) = valet::db::Lots::get(&db, &user.username).await {
                        let bytes = user.key().decrypt(&encrypted).expect("failed to decrypt");
                        let data = std::str::from_utf8(&bytes).expect("failed to parse data");
                        println!("{}", data);
                    }
                }
            })
            .await;
        }
    }

    Ok(())
}
