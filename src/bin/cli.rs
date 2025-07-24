use clap::{Parser, Subcommand};
use clap_repl::ClapEditor;
use clap_repl::reedline::{DefaultPrompt, DefaultPromptSegment, FileBackedHistory};
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
    Validate { username: String },
    Register { username: String },
    Unlock { username: String },
}

#[derive(Parser)]
enum Repl {
    Put { data: String },
    Get,
    Lock,
}

#[tokio::main]
async fn main() -> Result<(), valet::db::Error> {
    let cli = Cli::parse();
    let db = Database::new(valet::db::DEFAULT_URL).await?;

    let password = get_password();

    match &cli.command {
        Command::Validate { username } => {
            let user = get_user(&db, &username, password).await?;
            println!("{} validated", user.username);
        }
        Command::Register { username } => {
            let user = valet::db::Users::register(&db, &username, password).await?;
            println!("{} registered", user.username);
        }
        Command::Unlock { username } => {
            let user = get_user(&db, &username, password).await?;

            let prompt = DefaultPrompt {
                left_prompt: DefaultPromptSegment::Basic("valet".to_owned()),
                ..DefaultPrompt::default()
            };
            let rl = ClapEditor::<Repl>::builder()
                .with_editor_hook(|reed| {
                    reed.with_history(Box::new(FileBackedHistory::new(0).unwrap()))
                })
                .with_prompt(Box::new(prompt.clone()))
                .build();

            rl.repl_async(async |command| match &command {
                Repl::Put { data } => {
                    let encrypted = user
                        .key()
                        .encrypt(data.as_bytes())
                        .expect("failed to encrypt");
                    // valet::db::Lots::create(&db, &user.username, &encrypted)
                    //     .await
                    //     .ok();
                }
                Repl::Get => {
                    // if let Ok(encrypted) = valet::db::Lots::get(&db, &user.username).await {
                    //     let bytes = user.key().decrypt(&encrypted).expect("failed to decrypt");
                    //     let data = std::str::from_utf8(&bytes).expect("failed to parse data");
                    //     println!("{}", data);
                    // }
                }
                Repl::Lock => {
                    // TODO: There has to be a way to break out of `repl_async`...
                    std::process::exit(0);
                }
            })
            .await;
        }
    }

    Ok(())
}

// TODO: Error handling.
fn get_password() -> String {
    print!("Password: ");
    io::stdout().flush().ok();
    // TODO: Is there a better way to try to hide the password in memory?
    rpassword::read_password().unwrap()
}

async fn get_user(
    db: &Database,
    username: &str,
    password: String,
) -> Result<User, valet::user::Error> {
    if let Ok(u) = valet::db::Users::get(&db, &username, password).await {
        return Ok(u);
    } else {
        return Err(valet::user::Error::InvalidUsernamePassword);
    }
}
