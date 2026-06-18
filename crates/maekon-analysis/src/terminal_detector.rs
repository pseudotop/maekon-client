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

        // Find the prompt at a line-STRUCTURAL position (review4 F5). The previous
        // rfind-of-any-occurrence logic mis-parsed shell `$VAR` expansion (it landed
        // on the last `$`, not the prompt) and treated mid-line `>` redirects as
        // prompts. We now take the LEFTMOST structurally-valid prompt instead.
        let Some((pos, prompt)) = find_prompt(trimmed) else {
            continue;
        };

        let after = trimmed[pos + prompt.len_utf8()..].trim_start();
        if after.is_empty() {
            continue;
        }

        // Extract the command (first whitespace-delimited token)
        let command = after.split_whitespace().next().unwrap_or("").to_string();
        if command.is_empty() {
            continue;
        }

        // Scrub argument-borne secrets BEFORE truncation (review4 F4). The bare
        // verb is what propagates into the LLM/digest content label, but
        // `command_line` is a public field, so length truncation alone (the old
        // "privacy" bound) is insufficient — terminal lines carry live API keys,
        // bearer tokens, passwords and connection-string userinfo that the PII
        // filter does not mask at the Basic level.
        let scrubbed = scrub_command_line_secrets(after);
        let command_line = if scrubbed.len() > MAX_COMMAND_LINE_LEN {
            // UTF-8 safe truncation: find the last char boundary before MAX
            let end = scrubbed
                .char_indices()
                .take_while(|(i, _)| *i < MAX_COMMAND_LINE_LEN)
                .last()
                .map_or(0, |(i, c)| i + c.len_utf8());
            format!("{}...", &scrubbed[..end])
        } else {
            scrubbed
        };

        return Some(TerminalCommandInfo {
            command,
            command_line,
            prompt_char: prompt,
        });
    }

    None
}

/// Find the LEFTMOST structurally-valid shell prompt in a trimmed line.
///
/// A prompt char (`$ % # >`) is accepted only when it is immediately followed by
/// whitespace and either (a) it is at the start of the line, or (b) it is attached
/// to a recognized prompt prefix (the preceding text contains `@` or `:`, e.g.
/// `user@host:~$`). `>` is accepted only at the start of the line (a PS1 chevron),
/// never mid-line where it is an output redirect.
fn find_prompt(trimmed: &str) -> Option<(usize, char)> {
    let chars: Vec<(usize, char)> = trimmed.char_indices().collect();
    let mut prefix_signature = false; // saw '@' or ':' before the current position
    for (idx, &(byte_pos, c)) in chars.iter().enumerate() {
        let next_is_ws = chars
            .get(idx + 1)
            .map(|&(_, nc)| nc.is_whitespace())
            .unwrap_or(false);
        if PROMPT_CHARS.contains(&c) && next_is_ws {
            if idx == 0 {
                return Some((byte_pos, c));
            }
            // Mid-line: '>' is a redirect, never a prompt; other prompt chars are
            // valid only when attached to a host/path prompt prefix.
            if c != '>' && prefix_signature {
                return Some((byte_pos, c));
            }
        }
        if c == '@' || c == ':' {
            prefix_signature = true;
        }
    }
    None
}

/// Line-aware secret scrub for multi-line extracted text (e.g. terminal output
/// bound for the LLM accessibility context). Applies the same token-level
/// redaction to each line, preserving line structure so the LLM still sees the
/// layout. Independent of the configured PII level — the maekon-vision PII floor
/// masks email/phone/etc. but NOT API keys / bearer tokens / passwords at the
/// Basic level, which is exactly where terminal accessibility text is exposed.
/// (review4 F4 sibling)
pub fn scrub_text_secrets(text: &str) -> String {
    text.lines()
        .map(scrub_command_line_secrets)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Redact common argument-borne secrets from a captured command line, independent
/// of the configured PII level. Targets `KEY=VALUE` secrets, secret-bearing flags
/// (`--password`/`--token`/…), `Bearer <token>`, and `scheme://user:pass@host`
/// userinfo. (review4 F4)
fn scrub_command_line_secrets(s: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut redact_next = false;
    for tok in s.split_whitespace() {
        if redact_next {
            redact_next = false;
            out.push("[REDACTED]".to_string());
            continue;
        }
        let lower = tok.to_ascii_lowercase();
        if matches!(
            lower.as_str(),
            "--password"
                | "--passwd"
                | "--token"
                | "--secret"
                | "--api-key"
                | "--apikey"
                | "bearer"
        ) {
            out.push(tok.to_string());
            redact_next = true;
            continue;
        }
        out.push(scrub_token(tok));
    }
    out.join(" ")
}

/// Redact a single token if it embeds a `KEY=VALUE` secret or URL userinfo.
fn scrub_token(tok: &str) -> String {
    if let Some(eq) = tok.find('=') {
        let key = &tok[..eq];
        let val = &tok[eq + 1..];
        if !key.is_empty() && !val.is_empty() {
            let key_upper = key.to_ascii_uppercase();
            let sensitive = [
                "KEY",
                "TOKEN",
                "SECRET",
                "PASSWORD",
                "PASSWD",
                "PWD",
                "CREDENTIAL",
                "AUTH",
            ]
            .iter()
            .any(|k| key_upper.contains(k));
            if sensitive || val.len() >= 20 {
                return format!("{key}=[REDACTED]");
            }
        }
    }
    if let Some(sp) = tok.find("://") {
        let after = &tok[sp + 3..];
        if let Some(at) = after.find('@') {
            if after[..at].contains(':') {
                return format!("{}://[REDACTED]@{}", &tok[..sp], &after[at + 1..]);
            }
        }
    }
    tok.to_string()
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

    #[test]
    fn dollar_var_expansion_not_mistaken_for_prompt() {
        // review4 F5: take the leftmost structural prompt, not rfind of any '$'.
        let result = detect_terminal_command("$ echo $HOME").unwrap();
        assert_eq!(result.command, "echo");
        assert_eq!(result.prompt_char, '$');
    }

    #[test]
    fn mid_line_redirect_not_mistaken_for_prompt() {
        // review4 F5: a mid-line '>' is an output redirect, not a prompt.
        assert!(detect_terminal_command("cat a.txt > b.txt").is_none());
    }

    #[test]
    fn export_with_var_keeps_verb() {
        // review4 F5: the prompt is the leftmost '$', so the verb is "export".
        let result = detect_terminal_command("user@host:~$ export TOKEN=$SECRET").unwrap();
        assert_eq!(result.command, "export");
    }

    #[test]
    fn command_line_key_value_secret_scrubbed() {
        // review4 F4: argument-borne secrets are redacted from command_line.
        let r = detect_terminal_command("$ export STRIPE_KEY=sk_live_abc123def456ghi789").unwrap();
        assert_eq!(r.command, "export");
        assert!(!r.command_line.contains("sk_live_abc123def456ghi789"));
        assert!(r.command_line.contains("[REDACTED]"));
    }

    #[test]
    fn command_line_bearer_token_scrubbed() {
        // review4 F4: the token following a standalone "Bearer" is redacted.
        let r = detect_terminal_command("$ curl -H Authorization: Bearer tok_secret_value_xyz")
            .unwrap();
        assert_eq!(r.command, "curl");
        assert!(!r.command_line.contains("tok_secret_value_xyz"));
        assert!(r.command_line.contains("[REDACTED]"));
    }

    #[test]
    fn scrub_text_secrets_preserves_lines_and_redacts() {
        // review4 F4 sibling: line-aware scrub for multi-line accessibility text.
        let input = "line one\nexport API_KEY=sk_live_abcdef0123456789xyz\nline three";
        let out = scrub_text_secrets(input);
        assert_eq!(out.lines().count(), 3, "line structure preserved");
        assert!(!out.contains("sk_live_abcdef0123456789xyz"));
        assert!(out.contains("[REDACTED]"));
        assert!(out.contains("line one"));
        assert!(out.contains("line three"));
    }
}
