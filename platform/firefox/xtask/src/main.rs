use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let flags = if args.len() > 1 { &args[1..] } else { &[] };
    match args.first().map(|s| s.as_str()) {
        Some("build-install") => build_install(flags),
        Some("build-wasm") => build_wasm(flags),
        Some("install-native-host") => install_native_host(flags),
        Some(other) => {
            eprintln!("unknown xtask: {other}");
            usage();
            ExitCode::FAILURE
        }
        None => {
            usage();
            ExitCode::FAILURE
        }
    }
}

fn usage() {
    eprintln!("Usage: cargo firefox-xtask <COMMAND>");
    eprintln!();
    eprintln!("Commands:");
    eprintln!("  build-install [--dev]        Build WASM and install the native host");
    eprintln!("  build-wasm [--dev]           Build the WASM package with wasm-pack");
    eprintln!(
        "  install-native-host [--dev]  Build and register the Firefox native messaging host"
    );
}

fn is_dev(flags: &[String]) -> bool {
    flags.iter().any(|a| a == "--dev")
}

fn repo_root() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .expect("xtask lives at platform/firefox/xtask")
        .to_path_buf()
}

/// Build the WASM package and install the native host.
fn build_install(flags: &[String]) -> ExitCode {
    let result = build_wasm(flags);
    if result != ExitCode::SUCCESS {
        return result;
    }
    install_native_host(flags)
}

fn build_wasm(flags: &[String]) -> ExitCode {
    let root = repo_root();
    let dev = is_dev(flags);

    let mut cmd = Command::new("wasm-pack");
    cmd.arg("build").arg("--target").arg("web");
    if dev {
        cmd.arg("--dev");
        eprintln!("Building WASM package (dev)...");
    } else {
        eprintln!("Building WASM package (release)...");
    }
    cmd.arg("platform/firefox");
    cmd.current_dir(&root);

    match cmd.status() {
        Ok(s) if s.success() => {
            eprintln!("WASM package built: platform/firefox/pkg/");
            ExitCode::SUCCESS
        }
        Ok(s) => {
            eprintln!("wasm-pack failed (exit {})", s.code().unwrap_or(-1));
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("failed to run wasm-pack: {e}");
            eprintln!("Install it with: cargo install wasm-pack");
            ExitCode::FAILURE
        }
    }
}

fn install_native_host(flags: &[String]) -> ExitCode {
    let root = repo_root();
    let dev = is_dev(flags);
    let profile = if dev { "dev" } else { "release" };

    let dest_dir = match native_messaging_dir() {
        Some(d) => d,
        None => {
            eprintln!("Unsupported OS: {}", env::consts::OS);
            return ExitCode::FAILURE;
        }
    };

    eprintln!("Building valet-native-host ({profile})...");
    let mut cargo_args = vec!["build", "-p", "valet-native-host"];
    if !dev {
        cargo_args.push("--release");
    }
    let status = Command::new("cargo")
        .args(&cargo_args)
        .current_dir(&root)
        .status();

    match status {
        Ok(s) if s.success() => {}
        Ok(s) => {
            eprintln!("cargo build failed (exit {})", s.code().unwrap_or(-1));
            return ExitCode::FAILURE;
        }
        Err(e) => {
            eprintln!("failed to run cargo: {e}");
            return ExitCode::FAILURE;
        }
    }

    let target_dir = if dev { "debug" } else { "release" };
    let bin_path = root.join(format!("target/{target_dir}/valet-native-host"));
    if !bin_path.exists() {
        eprintln!("Build did not produce {}", bin_path.display());
        return ExitCode::FAILURE;
    }

    if let Err(e) = fs::create_dir_all(&dest_dir) {
        eprintln!("failed to create {}: {e}", dest_dir.display());
        return ExitCode::FAILURE;
    }

    let template_path = root.join("platform/firefox/native-host/com.nixpulvis.valet.json.template");
    let template = match fs::read_to_string(&template_path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("failed to read {}: {e}", template_path.display());
            return ExitCode::FAILURE;
        }
    };

    let manifest = template.replace("@VALET_NATIVE_HOST_PATH@", &bin_path.to_string_lossy());
    let dest = dest_dir.join("com.nixpulvis.valet.json");

    if let Err(e) = fs::write(&dest, &manifest) {
        eprintln!("failed to write {}: {e}", dest.display());
        return ExitCode::FAILURE;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&dest, fs::Permissions::from_mode(0o600));
    }

    eprintln!("Installed native messaging manifest:");
    eprintln!("  {}", dest.display());
    eprintln!("Pointing at:");
    eprintln!("  {}", bin_path.display());
    eprintln!();
    eprintln!("If you want to use a non-default DB path, set VALET_DB in your shell env");
    eprintln!("before launching Firefox (the native host inherits Firefox's environment).");

    ExitCode::SUCCESS
}

fn native_messaging_dir() -> Option<PathBuf> {
    let home = PathBuf::from(env::var("HOME").ok()?);
    match env::consts::OS {
        "macos" => Some(home.join("Library/Application Support/Mozilla/NativeMessagingHosts")),
        "linux" => Some(home.join(".mozilla/native-messaging-hosts")),
        _ => None,
    }
}
