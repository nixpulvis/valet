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
use std::sync::Arc;
use valet::db::Database;
use valet::lot::DEFAULT_LOT;
use valet::password::Password;
use valet::protocol::EmbeddedHandler;
use valet::protocol::message::{
    CreateLot, CreateRecord, DeleteLot, Fetch, History, List, ListLots, ListUsers, Register,
    Unlock, Validate,
};
use valet::record::{Data, Label, LabelName, Query, Record, SaveProgress};
use valet::{Lot, SendHandler};

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
        /// Print every historical revision of the matched record (newest
        /// first) instead of only the current password.
        #[arg(short = 'H', long = "history")]
        history: bool,
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
async fn main() -> Result<(), CliError> {
    let cli = Cli::parse();

    match &cli.command {
        ValetCommand::User(UserCommand::Register { username }) => {
            let client = open_client(&cli.database).await?;
            let password = get_password()?;
            client
                .call(Register {
                    username: username.clone(),
                    password,
                })
                .await?;
            println!("{} registered", username);
        }
        ValetCommand::User(UserCommand::Validate { username }) => {
            let client = open_client(&cli.database).await?;
            let username = get_default_username(username, &client).await?;
            let password = get_password()?;
            client
                .call(Validate {
                    username: username.clone(),
                    password,
                })
                .await?;
            println!("{} validated", username);
        }
        ValetCommand::User(UserCommand::List) => {
            let client = open_client(&cli.database).await?;
            for user in client.call(ListUsers).await? {
                println!("{user}")
            }
        }
        ValetCommand::Unlock { username } => {
            let client = open_client(&cli.database).await?;
            let username = get_default_username(username, &client).await?;
            let password = get_password()?;
            client
                .call(Unlock {
                    username: username.clone(),
                    password,
                })
                .await?;

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

            run_repl(rl, client, username).await;
        }
        ValetCommand::Import {
            username,
            ty,
            filepath,
        } => {
            // Bulk import streams a progress callback through
            // `Record::save_many`, which doesn't fit the one-shot
            // request/response shape. Keep it on the raw DB path for
            // now; `Client<Embedded>::state` is not reachable, so run a
            // parallel Database handle just for this operation.
            // TODO: fold import into the handler once we have a
            // streaming response protocol.
            let db = Database::new(&cli.database).await?;
            let client = open_client(&cli.database).await?;
            let username = get_default_username(username, &client).await?;
            let password = get_password()?;
            let user = valet::User::load(&db, &username, password).await?;
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

async fn open_client(database: &str) -> Result<Arc<EmbeddedHandler>, CliError> {
    let db = Database::new(database).await?;
    Ok(Arc::new(EmbeddedHandler::new(
        db,
        &tokio::runtime::Handle::current(),
    )))
}

async fn run_repl(rl: ClapEditor<Repl>, client: Arc<EmbeddedHandler>, username: String) {
    rl.repl_async(async |command| match &command {
        Repl::Lot(LotCommand::Create { name }) => {
            if let Err(e) = client
                .call(CreateLot {
                    username: username.clone(),
                    lot: name.clone(),
                })
                .await
            {
                println!("Failed to create lot: {e}");
            }
        }
        Repl::Lot(LotCommand::List { uuid }) => match client
            .call(ListLots {
                username: username.clone(),
            })
            .await
        {
            Ok(lots) => {
                for (lot_uuid, name) in lots {
                    if *uuid {
                        println!("{name} <{lot_uuid}>");
                    } else {
                        println!("{name}");
                    }
                }
            }
            Err(e) => println!("Failed to list lots: {e}"),
        },
        Repl::Lot(LotCommand::Delete { name }) => {
            if let Err(e) = client
                .call(DeleteLot {
                    username: username.clone(),
                    lot: name.clone(),
                })
                .await
            {
                println!("Failed to delete lot: {e}");
            }
        }
        Repl::List { path, uuid } => {
            let entries = match client
                .call(List {
                    username: username.clone(),
                    queries: vec![path.clone()],
                })
                .await
            {
                Ok(es) => es,
                Err(e) => {
                    println!("{e}");
                    return;
                }
            };
            if entries.is_empty() {
                println!("No records match: {path}");
                return;
            }
            for (record_uuid, label) in entries {
                let name = label.name();
                if *uuid {
                    println!("{name} <{record_uuid}>");
                } else {
                    println!("{name}");
                }
            }
        }
        Repl::Put { path, data } => {
            let target = match Query::from_str(path).and_then(Query::into_path) {
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
            // `create_record` upserts by label name, so a repeated put
            // extends the existing record's storgit history.
            if let Err(e) = client
                .call(CreateRecord {
                    username: username.clone(),
                    lot: target.lot,
                    label: target.label,
                    password,
                    extra: HashMap::new(),
                })
                .await
            {
                println!("Failed to save record: {e}");
            }
        }
        Repl::Get {
            path,
            uuid,
            history,
        } => {
            let query = match Query::from_str(path) {
                Ok(q) => q,
                Err(e) => {
                    println!("{e}: {path}");
                    return;
                }
            };
            // The handler's `list` applies the same query grammar the
            // CLI has always used, so we get a flat (uuid, label) set
            // to disambiguate against. Lot-name lookup per entry is
            // not exposed today; prompt by label and then fetch via
            // `fetch` (cross-lot by uuid).
            let entries = match client
                .call(List {
                    username: username.clone(),
                    queries: vec![path.clone()],
                })
                .await
            {
                Ok(es) => es,
                Err(e) => {
                    println!("{e}");
                    return;
                }
            };
            if entries.is_empty() {
                println!("No records match: {path}");
                return;
            }
            let (record_uuid, _label) = match entries.as_slice() {
                [one] => (one.0.clone(), one.1.clone()),
                many => {
                    for (i, (_, label)) in many.iter().enumerate() {
                        println!("{i}: {label}");
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
                    let Some(pick) = many.get(idx) else {
                        println!("Out of range");
                        return;
                    };
                    (pick.0.clone(), pick.1.clone())
                }
            };
            if *history {
                // History needs a lot name; since `list` doesn't tell us
                // which lot matched, fall back to the query's lot spec
                // (required when using `-H`).
                let lot = match query.into_path() {
                    Ok(p) => p.lot,
                    Err(_) => {
                        println!("`get -H` needs a full `lot::label` path");
                        return;
                    }
                };
                match client
                    .call(History {
                        username: username.clone(),
                        lot,
                        uuid: record_uuid.clone(),
                    })
                    .await
                {
                    Ok(revs) if revs.is_empty() => {
                        println!("No revisions: {path} <{record_uuid}>");
                    }
                    Ok(revs) => {
                        for rev in revs {
                            let secs = rev.time_millis / 1000;
                            let nanos = ((rev.time_millis.rem_euclid(1000)) as u32) * 1_000_000;
                            let ts = chrono::DateTime::<chrono::Utc>::from_timestamp(secs, nanos)
                                .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
                                .unwrap_or_else(|| rev.time_millis.to_string());
                            println!("{ts} {}: {}", rev.label, rev.password);
                        }
                    }
                    Err(e) => {
                        println!("Failed to load history: {e}");
                    }
                }
            } else {
                match client
                    .call(Fetch {
                        username: username.clone(),
                        uuid: record_uuid.clone(),
                    })
                    .await
                {
                    Ok(record) => {
                        if *uuid {
                            println!("{} <{}>", record.password(), record.uuid());
                        } else {
                            println!("{}", record.password());
                        }
                        for (k, v) in record.label().extra() {
                            println!("{k}: {v}");
                        }
                    }
                    Err(e) => {
                        println!("Failed to load record: {e}");
                    }
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

async fn get_default_username(
    provided: &Option<String>,
    client: &Arc<EmbeddedHandler>,
) -> Result<String, CliError> {
    match provided {
        Some(username) => Ok(username.to_owned()),
        // TODO: We need proper CLI error types here.
        None => {
            // TODO: Add a configurable default user.
            let usernames = client.call(ListUsers).await?;
            if usernames.len() == 1 {
                Ok(usernames[0].to_owned())
            } else {
                Err(CliError::User(valet::user::Error::NotFound))
            }
        }
    }
}

#[derive(Debug)]
enum CliError {
    User(valet::user::Error),
    Db(valet::db::Error),
    Protocol(valet::protocol::Error),
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CliError::User(e) => write!(f, "{e:?}"),
            CliError::Db(e) => write!(f, "{e:?}"),
            CliError::Protocol(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for CliError {}

impl From<valet::user::Error> for CliError {
    fn from(e: valet::user::Error) -> Self {
        CliError::User(e)
    }
}

impl From<valet::db::Error> for CliError {
    fn from(e: valet::db::Error) -> Self {
        CliError::Db(e)
    }
}

impl From<valet::protocol::Error> for CliError {
    fn from(e: valet::protocol::Error) -> Self {
        CliError::Protocol(e)
    }
}

impl From<valet::lot::Error> for CliError {
    fn from(e: valet::lot::Error) -> Self {
        CliError::User(valet::user::Error::Lot(e))
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

    let title_re = Regex::new(r"(\S+)\s*(?:\((.*)\))?").unwrap();
    let mut records = Vec::new();
    for result in rdr.deserialize::<CsvRecord>() {
        let csv_record = match result {
            Ok(r) => r,
            Err(e) => {
                dbg!(e);
                continue;
            }
        };
        let label = if let Some(captures) = title_re.captures(&csv_record.title) {
            let domain_or_label = captures[1].to_owned();
            if let Some(user) = captures.get(2) {
                format!("{}@{domain_or_label}", user.as_str())
            } else {
                domain_or_label
            }
        } else {
            eprintln!("Bad title: {}", csv_record.title);
            continue;
        };

        let mut data = HashMap::new();
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
        let parsed_label = match parsed_label
            .add_extra("url", csv_record.url)
            .and_then(|l| match l.name() {
                LabelName::Domain { id, .. } if id == &csv_record.username => Ok(l),
                _ => l.add_extra("username", csv_record.username),
            }) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("{e} for {label:?}");
                // TODO: Instead of this .and_then chain. Just try to add labels
                // if it works, great, otherwise print warning but still add it.
                // Maybe .add_extra should take an &mut self.
                continue;
            }
        };
        // TODO: Put text directly into a Password
        let Ok(password) = csv_record.password.as_str().try_into() else {
            continue;
        };
        // TODO: We need to load the lot and check for existing records before
        // minting new UUIDs. In general I don't think we actually want a
        // Record::new function at all, since we should always have a lot
        // before creating records, so something like Record::get which either
        // lazy loads or mints a fresh record is in order. We will also need a
        // way to batch this or something here, so it's a little more complex...
        records.push(Record::new(
            &*lot,
            parsed_label,
            Data::new(password).with_extra(data),
        ));
    }

    let total = records.len();
    let lot_name = lot.name().to_owned();
    println!("Importing {total} records into {lot_name}...");
    let mut put = 0usize;
    let result = Record::save_many(db, lot, &records, |ev| match ev {
        SaveProgress::OpenedStore => {
            println!("Opened store");
        }
        SaveProgress::PutRecord(record) => {
            put += 1;
            println!("Put {put}/{total} {lot_name}::{}", record.label());
        }
        SaveProgress::Bundle(_) => {
            println!("Bundle complete");
        }
        SaveProgress::SaveRecord => {
            println!("Saved {total} records");
        }
        SaveProgress::SaveLot => {
            println!("Saved lot {lot_name}");
        }
    })
    .await;
    if let Err(e) = result {
        dbg!(e);
    }
}
