use crate::finding::{Finding, Severity};

pub(crate) fn print_summary(findings: &[Finding], strict_i18n: bool) {
    if findings.is_empty() {
        println!("language-check: no findings");
        return;
    }

    let mut error_count = 0usize;
    let mut warning_count = 0usize;
    for finding in findings {
        match finding.severity {
            Severity::Error => error_count += 1,
            Severity::Warning => warning_count += 1,
        }

        let sev = match finding.severity {
            Severity::Error => "ERROR",
            Severity::Warning => "WARN ",
        };

        println!(
            "[{}] {}:{}:{} [{}] {}",
            sev,
            finding.path.display(),
            finding.line,
            finding.column,
            finding.category,
            finding.message
        );
        if !finding.snippet.is_empty() {
            println!("  -> {}", finding.snippet);
        }
    }

    println!();
    println!(
        "language-check summary: {} error(s), {} warning(s), strict_i18n={}",
        error_count, warning_count, strict_i18n
    );
}
