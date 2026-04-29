//! CLI credential management — runs before Tauri boot using KeychainOps sync core.

use oneshim_storage::keychain::KeychainOps;
use std::path::Path;

const KNOWN_PROVIDERS: &[&str] = &["openai"];

pub fn run(args: &[String], config_dir: &Path) -> i32 {
    let registry_path = config_dir.join("oneshim-keychain-registry.json");
    let ops = match KeychainOps::new(registry_path) {
        Ok(ops) => ops,
        Err(e) => {
            eprintln!("Error: failed to initialize keychain: {e}");
            return 1;
        }
    };

    match args.first().map(String::as_str) {
        Some("status") => cmd_status(&ops),
        Some("revoke") => cmd_revoke(&ops, &args[1..]),
        _ => {
            eprintln!("Usage: oneshim auth <status|revoke>");
            eprintln!();
            eprintln!("Commands:");
            eprintln!("  status              Show connected OAuth providers");
            eprintln!("  revoke <provider>   Remove credentials for a provider");
            eprintln!("  revoke --all        Remove all stored credentials");
            1
        }
    }
}

fn cmd_status(ops: &KeychainOps) -> i32 {
    println!("{:<12} {:<16} Expires", "Provider", "Status");
    for provider in KNOWN_PROVIDERS {
        let status = ops.probe_namespace(provider);
        let state = if status.connected {
            "connected"
        } else {
            "not connected"
        };
        let expires = status.expires_at.as_deref().unwrap_or("-");
        println!("{:<12} {:<16} {}", provider, state, expires);
    }
    0
}

fn cmd_revoke(ops: &KeychainOps, args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("--all") => {
            let mut had_error = false;
            for provider in KNOWN_PROVIDERS {
                match ops.delete_namespace_sync(provider) {
                    Ok(()) => println!("Removed credentials for '{provider}'"),
                    Err(e) => {
                        had_error = true;
                        eprintln!("Warning: failed to revoke '{provider}': {e}");
                    }
                }
            }
            if had_error {
                1
            } else {
                0
            }
        }
        Some(provider) => {
            let target = provider.to_owned();
            if !KNOWN_PROVIDERS.contains(&target.as_str()) {
                eprintln!("Unknown provider: '{target}'");
                eprintln!("Known providers: {}", KNOWN_PROVIDERS.join(", "));
                return 1;
            }
            match ops.delete_namespace_sync(&target) {
                Ok(()) => {
                    println!("Removed credentials for '{target}' from keychain.");
                    0
                }
                Err(e) => {
                    eprintln!("Error: {e}");
                    1
                }
            }
        }
        None => {
            eprintln!("Usage: oneshim auth revoke <provider|--all>");
            1
        }
    }
}
