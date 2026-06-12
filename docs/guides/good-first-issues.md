# Good First Issues — OSS Contributor On-Ramp

Welcome to Maekon. This guide gets you from zero to your first merged pull request.

See also: [CONTRIBUTING.md](../../CONTRIBUTING.md) | [CODE_OF_CONDUCT.md](../../CODE_OF_CONDUCT.md)

---

## Prerequisites

- **Rust 1.88.0 or later** — install via [rustup](https://rustup.rs/): `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
- **cargo** — included with Rust
- **Node.js LTS + pnpm** — required for frontend lint/build checks under `crates/maekon-web/frontend`
- **Platform toolchain**:
  - macOS: Xcode Command Line Tools (`xcode-select --install`)
  - Windows: MSVC Build Tools (Visual Studio Installer, "Desktop development with C++")
  - Linux (Ubuntu/Debian): `sudo apt-get install build-essential libwebkitgtk-6.0-dev libgtk-4-dev libclang-dev`

On Windows, run the bootstrap diagnostic before claiming local verification:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/diagnose-windows-dev-bootstrap.ps1
```

If the diagnostic reports blockers, the local session is static scan-only until
`cargo`/`rustc`, real `node`, and `pnpm` are available. A `node.exe` path under
`WindowsApps` usually means the Microsoft Store App Execution Alias is shadowing
the real Node.js installation.

If the diagnostic warns about non-ASCII Windows paths, run native Cargo TC
through the wrapper so Visual Studio and Cargo are prepared consistently:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\run-windows-cargo-tc.ps1
```

---

## Local Dev Setup (6 Steps)

```bash
# 1. Clone the repository
git clone https://github.com/pseudotop/maekon-client.git
cd maekon-client

# 2. Windows only: verify the required local toolchain is executable
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/diagnose-windows-dev-bootstrap.ps1

# 3. Verify the workspace compiles
cargo check --workspace

# 4. Run the test suite (excludes the Tauri GUI binary)
cargo test --workspace --exclude maekon-app

# Windows native full compile check, including maekon-app and native deps
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\run-windows-cargo-tc.ps1

# 5. Pick an issue from GitHub and create a branch
git checkout -b fix/your-issue-title

# 6. Open a PR when ready
# (See the PR checklist below before pushing)
```

---

## Curated Good First Issue Categories

These are concrete, bounded tasks with no deep architectural knowledge required.

### A. Add a PII Filter Pattern

**File**: `crates/maekon-vision/src/privacy.rs`

The PII filter masks sensitive data in window titles and OCR text. New patterns are welcome.

**Example task**: Add a new API key prefix to `mask_api_keys()`.

The function currently recognizes prefixes like `sk-`, `ghp_`, `glpat-`. To add `npx_` (a hypothetical token prefix):

1. Open `crates/maekon-vision/src/privacy.rs`.
2. Locate the `mask_api_keys` function and its list of recognized prefixes.
3. Add the new prefix to the pattern.
4. Add a test in the `#[cfg(test)] mod tests` block at the bottom of the file.

**Why it is a good first issue**: The function is self-contained, the test pattern is already established, and the change does not touch any cross-crate interfaces.

---

### B. Add a Test for an Existing Function

Many functions have minimal test coverage. Adding a test is a low-risk, high-value contribution.

**Example task**: Add edge-case tests for `sanitize_title_with_level` in `crates/maekon-vision/src/privacy.rs`.

Each PII filter level (`Off`, `Basic`, `Standard`, `Strict`) should have tests for boundary inputs (empty string, string with only a PII token, string with multiple token types). Find a level with missing tests and add them.

Test structure:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::config::PiiFilterLevel;

    #[test]
    fn basic_masks_email_but_not_api_key() {
        let result = sanitize_title_with_level("user@example.com sk-abc123", PiiFilterLevel::Basic);
        assert!(result.contains("[EMAIL]"));
        assert!(result.contains("sk-abc123")); // API key only masked at Strict
    }
}
```

---

### C. Fix a Clippy Lint Warning

Run `cargo clippy --workspace` and look for `warning:` lines not suppressed by `#[allow(...)]`. Pick one, fix it, and submit a PR.

Common categories:
- `clippy::needless_pass_by_value` — change `fn foo(x: String)` to `fn foo(x: &str)` where appropriate
- `clippy::map_unwrap_or` — replace `.map(...).unwrap_or(...)` with `.map_or(...)`
- `clippy::redundant_closure` — simplify `|x| f(x)` to `f`

Before fixing, check whether the warning is intentionally suppressed in the surrounding code.

---

### D. Improve Documentation

Doc comments on public functions are always welcome. The project uses English-first documentation.

**Example task**: Add a doc comment to a public function in `crates/maekon-core/src/` that currently has none. Include what the function does, what its parameters mean, and what errors it can return.

```rust
/// Returns the platform-specific directory where MAEKON stores its config file.
///
/// - macOS: `~/Library/Application Support/maekon/`
/// - Windows: `%APPDATA%\maekon\`
/// - Linux: `~/.config/maekon/`
///
/// # Errors
///
/// Returns `CoreError::Config` if the required environment variable (`HOME` or `APPDATA`) is not set.
pub fn config_dir() -> Result<PathBuf, CoreError> {
```

---

## Running the Test Suite

```bash
# All library crates (safe to run on any platform)
cargo test --workspace --exclude maekon-app

# Single crate
cargo test -p maekon-vision
cargo test -p maekon-core
cargo test -p maekon-storage
```

The `maekon-app` crate is excluded because it requires a display server and Tauri initialization. All library crate tests run headless.

---

## PR Checklist

Before opening a pull request:

- [ ] `cargo fmt --check` — no formatting changes required (`cargo fmt` to auto-fix)
- [ ] `cargo clippy --workspace` — no new warnings
- [ ] `cargo test --workspace --exclude maekon-app` — all tests pass
- [ ] Commit messages follow the existing style (imperative mood, present tense, under 72 chars)
- [ ] If you added a public function, it has a doc comment

---

## Getting Help

- Open a GitHub Discussion for questions before you start coding.
- Reference the issue number in your PR title (e.g., `fix: mask npx_ API key prefix (#42)`).
- See [CONTRIBUTING.md](../../CONTRIBUTING.md) for the full contribution guide including branch naming and review expectations.
- See [CODE_OF_CONDUCT.md](../../CODE_OF_CONDUCT.md) for community conduct standards.
