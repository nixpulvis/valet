use clap::{Parser, Subcommand};
use clap_repl::ClapEditor;
use clap_repl::reedline::{DefaultPrompt, DefaultPromptSegment, FileBackedHistory};
use std::io::{self, Write};
use tokio;
use valet::{lot, prelude::*};

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
    List {
        #[clap(default_value = "")]
        path: String,
    },
    Put {
        path: String,
        data: String,
    },
    Get {
        path: String,
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
                        .use_bracketed_paste(true)
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
                Repl::List { path } => {
                    let path = Path::parse(&path);
                    let lot = Lot::load(&db, &path.lot, &user)
                        .await
                        .expect("failed to load lot");

                    // TODO: lot.records() : IntoIter
                    for record in lot.records.borrow().iter() {
                        let record = record.borrow();
                        let label = record.data.label();
                        if label.starts_with(&path.label) {
                            // TODO: impl Display for Path.
                            println!("{:?}", Path::new(&path.lot, label));
                        }
                    }
                }
                Repl::Put { path, data } => {
                    let path = Path::parse(&path);
                    let lot = Lot::load(&db, &path.lot, &user)
                        .await
                        .expect("failed to load lot");
                    // TODO: Delete old record if it exists.
                    // TODO: Add deleted record to new record's history.
                    lot.insert_record(&db, RecordData::plain(&path.label, &data))
                        .await
                        .expect("failed to insert record");
                }
                Repl::Get { path } => {
                    let path = Path::parse(&path);
                    let lot = Lot::load(&db, &path.lot, &user)
                        .await
                        .expect("failed to load lot");
                    if let Some(record) = lot
                        .records
                        .borrow()
                        .iter()
                        .find(|r| r.borrow().data.label() == path.label)
                    {
                        println!("{}", record.borrow());
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

// TODO: Move into lib.
#[derive(Debug, PartialEq, Eq)]
struct Path {
    lot: String,
    label: String,
}

impl Path {
    fn new(lot: &str, label: &str) -> Self {
        Path {
            lot: lot.into(),
            label: label.into(),
        }
    }

    fn parse(path: &str) -> Self {
        let parts: Vec<&str> = path.rsplitn(2, "::").collect();
        if parts.len() == 1 || (parts.len() == 2 && parts[1] == "") {
            Path {
                lot: lot::DEFAULT_LOT.into(),
                label: parts[0].into(),
            }
        } else if parts.len() > 1 {
            Path {
                lot: parts[1].into(),
                label: parts[0].into(),
            }
        } else {
            unreachable!();
        }
    }
}

#[test]
fn test_parse_path() {
    assert_eq!(
        Path {
            lot: "main".into(),
            label: "".into()
        },
        Path::parse("")
    );
    assert_eq!(
        Path {
            lot: "main".into(),
            label: "".into()
        },
        Path::parse("::")
    );
    assert_eq!(
        Path {
            lot: "lot".into(),
            label: "".into(),
        },
        Path::parse("lot::")
    );
    assert_eq!(
        Path {
            lot: "main".into(),
            label: "label".into()
        },
        Path::parse("label")
    );
    assert_eq!(
        Path {
            lot: "lot".into(),
            label: "label".into()
        },
        Path::parse("lot::label")
    );
    assert_eq!(
        Path {
            lot: "lot::sublot".into(),
            label: "label".into()
        },
        Path::parse("lot::sublot::label")
    );
}

// TODO: Error handling.
fn get_password() -> String {
    print!("Password: ");
    io::stdout().flush().ok();
    // TODO: Is there a better way to try to hide the password in memory?
    rpassword::read_password().unwrap()
}
