/// Known title prefix → domain hint mappings.
/// Checked in order; first match wins.
const DOMAIN_PATTERNS: &[(&str, &str)] = &[
    // Google services
    ("Gmail", "gmail"),
    ("Google Calendar", "google-calendar"),
    ("Google Meet", "google-meet"),
    ("Google Drive", "google-drive"),
    ("Google Docs", "google-docs"),
    ("Google Sheets", "google-sheets"),
    ("Google Slides", "google-slides"),
    // Developer tools
    ("GitHub", "github"),
    ("GitLab", "gitlab"),
    ("Bitbucket", "bitbucket"),
    ("Stack Overflow", "stackoverflow"),
    ("Stack Exchange", "stackexchange"),
    ("npm", "npm"),
    ("crates.io", "crates-io"),
    ("PyPI", "pypi"),
    ("Docker Hub", "dockerhub"),
    ("Rust Playground", "rust-playground"),
    // Project management
    ("Jira", "jira"),
    ("Confluence", "confluence"),
    ("Trello", "trello"),
    ("Asana", "asana"),
    ("Linear", "linear"),
    ("Notion", "notion"),
    // Communication
    ("Slack", "slack"),
    ("Discord", "discord"),
    ("Microsoft Teams", "teams"),
    ("Zoom", "zoom"),
    // Documentation / Knowledge
    ("Wikipedia", "wikipedia"),
    ("MDN Web Docs", "mdn"),
    ("docs.rs", "docs-rs"),
    ("Hacker News", "hackernews"),
    ("Reddit", "reddit"),
    ("Medium", "medium"),
    ("Dev.to", "devto"),
    // Cloud / Infra
    ("AWS", "aws"),
    ("Azure", "azure"),
    ("Google Cloud", "gcp"),
    ("Vercel", "vercel"),
    ("Netlify", "netlify"),
    ("Cloudflare", "cloudflare"),
    // Design
    ("Figma", "figma"),
    // AI
    ("ChatGPT", "chatgpt"),
    ("Claude", "claude"),
    // Office
    ("Outlook", "outlook"),
    ("OneDrive", "onedrive"),
    ("Dropbox", "dropbox"),
    // Video / Media
    ("YouTube", "youtube"),
    ("Twitch", "twitch"),
    ("Spotify", "spotify"),
];

/// Extract a project name from a terminal CWD path.
///
/// Takes the last path component (directory name) as the project name.
/// e.g., "~/projects/oneshim-client" → "oneshim-client"
///       "/home/user/code/my-app"    → "my-app"
pub(super) fn extract_project_from_path(path: &str) -> Option<String> {
    let trimmed = path.trim().trim_end_matches('/');
    if trimmed.is_empty() || trimmed == "~" || trimmed == "/" {
        return None;
    }

    // Take the last path component
    let last = trimmed.rsplit('/').next().unwrap_or(trimmed);
    if last.is_empty() || last == "~" {
        return None;
    }

    Some(last.to_string())
}

/// Extract a domain hint from a browser page title using known prefix patterns.
///
/// Maps common page title prefixes to normalized domain identifiers.
/// This is heuristic-based and aims for ~80% accuracy on common sites.
pub(super) fn extract_domain_hint(page_title: &str) -> Option<String> {
    let trimmed = page_title.trim();
    for &(prefix, domain) in DOMAIN_PATTERNS {
        if trimmed.starts_with(prefix) {
            return Some(domain.to_string());
        }
    }

    None
}
