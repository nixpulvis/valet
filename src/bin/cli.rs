use clap::{Parser, Subcommand};
use clap_repl::ClapEditor;
use clap_repl::reedline::{DefaultPrompt, DefaultPromptSegment, FileBackedHistory};
use std::io::{self, Write};
use tokio;
use valet::prelude::*;

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: ValetCommand,
}

#[derive(Subcommand)]
enum ValetCommand {
    // TODO: Remove password from CLI, it should be prompted.
    Validate { username: String },
    Register { username: String },
    Unlock { username: String },
}

#[derive(Parser)]
enum Repl {
    #[command(subcommand)]
    Lot(LotCommand),
    Put {
        lot: String,
        data: String,
    },
    Get {
        lot: String,
    },
    Lock,
}

#[derive(Subcommand)]
enum LotCommand {
    Create { name: String },
    List,
    // Share { name: String, users: Vec<String> },
    // Unshare { name: String, users: Vec<String> },
    Delete { name: String },
}

#[tokio::main]
async fn main() -> Result<(), valet::user::Error> {
    let cli = Cli::parse();
    let db = Database::new(valet::db::DEFAULT_URL).await?;

    let password = get_password();

    match &cli.command {
        ValetCommand::Validate { username } => {
            let user = User::load(&db, &username, password).await?;
            println!("{} validated", user.username);
        }
        ValetCommand::Register { username } => {
            User::new(&username, password)?.register(&db).await?;
            println!("{} registered", username);
        }
        ValetCommand::Unlock { username } => {
            let user = User::load(&db, &username, password).await?;

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
                Repl::Lot(LotCommand::Create { name }) => {
                    Lot::new(&name)
                        .expect("failed to create lot")
                        .save(&db)
                        .await
                        .expect("failed to save lot");
                }
                Repl::Lot(LotCommand::List) => {
                    // TODO: user_lot_keys
                    unimplemented!();
                }
                Repl::Lot(LotCommand::Delete { name }) => {
                    dbg!(&name);
                    unimplemented!();
                }
                Repl::Put { lot, data } => {
                    let lot = Lot::load(&db, &lot, &user)
                        .await
                        .expect("failed to load lot");
                    lot.insert_record(&db, RecordData::plain("data", &data))
                        .await
                        .expect("failed to insert record");
                }
                Repl::Get { lot } => {
                    let lot = Lot::load(&db, &lot, &user)
                        .await
                        .expect("failed to load lot");
                    for record in lot.records.borrow().iter() {
                        match &record.borrow().data {
                            RecordData::Plain(label, value) => {
                                println!("{}: {}", label, value);
                            }
                            RecordData::Domain(label, value) => {
                                println!("{}: {:?}", label, value);
                            }
                        }
                    }
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
