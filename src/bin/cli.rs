use clap::{CommandFactory, Parser, Subcommand, crate_description};
use clap_complete::{Shell, generate};
use clap_repl::ClapEditor;
use clap_repl::reedline::{DefaultPrompt, DefaultPromptSegment, FileBackedHistory};
use regex::Regex;
use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::io::Write;
use std::str::FromStr;
use tokio;
use valet::{
    prelude::*,
    record::{Query, RecordIndex},
    user,
};

#[derive(Parser)]
#[command(version, about = crate_description!())]
struct Cli {
    #[arg(short, long, default_value_t = valet::db::default_url())]
    database: String,

    #[command(subcommand)]
    command: ValetCommand,
}

#[derive(Subcommand)]
enum ValetCommand {
    Unlock {
        #[arg(short, long = "user")]
        username: Option<String>,
    },
    Import {
        #[arg(short, long = "user")]
        username: Option<String>,
        #[arg(short, long = "type", required = true)]
        ty: String,
        filepath: String,
    },
    // Export {
    //     #[arg(short, long = "user")]
    //     username: Option<String>,
    //     #[arg(short, long = "type", required = true)]
    //     ty: String,
    // },
    #[command(subcommand)]
    User(UserCommand),

    #[command(subcommand)]
    Config(ConfigCommand),
}

#[derive(Subcommand)]
enum UserCommand {
    Register {
        username: String,
    },
    Validate {
        #[arg(short, long = "user")]
        username: Option<String>,
    },
    List,
}

#[derive(Subcommand)]
enum ConfigCommand {
    GenerateCompletions { shell: Shell },
}

#[derive(Parser)]
enum Repl {
    #[command(subcommand)]
    Lot(LotCommand),
    /// List records matching a query path.
    ///
    /// With no path, lists every record in the default lot (`main`) only.
    /// To search across all lots, pass `~::` (regex-match-all lot spec).
    List {
        #[clap(default_value = "")]
        path: String,
        #[arg(long = "uuid")]
        uuid: bool,
    },
    Put {
        path: String,
        data: String,
    },
    Get {
        path: String,
        #[arg(long = "uuid")]
        uuid: bool,
    },
    Clear,
    Lock,
}

#[derive(Subcommand)]
enum LotCommand {
    Create {
        name: String,
    },
    List {
        #[arg(long = "uuid")]
        uuid: bool,
    },
    // Share { name: String, users: Vec<String> },
    // Unshare { name: String, users: Vec<String> },
    Delete {
        name: String,
    },
}

fn get_password() -> Result<Password, valet::user::Error> {
    print!("Password: ");
    io::stdout().flush().ok();
    // TODO: Can we write our own STDIN reader which avoids extra allocation
    // altogether by disabling the buffered input (raw mode) and copies each
    // input character into a fixed length buffer. Maximum password lengths
    // could be something like 200 characters.
    let mut password_string = rpassword::read_password().unwrap();
    let password: Password = if let Ok(password) = password_string.as_str().try_into() {
        password
    } else {
        return Err(valet::user::Error::Invalid);
    };
    zeroize::Zeroize::zeroize(&mut password_string);
    Ok(password)
}

#[tokio::main]
async fn main() -> Result<(), valet::user::Error> {
    let cli = Cli::parse();

    match &cli.command {
        ValetCommand::User(UserCommand::Register { username }) => {
            let db = Database::new(&cli.database).await?;
            let password = get_password()?;
            let user = User::new(&username, password)?.register(&db).await?;
            Lot::new(DEFAULT_LOT)
                .save(&db, &user)
                .await
                .expect("failed to save lot");
            println!("{} registered", username);
        }
        ValetCommand::User(UserCommand::Validate { username }) => {
            let db = Database::new(&cli.database).await?;
            let username = get_default_username(username, &db).await?;
            let password = get_password()?;
            let user = User::load(&db, &username, password).await?;
            println!("{} validated", user.username());
        }
        ValetCommand::User(UserCommand::List) => {
            let db = Database::new(&cli.database).await?;
            for user in User::list(&db).await? {
                println!("{user}")
            }
        }
        ValetCommand::Unlock { username } => {
            let db = Database::new(&cli.database).await?;

            let username = get_default_username(username, &db).await?;
            let password = get_password()?;
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

            let mut store: Vec<(Lot, RecordIndex)> = load_store(&db, &user).await;

            rl.repl_async(async |command| match &command {
                Repl::Lot(LotCommand::Create { name }) => {
                    match Lot::new(&name).save(&db, &user).await {
                        Ok(_) => {
                            store = load_store(&db, &user).await;
                        }
                        Err(e) => {
                            println!("Failed to save lot: {e:?}");
                        }
                    }
                }
                Repl::Lot(LotCommand::List { uuid }) => {
                    for (lot, _) in store.iter() {
                        if *uuid {
                            println!("{} <{}>", lot.name(), lot.uuid());
                        } else {
                            println!("{}", lot.name());
                        }
                    }
                }
                Repl::Lot(LotCommand::Delete { name }) => {
                    dbg!(&name);
                    unimplemented!();
                }
                Repl::List { path, uuid } => {
                    let query = match Query::from_str(path) {
                        Ok(q) => q,
                        Err(e) => {
                            println!("{e}: {path}");
                            return;
                        }
                    };
                    let matching_lots: Vec<_> = store
                        .iter()
                        .filter(|(lot, _)| query.matches_lot(lot.name()))
                        .collect();
                    if matching_lots.is_empty() {
                        println!("No lots match: {path}");
                        return;
                    }
                    let mut printed = false;
                    for (lot, index) in &matching_lots {
                        for (entry_label, record_uuid) in index.search(&query) {
                            if *uuid {
                                println!("{}::{entry_label} <{record_uuid}>", lot.name());
                            } else {
                                println!("{}::{entry_label}", lot.name());
                            }
                            printed = true;
                        }
                    }
                    if !printed {
                        println!("No records match: {path}");
                    }
                }
                Repl::Put { path, data } => {
                    let target = match Query::from_str(&path).and_then(Query::into_path) {
                        Ok(p) => p,
                        Err(e) => {
                            println!("{e}: {path}");
                            return;
                        }
                    };
                    let Ok(password) = data.as_str().try_into() else {
                        println!("Invalid password");
                        return;
                    };
                    let Some((lot, _)) = store.iter().find(|(l, _)| l.name() == target.lot) else {
                        println!("Unknown lot: {}", target.lot);
                        return;
                    };
                    // TODO: Delete old record if it exists.
                    // TODO: Add deleted record to new record's history.
                    // TODO: Put data in a Password itself.
                    match Record::new(lot, target.label, Data::new(password))
                        .upsert(&db, lot)
                        .await
                    {
                        Ok(_) => {
                            store = load_store(&db, &user).await;
                        }
                        Err(e) => {
                            println!("Failed to save record: {e:?}");
                        }
                    }
                }
                Repl::Get { path, uuid } => {
                    let query = match Query::from_str(&path) {
                        Ok(q) => q,
                        Err(e) => {
                            println!("{e}: {path}");
                            return;
                        }
                    };
                    let matching_lots: Vec<_> = store
                        .iter()
                        .filter(|(lot, _)| query.matches_lot(lot.name()))
                        .collect();
                    if matching_lots.is_empty() {
                        println!("No lots match: {path}");
                        return;
                    }
                    let matches: Vec<_> = matching_lots
                        .iter()
                        .flat_map(|(lot, index)| {
                            index
                                .search(&query)
                                .map(move |(label, uuid)| (lot, label, uuid))
                        })
                        .collect();
                    let (picked_lot, record_uuid) = match matches.as_slice() {
                        [] => {
                            println!("No records match: {path}");
                            return;
                        }
                        [(lot, _, uuid)] => (*lot, *uuid),
                        many => {
                            for (i, (lot, label, _)) in many.iter().enumerate() {
                                println!("{i}: {}::{label}", lot.name());
                            }
                            print!("Pick: ");
                            io::stdout().flush().ok();
                            let mut buf = String::new();
                            if io::stdin().read_line(&mut buf).is_err() {
                                return;
                            }
                            let Ok(idx) = buf.trim().parse::<usize>() else {
                                println!("Not a number");
                                return;
                            };
                            let Some((lot, _, uuid)) = many.get(idx) else {
                                println!("Out of range");
                                return;
                            };
                            (*lot, *uuid)
                        }
                    };
                    // TODO: The in-memory index can be stale if another
                    // writer deleted the record. Need a general
                    // resync/retry strategy for multi-writer setups.
                    match Record::show(&db, picked_lot, record_uuid).await {
                        Ok(Some(record)) => {
                            if *uuid {
                                println!("{} <{}>", record.password(), record.uuid());
                            } else {
                                println!("{}", record.password());
                            }
                        }
                        Ok(None) => {
                            println!("Record not found: {path} <{record_uuid}>");
                        }
                        Err(e) => {
                            println!("Failed to load record: {e:?}");
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
        ValetCommand::Import {
            username,
            ty,
            filepath,
        } => {
            let db = Database::new(&cli.database).await?;
            let username = get_default_username(username, &db).await?;
            let password = get_password()?;
            let user = User::load(&db, &username, password).await?;
            if let Some(mut lot) = Lot::load(&db, DEFAULT_LOT, &user).await? {
                if ty == "apple" {
                    import_apple(&db, &mut lot, filepath).await;
                }
            } else {
                eprintln!("Missing LOT: {}", DEFAULT_LOT);
            }
        }
        ValetCommand::Config(ConfigCommand::GenerateCompletions { shell }) => {
            let mut cmd = Cli::command();
            let name = cmd.get_name().to_owned();
            generate(*shell, &mut cmd, name, &mut io::stdout());
        }
    }

    Ok(())
}

/// Load every lot for `user`, paired with its record index, so the REPL can
/// serve `list`/`get` without re-decrypting the user's lot keys on each call.
async fn load_store(db: &Database, user: &User) -> Vec<(Lot, RecordIndex)> {
    let lots = user.lots(db).await.expect("failed to load lots");
    let mut store = Vec::with_capacity(lots.len());
    for lot in lots {
        let index = lot.index(db).await.expect("failed to load index");
        store.push((lot, index));
    }
    store
}

async fn get_default_username(
    provided: &Option<String>,
    db: &Database,
) -> Result<String, user::Error> {
    match provided {
        Some(username) => Ok(username.to_owned()),
        // TODO: We need proper CLI error types here.
        None => {
            // TODO: Add a configurable default user.
            let usernames = User::list(&db).await?;
            if usernames.len() == 1 {
                Ok(usernames[0].to_owned())
            } else {
                Err(user::Error::NotFound)
            }
        }
    }
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
                let re = Regex::new(r"(\S+)\s*(?:\((.*)\))?").unwrap();
                let label;
                if let Some(captures) = re.captures(&csv_record.title) {
                    let domain_or_label = captures[1].to_owned();
                    if let Some(user) = captures.get(2) {
                        label = format!("{}@{domain_or_label}", user.as_str());
                    } else {
                        label = domain_or_label;
                    }
                } else {
                    eprintln!("Bad title: {}", csv_record.title);
                    continue;
                }

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
                let parsed_label = match label.parse::<Label>() {
                    Ok(l) => l,
                    Err(e) => {
                        eprintln!("Invalid label {label:?}: {e}");
                        continue;
                    }
                };
                // TODO: Put text directly into a Password
                if let Ok(password) = data
                    .remove("password")
                    .unwrap_or_default()
                    .as_str()
                    .try_into()
                {
                    match Record::new(&lot, parsed_label, Data::new(password).with_extra(data))
                        .upsert(&db, lot)
                        .await
                    {
                        Ok(uuid) => {
                            println!(
                                "Inserted {}::{} <{}>",
                                lot.name(),
                                label,
                                uuid.to_uuid().as_hyphenated()
                            )
                        }
                        Err(e) => {
                            dbg!(e);
                        }
                    }
                }
            }
            Err(e) => {
                dbg!(e);
            }
        }
    }
}
