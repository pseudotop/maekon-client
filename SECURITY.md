# Security Policy

We take the security of Maekon seriously. If you discover a vulnerability, please follow the procedures outlined in this document to report it.

## Reporting a Security Vulnerability

**Do not report security vulnerabilities as public issues.** Please use the private channels listed below.

### How to Report

1. **Email**: Send an email to `security@maekon.dev`. Please use PGP encryption if possible.
2. **GitHub Security Advisory**: You can report privately by selecting "Report a vulnerability" under the "Security" tab in the repository.

For new Maekon Client reports, prefer the active public repository advisory
form: <https://github.com/pseudotop/maekon-client/security/advisories/new>.
This legacy repository remains a transition/security-only route until archived.

Compatibility note: some crate names, package names, config paths, and release
artifacts still use `oneshim` as a stable technical identifier. Please include
the exact identifier or path shown on disk when it helps reproduce a report.

### Information to Include in Your Report

To enable an effective response, please include as much of the following information as possible.

- **Vulnerability Type**: CWE identifier if applicable (e.g., CWE-79 XSS, CWE-89 SQL Injection, CWE-200 Information Exposure)
- **Affected Crate**: The name of the crate containing the vulnerability and the source file path (e.g., `crates/oneshim-vision/src/privacy.rs`)
- **Steps to Reproduce**: Step-by-step instructions to reproduce the vulnerability
- **Impact**: The expected impact if the vulnerability is exploited (local data exposure, remote code execution, etc.)
- **Proof of Concept (PoC)**: Code or screenshots demonstrating the vulnerability, if available
- **Suggested Fix**: Any ideas for fixing the vulnerability, if you have them (optional)
- **Environment**: OS, Rust version (`rustc --version`), cargo version, and relevant crate versions

### Security Areas of Particular Concern

The following areas are of particular security importance in Maekon.

- **Screen Capture and PII Filter** (`oneshim-vision`): Bypassing the masking of personally identifiable information on-screen
- **Local SQLite Storage** (`oneshim-storage`): Unauthorized access to unencrypted data
- **JWT Authentication Tokens** (`oneshim-network`): Token theft or validation bypass
- **Automation Control** (`oneshim-automation`): Arbitrary command execution via policy validation bypass
- **Auto-Update** (`oneshim-app`): Bypassing integrity verification of update binaries
- **Local Web Dashboard** (`oneshim-web`): Unauthorized access to the local API (related to `allow_external` configuration)

## Supported Versions

Security updates are provided for the following versions.

| Version | Support Status |
|---------|---------------|
| Latest `main` branch | Supported |
| Latest release tag | Supported |
| Previous releases | Not supported |

As no official release has been made yet, please report security vulnerabilities against the **latest `main` branch**.

## Response Time SLA

Upon receiving a security report, we aim to respond according to the following timeline.

| Phase | Target Timeline |
|-------|----------------|
| Acknowledgment of receipt | Within 3 business days |
| Vulnerability assessment and response plan | Within 14 days |
| Patch release | Within 90 days |
| Reporter notification and disclosure schedule | Immediately after patch release |

For urgent security issues (high-severity vulnerabilities such as remote code execution or complete authentication bypass), please include `[URGENT]` in the email subject line to receive priority handling.

## Responsible Disclosure Policy

Maekon follows a **Responsible Disclosure** policy.

### Our Commitments

- We will protect the privacy of the reporter.
- We will notify the reporter upon completion of the fix and coordinate the disclosure schedule with their consent.
- We will credit the reporter in the Security Advisory for their contribution (if desired).
- We will not pursue legal action against good-faith security research activities.

### Our Requests to Reporters

- Please refrain from public disclosure until the vulnerability has been fixed.
- Please ensure that your validation of the vulnerability does not impact other users' data or services.
- Do not destroy, modify, or exfiltrate data without prior authorization.

## Security Contact

| Channel | Contact |
|---------|---------|
| Security Email | `security@maekon.dev` |
| GitHub Security Advisory | Repository Security tab |

## Security Update Notifications

Security updates will be announced through the following channels.

- GitHub Security Advisories
- Release notes (CHANGELOG.md)
- GitHub Releases page

## Integrity References

- Standalone integrity baseline: `docs/security/standalone-integrity-baseline.md`
- Integrity runbook: `docs/security/integrity-runbook.md`
- Local integrity verification script: `scripts/verify-integrity.sh`

---

We appreciate everyone who contributes to improving our security.
