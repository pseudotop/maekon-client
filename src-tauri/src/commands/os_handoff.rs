//! OS handoff boundary — the one place Maekon hands a URL to the operating
//! system (#9707).
//!
//! Two demo slices need to leave the app: #9627 opens an OS mail compose window
//! for a reviewed draft, and #9628 opens the Console assignment board. Neither
//! could, because nothing here could: `src-tauri/Cargo.toml` carries no
//! `tauri-plugin-opener` / `tauri-plugin-shell`, and `capabilities/default.json`
//! grants no `opener:*` / `shell:*` permission. The only existing way out is
//! webview `window.open(url, '_blank')` (`BugReportWizard.tsx:82,136`,
//! `OAuthConnectionPanel.tsx:98`), which is best-effort in three ways that
//! matter here:
//!
//! 1. Its failure signal is a `null` return, which cannot separate "no mail app
//!    installed" from "popup blocked" from "handoff failed" — #9627 has to
//!    define recovery for exactly those.
//! 2. All three existing call sites are `https:`. Nothing shows `mailto:`
//!    survives that path.
//! 3. The URL it opens is chosen in JavaScript, so any recipient rule lives on
//!    the side of the boundary that the rule is supposed to constrain.
//!
//! ## Why an app-owned command instead of `tauri-plugin-opener`
//!
//! `cargo-vet` is enforced with 347 exemptions, each carrying a `review-by` and
//! a `follow-up` (`supply-chain/config.toml`), so a plugin's dependency graph is
//! a real cost. It buys little here: `windows` 0.62 is already a workspace
//! dependency and `ShellExecuteW` is already called by the vendored runtime
//! (`patches/tauri-runtime-wry-2.11.2/src/dialog/windows.rs:64`), and `url` 2 is
//! already in `Cargo.lock` via `maekon-network`. Tauri itself is vendored
//! (`patches/tauri-2.11.2`, `wry-0.55.1`), so an external plugin would also owe
//! compatibility against that patch set. Owning the command instead keeps the
//! allowlist in Rust, where a caller cannot route around it, and lets failures
//! come back as typed `IpcError` codes.
//!
//! ## Deliberately narrow
//!
//! `https` targets are an exact-match host allowlist; `mailto` recipients are
//! restricted to the RFC 2606 / RFC 6761 reserved domains. The mail restriction
//! is what makes #9627's "cannot be changed to a really deliverable address"
//! acceptance criterion hold at a layer its UI cannot bypass. Both lists are
//! meant to be widened by an explicit, reviewed change — never by a caller.

use std::fmt;

use url::Url;

use crate::ipc_error::IpcError;

mod mailto;

/// Upper bound on an accepted URL, in characters.
///
/// Chosen at the low end of what real handlers accept rather than at any spec
/// limit: everything this boundary is for is a short console link or a template
/// mail draft, and an oversized argument is the interesting half of a hostile
/// input. `mailto:` bodies large enough to matter belong in the mail app, not in
/// an argv entry.
pub const MAX_HANDOFF_URL_CHARS: usize = 8_000;

/// Every policy refusal. One wire code, because reaching any of these means the
/// caller had a bug or the value was tampered with — none is a path a user can
/// walk into, so none deserves its own sentence in five locales.
pub const CODE_REJECTED: &str = "handoff.rejected";
/// No application is registered for the scheme (no mail client installed).
pub const CODE_NO_HANDLER: &str = "handoff.no_handler";
/// A handler exists but the OS could not be asked to run it.
pub const CODE_LAUNCH_FAILED: &str = "handoff.launch_failed";

// Hosts reachable over `https` come from `ServerConfig::allowed_handoff_hosts`.
//
// #9785 moved them out of a hardcoded constant. The original list named the
// vendor Console directly, which was the only hostname literal in this client —
// everything else, `base_url` included, is configured — so the public-export
// scanner flagged it, correctly. The constant also had a limit its own author
// wrote down: a self-hosted Console could never be reached through a static
// list. Configuration answers both.
//
// What is NOT relaxed: matching stays exact and case-insensitive. A suffix rule
// would turn `evil-app.<vendor-domain>` into an allowed target, so neither
// wildcards nor suffixes are accepted here.
//
// Empty is the default, and empty means no https handoff at all. That is
// deny-by-default in the direction that matters: a misconfigured client refuses
// to open anything rather than opening the wrong thing. #9628 is the consumer.

/// Percent-encoded octets refused in header-bearing URL positions.
const REFUSED_ENCODED_OCTETS: &[&str] = &["%0a", "%0d", "%00"];

/// Why a URL was refused.
///
/// Finer-grained than the single wire code it maps to, because the distinction
/// is worth asserting in tests even when it is not worth showing to a user:
/// "we rejected the scheme" and "we rejected the host" fail for different
/// reasons and a test that cannot tell them apart passes on the wrong one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejection {
    Empty,
    TooLong { chars: usize },
    ControlCharacter,
    EncodedControlOctet,
    Unparsable,
    SchemeNotAllowed { scheme: String },
    HostMissing,
    HostNotAllowed { host: String },
    MailtoRecipientMissing,
    MailtoRecipientMalformed,
    MailtoDomainNotReserved { domain: String },
    MailtoHeaderNotAllowed,
}

impl fmt::Display for Rejection {
    /// Never interpolates the URL, the recipient local-part, or the query.
    ///
    /// This string reaches `IpcError::message`, which is logged and can be
    /// surfaced; the rejected value is attacker-influenced and, for `mailto`,
    /// carries a draft body. The scheme, the host, and the recipient *domain*
    /// are named because all three are drawn from a closed set we already
    /// publish in this file.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "empty handoff target"),
            Self::TooLong { chars } => {
                write!(
                    f,
                    "handoff target too long ({chars} > {MAX_HANDOFF_URL_CHARS} chars)"
                )
            }
            Self::ControlCharacter => write!(f, "handoff target contains a control character"),
            Self::EncodedControlOctet => {
                write!(f, "handoff target contains an encoded control octet")
            }
            Self::Unparsable => write!(f, "handoff target is not a parsable URL"),
            Self::SchemeNotAllowed { scheme } => {
                write!(
                    f,
                    "scheme `{scheme}` is not handed off; only https and mailto are"
                )
            }
            Self::HostMissing => write!(f, "https handoff target has no host"),
            Self::HostNotAllowed { host } => write!(f, "host `{host}` is not an allowed target"),
            Self::MailtoRecipientMissing => write!(f, "mailto handoff target has no recipient"),
            Self::MailtoRecipientMalformed => write!(f, "mailto recipient is not a single address"),
            Self::MailtoDomainNotReserved { domain } => write!(
                f,
                "mailto domain `{domain}` is not an RFC 2606/6761 reserved domain"
            ),
            Self::MailtoHeaderNotAllowed => {
                write!(f, "mailto query permits one subject and one body only")
            }
        }
    }
}

/// A URL that passed every policy check, ready to hand to the OS.
///
/// The only constructor is [`validate`], so holding one is the proof. Callers
/// cannot assemble it from an arbitrary string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedTarget {
    url: String,
}

impl ValidatedTarget {
    /// The exact string to hand to the OS.
    pub fn as_str(&self) -> &str {
        &self.url
    }
}

/// True if `input` holds any C0 control, DEL, or a raw newline/tab.
///
/// Checked against the *raw* input rather than the parsed URL because the
/// WHATWG parser strips tabs and newlines silently — by the time `Url::parse`
/// has returned, the evidence that they were there is gone.
fn has_control_character(input: &str) -> bool {
    input.chars().any(|c| c.is_control())
}

/// Apply the whole policy to a caller-supplied string.
///
/// Pure: no filesystem, no process, no state. Every acceptance criterion about
/// what may leave the app is decided here, so the negative cases are ordinary
/// unit tests rather than something that needs a desktop session.
/// Validate a handoff target against `allowed_hosts`.
///
/// `allowed_hosts` is passed in rather than read here so this stays a pure
/// function: the whole rejection table below is testable without constructing
/// an `AppConfig`, which is how it was tested before #9785 and still is.
pub fn validate(input: &str, allowed_hosts: &[String]) -> Result<ValidatedTarget, Rejection> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(Rejection::Empty);
    }

    // Length before parsing: `Url::parse` on a multi-megabyte argument is work
    // we already know we will throw away.
    let chars = trimmed.chars().count();
    if chars > MAX_HANDOFF_URL_CHARS {
        return Err(Rejection::TooLong { chars });
    }

    if has_control_character(trimmed) {
        return Err(Rejection::ControlCharacter);
    }

    let url = Url::parse(trimmed).map_err(|_| Rejection::Unparsable)?;

    match url.scheme() {
        "https" => {
            let lowered = trimmed.to_ascii_lowercase();
            if REFUSED_ENCODED_OCTETS
                .iter()
                .any(|octet| lowered.contains(octet))
            {
                return Err(Rejection::EncodedControlOctet);
            }
            let host = url
                .host_str()
                .ok_or(Rejection::HostMissing)?
                .to_ascii_lowercase();
            // Exact, case-insensitive. `host` is already lowercased; lowercase
            // the configured side too so a capitalised config entry is not a
            // silent no-match.
            if !allowed_hosts
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(&host))
            {
                return Err(Rejection::HostNotAllowed { host });
            }
        }
        "mailto" => {
            mailto::validate(&url)?;
        }
        other => {
            return Err(Rejection::SchemeNotAllowed {
                scheme: other.to_owned(),
            })
        }
    }

    // Hand on the trimmed original, not `url.to_string()`. The parser
    // normalizes (it appends a `/` path to a bare https origin, reorders
    // nothing but re-encodes some octets), and for `mailto:` that rewriting is
    // exactly the kind of silent change we do not want between the string the
    // policy inspected and the string the OS receives.
    Ok(ValidatedTarget {
        url: trimmed.to_owned(),
    })
}

/// What went wrong once the OS was actually asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchFailure {
    /// Nothing is registered for the scheme — no mail client, typically.
    NoHandler,
    /// A handler may exist, but the request could not be completed.
    Failed { detail: String },
}

impl From<Rejection> for IpcError {
    fn from(rejection: Rejection) -> Self {
        IpcError::new(CODE_REJECTED, rejection.to_string())
    }
}

impl From<LaunchFailure> for IpcError {
    fn from(failure: LaunchFailure) -> Self {
        match failure {
            LaunchFailure::NoHandler => IpcError::new(
                CODE_NO_HANDLER,
                "no application is registered to open this target",
            ),
            LaunchFailure::Failed { detail } => IpcError::new(CODE_LAUNCH_FAILED, detail),
        }
    }
}

/// Hand `target` to the OS.
///
/// Never goes through a shell on any platform. On the Unix paths that means
/// `Command`, which `exec`s directly, and on Windows it means `ShellExecuteW`
/// rather than `cmd /c start` — `start` is parsed by `cmd`, which is the
/// argument-injection vector this boundary exists to close.
///
/// Flag injection is closed upstream instead of by an argument separator:
/// [`validate`] admits only `https:` and `mailto:` URLs, so an accepted target
/// can never begin with `-`.
#[cfg(target_os = "macos")]
fn launch(target: &ValidatedTarget) -> Result<(), LaunchFailure> {
    use std::process::Command;

    // Absolute path: resolving `open` through `PATH` would let a directory
    // earlier in `PATH` decide what runs.
    let status = Command::new("/usr/bin/open")
        .arg(target.as_str())
        .status()
        .map_err(|e| LaunchFailure::Failed {
            detail: format!("could not run /usr/bin/open: {e}"),
        })?;

    if status.success() {
        return Ok(());
    }

    // `open(1)` reports "no application knows how to open" through the same
    // exit status it uses for every other failure, so macOS cannot separate
    // NoHandler from Failed without parsing a localized stderr string. It
    // reports the honest, coarser answer instead; the UI sentence for
    // `handoff.launch_failed` covers both.
    Err(LaunchFailure::Failed {
        detail: format!("/usr/bin/open exited with {status}"),
    })
}

/// Launch a target that already passed [`validate`].
///
/// Exposed inside the app crate so dedicated commands can own URL construction
/// while still sharing the single shell-free OS boundary.
pub async fn launch_validated(target: ValidatedTarget) -> Result<(), IpcError> {
    tauri::async_runtime::spawn_blocking(move || launch(&target))
        .await
        .map_err(|error| {
            IpcError::new(CODE_LAUNCH_FAILED, format!("handoff task failed: {error}"))
        })?
        .map_err(IpcError::from)
}

#[cfg(target_os = "linux")]
fn launch(target: &ValidatedTarget) -> Result<(), LaunchFailure> {
    use std::process::Command;

    let status = Command::new("xdg-open")
        .arg(target.as_str())
        .status()
        .map_err(|e| LaunchFailure::Failed {
            detail: format!("could not run xdg-open: {e}"),
        })?;

    if status.success() {
        return Ok(());
    }

    // xdg-open(1) exit codes: 2 unknown target, 3 required tool missing,
    // 4 action failed. 3 is the closest thing to "nothing is installed".
    match status.code() {
        Some(3) => Err(LaunchFailure::NoHandler),
        other => Err(LaunchFailure::Failed {
            detail: match other {
                Some(code) => format!("xdg-open exited with {code}"),
                None => "xdg-open terminated by signal".to_owned(),
            },
        }),
    }
}

#[cfg(target_os = "windows")]
fn launch(target: &ValidatedTarget) -> Result<(), LaunchFailure> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    fn wide(value: &str) -> Vec<u16> {
        std::ffi::OsStr::new(value)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    let verb = wide("open");
    let file = wide(target.as_str());

    // SAFETY: both pointers are NUL-terminated wide strings owned by locals
    // that outlive the call, and the remaining arguments are null, which this
    // API documents as "no parent window / no parameters / no working
    // directory".
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };

    // ShellExecuteW returns a fake HINSTANCE: > 32 on success, and an SE_ERR_*
    // code otherwise.
    let code = result as usize;
    if code > 32 {
        return Ok(());
    }

    const SE_ERR_NOASSOC: usize = 31;
    const SE_ERR_ASSOCINCOMPLETE: usize = 27;
    match code {
        SE_ERR_NOASSOC | SE_ERR_ASSOCINCOMPLETE => Err(LaunchFailure::NoHandler),
        other => Err(LaunchFailure::Failed {
            detail: format!("ShellExecuteW failed with code {other}"),
        }),
    }
}

/// Open an external target in the user's default application.
///
/// Deliberately not gated on `cfg(feature = "server")`. The connected-mode
/// slices are the reason it exists, but opening a link is a local-first
/// capability and gating it would make the default build the odd one out.
///
/// One call hands off at most once: it validates, asks the OS, and returns.
/// There is no retry and no queue.
#[tauri::command]
pub async fn open_external_target(
    url: String,
    state: tauri::State<'_, crate::runtime_state::ConfigRuntimeState>,
) -> Result<(), IpcError> {
    // #9785: read the allowlist per call rather than caching it. Settings are
    // editable at runtime, and a cached copy would keep honouring a host the
    // user has just removed — the wrong direction to be stale in for a control
    // whose whole job is refusing destinations.
    let allowed = state.config_manager().get().server.allowed_handoff_hosts;
    let target = validate(&url, &allowed)?;

    // `status()` blocks until the launcher process exits, which is how the
    // failure classes above are observed at all. Off the async runtime's
    // worker so a slow handler cannot stall unrelated IPC.
    launch_validated(target).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Allowlist used by the tests.
    ///
    /// #9785: reserved-for-documentation hosts (RFC 2606 §3), not the vendor
    /// Console. The old tests named it directly, which is why the public-export
    /// scanner flagged this file's test lines as well as its source. What the
    /// tests actually prove — exact match, case-insensitivity, scheme policy —
    /// never depended on which host it was.
    fn allowed() -> Vec<String> {
        vec![
            "console.example.com".to_string(),
            "preview.example.com".to_string(),
        ]
    }

    /// The default configuration allows no https handoff at all.
    ///
    /// This is the behaviour change #9785 makes, so it is pinned rather than
    /// left implied: a client that has not been told where its Console lives
    /// refuses to open one, instead of carrying a vendor address it inherited
    /// from a constant.
    #[test]
    fn the_empty_default_allowlist_refuses_every_https_target() {
        for input in [
            "https://console.example.com",
            "https://console.example.com/assignments/board",
            "https://anything.example.org",
        ] {
            let err = validate(input, &[]).unwrap_err();
            assert!(
                matches!(err, Rejection::HostNotAllowed { .. }),
                "{input} must be refused when nothing is allowlisted, got {err:?}"
            );
        }
    }

    /// A bare trailing dot is refused, and that is deliberate.
    ///
    /// #9785 review asked what happens here. Measured rather than reasoned:
    /// `Url::parse("https://console.example.com.").host_str()` returns
    /// `"console.example.com."` — the dot survives parsing — so exact match
    /// refuses it. RFC 1034 §3.1 says that names the same FQDN, so this is a
    /// refusal of a legitimate address, not of an attack.
    ///
    /// It is left refusing anyway. The behaviour is identical to the constant
    /// this replaced, so it is not a regression this change introduced, and
    /// for an allowlist deciding where to send a browser, refusing an address
    /// that is merely spelled unusually is the safe direction to be wrong in.
    /// Admitting it would mean normalizing the host, which is a new surface
    /// that wants its own slice.
    #[test]
    fn a_trailing_dot_host_is_refused_rather_than_normalized() {
        let err = validate("https://console.example.com.", &allowed()).unwrap_err();
        assert!(
            matches!(err, Rejection::HostNotAllowed { .. }),
            "trailing-dot host must fail closed, got {err:?}"
        );
    }

    /// A configured host matches regardless of the case it was written in.
    #[test]
    fn configured_hosts_match_case_insensitively() {
        let configured = vec!["Console.Example.COM".to_string()];
        let target = validate("https://CONSOLE.example.com/board", &configured)
            .expect("case must not decide the match");
        assert_eq!(target.as_str(), "https://CONSOLE.example.com/board");
    }

    /// Exact match survives the move to configuration.
    ///
    /// The constant's author refused a suffix rule because it would admit
    /// `evil-app.<vendor-domain>`. Configuration must not quietly reintroduce
    /// what the constant deliberately excluded.
    #[test]
    fn a_configured_host_does_not_admit_its_subdomains_or_its_parent() {
        for input in [
            "https://evil-console.example.com",
            "https://console.example.com.attacker.test",
            "https://example.com",
            "https://sub.console.example.com",
        ] {
            let err = validate(input, &allowed()).unwrap_err();
            assert!(
                matches!(err, Rejection::HostNotAllowed { .. }),
                "{input} must not be admitted by an exact-match allowlist, got {err:?}"
            );
        }
    }

    #[test]
    fn accepts_an_allowlisted_https_host() {
        let target = validate("https://console.example.com/assignments/board", &allowed())
            .expect("allowlisted console host");
        assert_eq!(
            target.as_str(),
            "https://console.example.com/assignments/board"
        );
    }

    #[test]
    fn accepts_a_reserved_domain_mailto() {
        let target = validate("mailto:pm@example.com?subject=WBS", &allowed())
            .expect("reserved mail domain");
        assert_eq!(target.as_str(), "mailto:pm@example.com?subject=WBS");
    }

    #[test]
    fn preserves_the_caller_string_rather_than_the_parser_normalization() {
        // `Url::parse("https://host").to_string()` appends a `/`. Handing the
        // OS a string the policy never inspected is the drift this closes.
        let target = validate("https://console.example.com", &allowed()).expect("bare origin");
        assert_eq!(target.as_str(), "https://console.example.com");
    }

    #[test]
    fn rejects_every_scheme_outside_https_and_mailto() {
        for input in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/html,<script>alert(1)</script>",
            "http://console.example.com",
            "ftp://console.example.com",
            "vscode://file/etc/passwd",
        ] {
            let err = validate(input, &allowed()).unwrap_err();
            assert!(
                matches!(err, Rejection::SchemeNotAllowed { .. }),
                "{input} should be refused for its scheme, got {err:?}"
            );
        }
    }

    #[test]
    fn rejects_a_bare_path_with_no_scheme() {
        assert_eq!(
            validate("console.example.com", &allowed()).unwrap_err(),
            Rejection::Unparsable
        );
        assert_eq!(
            validate("/etc/passwd", &allowed()).unwrap_err(),
            Rejection::Unparsable
        );
    }

    #[test]
    fn rejects_https_hosts_outside_the_allowlist() {
        for input in [
            "https://example.com",
            "https://evil.com",
            // The reason the rule is exact-match and not a suffix rule.
            "https://evil-console.example.com",
            "https://console.example.com.evil.com",
            // Userinfo that reads like the allowed host to a human skimming it.
            "https://console.example.com@evil.com",
        ] {
            let err = validate(input, &allowed()).unwrap_err();
            assert!(
                matches!(err, Rejection::HostNotAllowed { .. }),
                "{input} should be refused for its host, got {err:?}"
            );
        }
    }

    #[test]
    fn matches_the_allowlisted_host_case_insensitively() {
        validate("https://CONSOLE.EXAMPLE.COM/board", &allowed())
            .expect("host match is case-insensitive");
    }

    #[test]
    fn matches_reserved_mail_domains_case_insensitively() {
        // Domain names are case-insensitive. Comparing raw against the
        // lowercase reserved lists would refuse an address that IS reserved,
        // making #9627's recipients depend on their casing.
        validate("mailto:PM@EXAMPLE.COM", &allowed())
            .expect("reserved second-level domain, uppercased");
        validate("mailto:PM@Vendor.Invalid", &allowed()).expect("reserved TLD, mixed case");
    }

    #[test]
    fn rejects_mailto_recipients_outside_the_reserved_domains() {
        for input in [
            "mailto:someone@gmail.com",
            // #9785: was the vendor's own domain. Any domain outside the
            // reserved lists proves the same rule, and this file should not be
            // the one place that names it.
            "mailto:pm@acme.co",
            "mailto:a@example.com.evil.com",
        ] {
            let err = validate(input, &allowed()).unwrap_err();
            assert!(
                matches!(err, Rejection::MailtoDomainNotReserved { .. }),
                "{input} should be refused for its domain, got {err:?}"
            );
        }
    }

    #[test]
    fn rejects_a_deliverable_recipient_hidden_behind_a_reserved_one() {
        // Every recipient is checked, not just the first.
        let err = validate("mailto:ok@example.com,real@gmail.com", &allowed()).unwrap_err();
        assert!(
            matches!(err, Rejection::MailtoDomainNotReserved { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn accepts_the_reserved_tlds_but_not_localhost() {
        validate("mailto:a@vendor.example", &allowed()).expect(".example is reserved");
        validate("mailto:a@vendor.invalid", &allowed()).expect(".invalid is reserved");
        validate("mailto:a@vendor.test", &allowed()).expect(".test is reserved");

        // `.localhost` can genuinely deliver to a local MTA.
        let err = validate("mailto:a@vendor.localhost", &allowed()).unwrap_err();
        assert!(
            matches!(err, Rejection::MailtoDomainNotReserved { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn rejects_malformed_mailto_recipients() {
        for input in [
            "mailto:",
            "mailto:?subject=hi",
            "mailto:@example.com",
            "mailto:nobody",
            "mailto:a%20b@example.com",
            "mailto:.a@example.com",
            "mailto:a..b@example.com",
            "mailto:a@-vendor.example",
            "mailto:a@vendor..example",
        ] {
            let err = validate(input, &allowed()).unwrap_err();
            assert!(
                matches!(
                    err,
                    Rejection::MailtoRecipientMissing | Rejection::MailtoRecipientMalformed
                ),
                "{input} should be refused as malformed, got {err:?}"
            );
        }
    }

    #[test]
    fn rejects_encoded_crlf_header_injection() {
        // #9627's header-injection criterion, enforced where the UI cannot
        // route around it. These survive URL parsing intact, which is exactly
        // why they are the dangerous form.
        for input in [
            "mailto:a@example.com%0aBcc:victim@gmail.com",
            "mailto:a@example.com%0D%0ABcc:victim@gmail.com",
            "mailto:a@example.com?subject=hi%0aBcc:victim@gmail.com",
            "mailto:a@example.com%00",
            // Case must not be an escape hatch.
            "mailto:a@example.com%0ABcc:victim@gmail.com",
        ] {
            assert_eq!(
                validate(input, &allowed()).unwrap_err(),
                Rejection::EncodedControlOctet,
                "{input} should be refused for an encoded control octet"
            );
        }
    }

    #[test]
    fn permits_encoded_newlines_only_inside_body() {
        let target = validate(
            "mailto:a@example.com?subject=Review&body=Line%201%0D%0ALine%202",
            &allowed(),
        )
        .expect("body line breaks are message content, not headers");
        assert!(target.as_str().contains("body=Line%201%0D%0ALine%202"));

        for input in [
            "mailto:a@example.com?cc=real@gmail.com",
            "mailto:a@example.com?bcc=real@gmail.com",
            "mailto:a@example.com?to=real@gmail.com",
            "mailto:a@example.com?subject=one&subject=two",
            "mailto:a@example.com?body=one&body=two",
        ] {
            assert_eq!(
                validate(input, &allowed()).unwrap_err(),
                Rejection::MailtoHeaderNotAllowed,
                "{input} must not add recipients or duplicate fields"
            );
        }
    }

    #[test]
    fn rejects_raw_control_characters_the_url_parser_would_have_stripped() {
        // `Url::parse` silently drops raw tabs and newlines, so checking after
        // parsing would find a clean URL and forget these were ever present.
        for input in [
            "https://console.example.com/\nevil",
            "mailto:a@example.com\r\nBcc:victim@gmail.com",
            "https://console.example.com/\tboard",
        ] {
            assert_eq!(
                validate(input, &allowed()).unwrap_err(),
                Rejection::ControlCharacter,
                "{input:?} should be refused for a raw control character"
            );
        }
    }

    #[test]
    fn rejects_empty_and_whitespace_only_targets() {
        assert_eq!(validate("", &allowed()).unwrap_err(), Rejection::Empty);
        assert_eq!(validate("   ", &allowed()).unwrap_err(), Rejection::Empty);
    }

    #[test]
    fn rejects_targets_over_the_length_bound() {
        let long = format!(
            "https://console.example.com/{}",
            "a".repeat(MAX_HANDOFF_URL_CHARS)
        );
        let err = validate(&long, &allowed()).unwrap_err();
        assert!(matches!(err, Rejection::TooLong { .. }), "{err:?}");

        // The bound itself is accepted — an off-by-one here would silently
        // shrink what callers may send.
        let prefix = "https://console.example.com/";
        let exact = format!(
            "{prefix}{}",
            "a".repeat(MAX_HANDOFF_URL_CHARS - prefix.len())
        );
        assert_eq!(exact.chars().count(), MAX_HANDOFF_URL_CHARS);
        validate(&exact, &allowed()).expect("a target exactly at the bound is accepted");
    }

    #[test]
    fn an_accepted_target_can_never_look_like_a_command_flag() {
        // What makes the Unix `Command` calls safe without an argument
        // separator. If `validate` ever admits another scheme, this fails.
        for input in ["https://console.example.com/board", "mailto:a@example.com"] {
            let target = validate(input, &allowed()).expect(input);
            assert!(
                target.as_str().starts_with("https:") || target.as_str().starts_with("mailto:"),
                "{input} produced a target that could be read as a flag"
            );
            assert!(!target.as_str().starts_with('-'));
        }
    }

    #[test]
    fn rejection_messages_never_echo_the_rejected_target() {
        // These strings land in `IpcError::message`, which is logged. A
        // `mailto:` rejection carries a draft body; an https rejection carries
        // whatever the tamperer chose.
        let secret = "s3cret-draft-body";
        let err = validate(
            &format!("mailto:victim@gmail.com?body={secret}"),
            &allowed(),
        )
        .unwrap_err();
        let message = IpcError::from(err).message;
        assert!(!message.contains(secret), "leaked the body: {message}");
        assert!(
            !message.contains("victim"),
            "leaked the local-part: {message}"
        );
        // The domain is named, and is drawn from the closed set this file
        // publishes.
        assert!(message.contains("gmail.com"), "{message}");
    }

    #[test]
    fn every_rejection_maps_to_the_single_policy_wire_code() {
        for input in ["", "file:///etc/passwd", "mailto:a@gmail.com", "not a url"] {
            let err = IpcError::from(validate(input, &allowed()).unwrap_err());
            assert_eq!(err.code, CODE_REJECTED, "{input}");
        }
    }

    #[test]
    fn launch_failures_map_to_distinct_wire_codes() {
        // #9627 needs "no mail app" and "handoff failed" to be different
        // answers; a single failure code would make its recovery branch dead.
        assert_eq!(
            IpcError::from(LaunchFailure::NoHandler).code,
            CODE_NO_HANDLER
        );
        assert_eq!(
            IpcError::from(LaunchFailure::Failed {
                detail: "boom".to_owned()
            })
            .code,
            CODE_LAUNCH_FAILED
        );
        assert_ne!(CODE_NO_HANDLER, CODE_LAUNCH_FAILED);
    }

    #[test]
    fn wire_codes_stay_outside_the_adr019_registry() {
        // `tauriIpcErrors.ts` may only map codes the registry does not define;
        // shadowing one would replace its catalog template with a fixed UI
        // sentence. Read the registry rather than restating it.
        let registry =
            include_str!("../../../crates/maekon-core/tests/wire_contract_snapshot.expected.txt");
        let codes: Vec<&str> = registry
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        for code in [CODE_REJECTED, CODE_NO_HANDLER, CODE_LAUNCH_FAILED] {
            assert!(
                !codes.contains(&code),
                "{code} is an ADR-019 registry code; minting it here would shadow its template"
            );
        }
    }
}
