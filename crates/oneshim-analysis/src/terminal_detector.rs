//! Terminal command detection from accessibility-extracted text.
//!
//! When AppSubcategory is Terminal and extracted_text is available
//! (Basic/Off PII level), detects terminal prompt patterns and extracts
//! the current command line. Simple pattern matching, not shell parsing.

/// Result of terminal command detection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalCommandInfo {
    /// The detected command (first word after the prompt).
    /// e.g., "cargo", "git", "docker", "npm"
    pub command: String,
    /// Full command line after prompt (truncated to 120 chars).
    pub command_line: String,
    /// The prompt pattern that was matched.
    pub prompt_char: char,
}

/// Terminal prompt characters to detect.
const PROMPT_CHARS: &[char] = &['$', '%', '#', '>'];

/// Maximum command line length to capture (privacy bound).
const MAX_COMMAND_LINE_LEN: usize = 120;

/// Detect a terminal command from accessibility-extracted text.
///
/// Looks for prompt patterns (`$`, `%`, `#`, `>`) at the start of
/// lines or after whitespace, then extracts the text following the
/// prompt as the command line.
///
/// Returns `None` if no prompt pattern is detected or if the text
/// after the prompt is empty.
pub fn detect_terminal_command(text: &str) -> Option<TerminalCommandInfo> {
    // Process lines in reverse order to find the most recent command
    // (terminal output accumulates upward).
    for line in text.lines().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        for &prompt in PROMPT_CHARS {
            // Match patterns: "$ cmd", "% cmd", "# cmd", "> cmd"
            // Also match: "user@host:~$ cmd", "PS1> cmd"
            if let Some(pos) = trimmed.rfind(prompt) {
                let after = trimmed[pos + prompt.len_utf8()..].trim_start();
                if after.is_empty() {
                    continue;
                }

                // Extract the command (first whitespace-delimited token)
                let command = after.split_whitespace().next().unwrap_or("").to_string();

                if command.is_empty() {
                    continue;
                }

                // Truncate full command line for privacy
                let command_line = if after.len() > MAX_COMMAND_LINE_LEN {
                    // UTF-8 safe truncation: find the last char boundary before MAX
                    let end = after
                        .char_indices()
                        .take_while(|(i, _)| *i < MAX_COMMAND_LINE_LEN)
                        .last()
                        .map_or(0, |(i, c)| i + c.len_utf8());
                    format!("{}...", &after[..end])
                } else {
                    after.to_string()
                };

                return Some(TerminalCommandInfo {
                    command,
                    command_line,
                    prompt_char: prompt,
                });
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_simple_dollar_prompt() {
        let text = "$ cargo test --workspace";
        let result = detect_terminal_command(text).unwrap();
        assert_eq!(result.command, "cargo");
        assert_eq!(result.command_line, "cargo test --workspace");
        assert_eq!(result.prompt_char, '$');
    }

    #[test]
    fn detect_percent_prompt() {
        let text = "% git status";
        let result = detect_terminal_command(text).unwrap();
        assert_eq!(result.command, "git");
        assert_eq!(result.prompt_char, '%');
    }

    #[test]
    fn detect_hash_prompt_root() {
        let text = "# apt-get update";
        let result = detect_terminal_command(text).unwrap();
        assert_eq!(result.command, "apt-get");
        assert_eq!(result.prompt_char, '#');
    }

    #[test]
    fn detect_chevron_prompt() {
        let text = "> docker compose up";
        let result = detect_terminal_command(text).unwrap();
        assert_eq!(result.command, "docker");
        assert_eq!(result.prompt_char, '>');
    }

    #[test]
    fn detect_user_host_prompt() {
        let text = "user@host:~/projects$ npm install";
        let result = detect_terminal_command(text).unwrap();
        assert_eq!(result.command, "npm");
        assert_eq!(result.prompt_char, '$');
    }

    #[test]
    fn multiline_picks_last_command() {
        let text = "output line 1\noutput line 2\n$ ls -la\n";
        let result = detect_terminal_command(text).unwrap();
        assert_eq!(result.command, "ls");
    }

    #[test]
    fn empty_prompt_returns_none() {
        let text = "$ ";
        assert!(detect_terminal_command(text).is_none());
    }

    #[test]
    fn no_prompt_returns_none() {
        let text = "just some output text without a prompt";
        assert!(detect_terminal_command(text).is_none());
    }

    #[test]
    fn long_command_truncated() {
        let long_cmd = format!("$ {}", "x".repeat(200));
        let result = detect_terminal_command(&long_cmd).unwrap();
        assert!(result.command_line.len() <= MAX_COMMAND_LINE_LEN + 3); // +3 for "..."
        assert!(result.command_line.ends_with("..."));
    }

    #[test]
    fn blank_lines_skipped() {
        let text = "\n\n\n$ cargo build\n\n";
        let result = detect_terminal_command(text).unwrap();
        assert_eq!(result.command, "cargo");
    }
}
