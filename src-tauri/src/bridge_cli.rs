use std::path::{Path, PathBuf};

use crate::cli_subscription_bridge::{
    default_context_export_path, revoke_bridge_files, should_include_user_scope, sync_bridge_files,
};

pub fn run(args: &[String], data_dir: &Path) -> i32 {
    match args.first().map(String::as_str) {
        Some("sync") => cmd_sync(&args[1..], data_dir),
        Some("revoke") => cmd_revoke(&args[1..]),
        _ => {
            eprintln!("Usage: oneshim bridge <sync|revoke> [--user-scope]");
            1
        }
    }
}

fn cmd_sync(args: &[String], data_dir: &Path) -> i32 {
    let include_user_scope = parse_user_scope_flag(args).unwrap_or_else(|message| {
        eprintln!("{message}");
        std::process::exit(1);
    });
    let project_root = current_project_root(data_dir);
    let context_export_path = default_context_export_path(data_dir);
    let report = sync_bridge_files(&project_root, &context_export_path, include_user_scope);

    for path in &report.written_files {
        println!("wrote {}", path.display());
    }
    for path in &report.unchanged_files {
        println!("unchanged {}", path.display());
    }
    for err in &report.errors {
        eprintln!("error {err}");
    }

    if report.errors.is_empty() {
        0
    } else {
        1
    }
}

fn cmd_revoke(args: &[String]) -> i32 {
    let include_user_scope = parse_user_scope_flag(args).unwrap_or_else(|message| {
        eprintln!("{message}");
        std::process::exit(1);
    });
    let project_root = current_project_root(Path::new("."));
    let report = revoke_bridge_files(&project_root, include_user_scope);

    for path in &report.removed_files {
        println!("removed {}", path.display());
    }
    for path in &report.skipped_files {
        println!("skipped {}", path.display());
    }
    for err in &report.errors {
        eprintln!("error {err}");
    }

    if report.errors.is_empty() {
        0
    } else {
        1
    }
}

fn current_project_root(fallback: &Path) -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| fallback.to_path_buf())
}

fn parse_user_scope_flag(args: &[String]) -> Result<bool, String> {
    if args.is_empty() {
        return Ok(should_include_user_scope());
    }

    if args.len() == 1 && args[0] == "--user-scope" {
        return Ok(true);
    }

    Err("Usage: oneshim bridge <sync|revoke> [--user-scope]".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_user_scope_flag_accepts_default_and_flag() {
        assert!(parse_user_scope_flag(&[]).is_ok());
        assert_eq!(
            parse_user_scope_flag(&["--user-scope".to_string()]),
            Ok(true)
        );
    }

    #[test]
    fn parse_user_scope_flag_rejects_unknown_args() {
        assert!(parse_user_scope_flag(&["--other".to_string()]).is_err());
    }
}
