use crate::finding::Mode;

pub(crate) fn parse_mode(args: &[String]) -> Mode {
    if let Some(first) = args.first() {
        match first.as_str() {
            "non-english" => Mode::NonEnglish,
            "i18n" => Mode::I18n,
            _ => Mode::All,
        }
    } else {
        Mode::All
    }
}

pub(crate) fn print_help() {
    println!("language-check - language and i18n quality checker");
    println!();
    println!("Usage:");
    println!("  cargo run -p maekon-lint --bin language-check -- [non-english|i18n|all] [--strict-i18n] [--path <dir>] [--ignore <substring>]");
    println!();
    println!("Modes:");
    println!(
        "  non-english   Scan code files for non-ASCII characters (excluding locale JSON files)."
    );
    println!("  i18n          Validate frontend locale key coverage and i18n usage heuristics.");
    println!("  all           Run both checks (default).");
    println!();
    println!("Options:");
    println!("  --strict-i18n Treat i18n warnings (hardcoded UI strings) as build failures.");
    println!("  --path <dir>  Limit scan to a specific subdirectory (repeatable).");
    println!("  --ignore <s>  Ignore files whose path contains substring <s> (repeatable).");
}

pub(crate) fn collect_option_values(args: &[String], option: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut i = 0usize;
    while i < args.len() {
        if args[i] == option {
            if let Some(value) = args.get(i + 1) {
                values.push(value.clone());
                i += 2;
                continue;
            }
        }
        i += 1;
    }
    values
}
