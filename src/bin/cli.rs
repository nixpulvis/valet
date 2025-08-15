use clap::{Parser, Subcommand};
use clap_repl::ClapEditor;
use clap_repl::reedline::{DefaultPrompt, DefaultPromptSegment, FileBackedHistory};
use std::collections::HashMap;
use std::fmt;
use std::fs::File;
use std::io::Write;
use tokio;
use valet::prelude::*;

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: ValetCommand,

    #[arg(short, long, default_value = valet::db::DEFAULT_URL)]
    database: String,
}

#[derive(Subcommand)]
enum ValetCommand {
    Validate {
        username: String,
    },
    Register {
        username: String,
    },
    Import {
        username: String,
        #[arg(short, long = "type", required = true)]
        ty: String,
        filepath: String,
    },
    // Export { username: String, path: String },
    Unlock {
        username: String,
    },
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
    Clear,
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

// TODO: Error handling.
macro_rules! get_password {
    () => {{
        print!("Password: ");
        std::io::stdout().flush().ok();
        // TODO: Can we write our own STDIN reader which avoids allocation
        // altogether by disabling the buffered input (raw mode) and copies each
        // input character into a fixed length buffer. Maximum password lengths
        // could be something like 200 characters.
        pw!(rpassword::read_password().unwrap())
    }};
}

#[tokio::main]
async fn main() -> Result<(), valet::user::Error> {
    let cli = Cli::parse();
    let db = Database::new(&cli.database).await?;

    let password = get_password!();

    match &cli.command {
        ValetCommand::Validate { username } => {
            let user = User::load(&db, &username, password).await?;
            println!("{} validated", user.username());
        }
        ValetCommand::Register { username } => {
            let user = User::new(&username, password)?.register(&db).await?;
            Lot::new(DEFAULT_LOT)
                .save(&db, &user)
                .await
                .expect("failed to save lot");
            println!("{} registered", username);
        }
        ValetCommand::Import {
            username,
            ty,
            filepath,
        } => {
            let user = User::load(&db, &username, password).await?;
            if let Some(mut lot) = Lot::load(&db, DEFAULT_LOT, &user).await? {
                if ty == "apple" {
                    import_apple(&db, &mut lot, filepath).await;
                }
            } else {
                eprintln!("Missing LOT: {}", DEFAULT_LOT);
            }
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
                        .save(&db, &user)
                        .await
                        .expect("failed to save lot");
                }
                Repl::Lot(LotCommand::List) => {
                    for lot in user.lots(&db).await.expect("failed to load lots").iter() {
                        println!("{}", lot.name());
                    }
                }
                Repl::Lot(LotCommand::Delete { name }) => {
                    dbg!(&name);
                    unimplemented!();
                }
                Repl::List { path } => {
                    let path = Path::parse(&path);
                    for lot in user.lots(&db).await.expect("failed to load lots").iter() {
                        if lot.name().starts_with(&path.lot) {
                            if let Ok(Some(lot)) = Lot::load(&db, &path.lot, &user).await {
                                for record in lot.records() {
                                    let label = record.data().label();
                                    if label.starts_with(&path.label) {
                                        println!("{}", Path::new(&path.lot, label));
                                    }
                                }
                            } else {
                                println!("Failed to load lot: {}", path.lot);
                            }
                        }
                    }
                }
                Repl::Put { path, data } => {
                    let path = Path::parse(&path);
                    if let Some(mut lot) = Lot::load(&db, &path.lot, &user)
                        .await
                        .expect("failed to load lot")
                    {
                        // TODO: Delete old record if it exists.
                        // TODO: Add deleted record to new record's history.
                        Record::new(&lot, RecordData::plain(&path.label, &data))
                            .insert(&db, &mut lot)
                            .await
                            .expect("failed to insert record");
                    }
                }
                Repl::Get { path } => {
                    let path = Path::parse(&path);
                    if let Some(lot) = Lot::load(&db, &path.lot, &user)
                        .await
                        .expect("failed to load lot")
                    {
                        if let Some(record) = lot
                            .records()
                            .iter()
                            .find(|r| r.data().label() == path.label)
                        {
                            println!("{}::{}", lot.name(), record);
                        }
                    }
                }
                Repl::Clear => {
                    // NOTE: Order matters here.
                    // 2J first clears into scrollback
                    // 3J then clears scrollback
                    // H resets the cursor to the topleft
                    print!("\x1b[2J\x1b[3J\x1b[H");
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
        // NOTE: parts will always have at least 1 element.
        if parts.len() == 1 || parts.len() == 2 && parts[1] == "" {
            Path {
                lot: DEFAULT_LOT.into(),
                label: parts[0].into(),
            }
        } else {
            Path {
                lot: parts[1].into(),
                label: parts[0].into(),
            }
        }
    }
}

impl fmt::Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.label.is_empty() {
            write!(f, "{}", self.lot)
        } else {
            write!(f, "{}::{}", self.lot, self.label)
        }
    }
}

#[test]
fn test_path_parse() {
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

async fn import_apple(db: &Database, lot: &mut Lot, path: &str) {
    let file = File::open(path).expect("failed to open file");
    let mut rdr = csv::Reader::from_reader(file);

    #[derive(Debug, serde::Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct CsvRecord {
        title: String,
        #[serde(rename = "URL")]
        url: String,
        username: String,
        password: String,
        notes: Option<String>,
        #[serde(rename = "OTPAuth")]
        otp: Option<String>,
    }

    for result in rdr.deserialize::<CsvRecord>() {
        match result {
            Ok(csv_record) => {
                let mut data = HashMap::new();
                data.insert("url".into(), csv_record.url);
                data.insert("username".into(), csv_record.username);
                data.insert("password".into(), csv_record.password);
                if let Some(notes) = csv_record.notes {
                    data.insert("notes".into(), notes);
                }
                if let Some(otp) = csv_record.otp {
                    data.insert("otp".into(), otp);
                }
                match Record::new(&lot, RecordData::domain(&csv_record.title, data))
                    .insert(&db, lot)
                    .await
                {
                    Ok(uuid) => {
                        println!("Inserted {} => {}", csv_record.title, uuid.as_hyphenated())
                    }
                    Err(e) => {
                        dbg!(e);
                    }
                }
            }
            Err(e) => {
                dbg!(e);
            }
        }
    }
}
