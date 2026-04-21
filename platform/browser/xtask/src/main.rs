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
    eprintln!("Usage: cargo browser-xtask <COMMAND>");
    eprintln!();
    eprintln!("Commands:");
    eprintln!("  build-install [--release]        Build WASM and install the native host");
    eprintln!("  build-wasm [--release]           Build the WASM package with wasm-pack");
    eprintln!(
        "  install-native-host [--release]  Build and register the browser's native messaging host"
    );
}

fn is_release(flags: &[String]) -> bool {
    flags.iter().any(|a| a == "--release")
}

fn repo_root() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .expect("xtask lives at platform/browser/xtask")
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
    let release = is_release(flags);

    let mut cmd = Command::new("wasm-pack");
    cmd.arg("build").arg("--target").arg("web");
    if release {
        eprintln!("Building WASM package (release)...");
    } else {
        cmd.arg("--dev");
        eprintln!("Building WASM package (dev)...");
    }
    cmd.arg("platform/browser");
    cmd.current_dir(&root);

    match cmd.status() {
        Ok(s) if s.success() => {
            eprintln!("WASM package built: platform/browser/pkg/");
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
    let release = is_release(flags);
    let profile = if release { "release" } else { "dev" };

    let dest_dir = match native_messaging_dir() {
        Some(d) => d,
        None => {
            eprintln!("Unsupported OS: {}", env::consts::OS);
            return ExitCode::FAILURE;
        }
    };

    eprintln!("Building valet-native-host and valetd ({profile})...");
    let mut cargo_args = vec!["build", "-p", "valet-native-host", "-p", "valetd"];
    if release {
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

    let target_dir = if release { "release" } else { "debug" };
    let bin_path = root.join(format!("target/{target_dir}/valet-native-host"));
    let daemon_path = root.join(format!("target/{target_dir}/valetd"));
    for p in [&bin_path, &daemon_path] {
        if !p.exists() {
            eprintln!("Build did not produce {}", p.display());
            return ExitCode::FAILURE;
        }
    }

    if let Err(e) = fs::create_dir_all(&dest_dir) {
        eprintln!("failed to create {}: {e}", dest_dir.display());
        return ExitCode::FAILURE;
    }

    let template_path = root.join("platform/browser/native-host/com.nixpulvis.valet.json.template");
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
    eprintln!("The shim auto-spawns a sibling valetd binary from:");
    eprintln!("  {}", daemon_path.display());
    eprintln!();
    eprintln!("If you want to use a non-default DB or socket path, set VALET_DB or");
    eprintln!("VALET_SOCKET in your shell env before launching the browser (the");
    eprintln!("native host inherits the browser's environment).");

    // The shim reuses an existing daemon if the socket is live, so a stale
    // `valetd` from before this rebuild would otherwise keep serving the
    // old wire schema. Kill it so the next shim connection spawns the
    // freshly-built binary. Ignore errors (none running is fine).
    let killed = Command::new("pkill")
        .args(["-x", "valetd"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if killed {
        eprintln!();
        eprintln!("Stopped running valetd; the shim will spawn the new build on demand.");
    }

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
