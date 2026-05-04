use super::helpers::{extract_domain_hint, extract_project_from_path};
use super::*;

// ── Existing content extraction tests ──────────────────────────────

#[test]
fn vscode_file_extraction() {
    let parser = TitleBarParser::new();
    let result = parser
        .parse(
            "Visual Studio Code",
            "main.rs - oneshim-core - Visual Studio Code",
        )
        .unwrap();
    assert_eq!(result.content_label, "main.rs");
    assert_eq!(result.content_type, ContentType::File);
    assert!(result.confidence > 0.9);
}

#[test]
fn cursor_file_extraction() {
    let parser = TitleBarParser::new();
    let result = parser
        .parse("Cursor", "lib.rs - my-project - Cursor")
        .unwrap();
    assert_eq!(result.content_label, "lib.rs");
    assert_eq!(result.content_type, ContentType::File);
}

#[test]
fn chrome_page_extraction() {
    let parser = TitleBarParser::new();
    let result = parser
        .parse("Google Chrome", "Rust Programming Language - Google Chrome")
        .unwrap();
    assert_eq!(result.content_label, "Rust Programming Language");
    assert_eq!(result.content_type, ContentType::WebPage);
}

#[test]
fn firefox_page_extraction() {
    let parser = TitleBarParser::new();
    let result = parser
        .parse("Firefox", "GitHub - My Repo - Mozilla Firefox")
        .unwrap();
    assert_eq!(result.content_label, "GitHub - My Repo");
    assert_eq!(result.content_type, ContentType::WebPage);
}

#[test]
fn arc_page_extraction() {
    let parser = TitleBarParser::new();
    let result = parser.parse("Arc", "Stack Overflow - Arc").unwrap();
    assert_eq!(result.content_label, "Stack Overflow");
    assert_eq!(result.content_type, ContentType::WebPage);
}

#[test]
fn slack_channel_pipe_format() {
    let parser = TitleBarParser::new();
    let result = parser
        .parse("Slack", "#general | ONESHIM Workspace")
        .unwrap();
    assert_eq!(result.content_label, "#general");
    assert_eq!(result.content_type, ContentType::Channel);
}

#[test]
fn slack_channel_dash_format() {
    let parser = TitleBarParser::new();
    let result = parser
        .parse("Slack", "ONESHIM Workspace - #engineering")
        .unwrap();
    assert_eq!(result.content_label, "#engineering");
    assert_eq!(result.content_type, ContentType::Channel);
}

#[test]
fn terminal_path_extraction() {
    let parser = TitleBarParser::new();
    let result = parser
        .parse("iTerm2", "user@host: ~/projects/oneshim")
        .unwrap();
    assert_eq!(result.content_label, "~/projects/oneshim");
    assert_eq!(result.content_type, ContentType::File);
}

#[test]
fn warp_terminal() {
    let parser = TitleBarParser::new();
    let result = parser.parse("Warp", "dev@server: /var/log").unwrap();
    assert_eq!(result.content_label, "/var/log");
    assert_eq!(result.content_type, ContentType::File);
}

#[test]
fn intellij_em_dash_format() {
    let parser = TitleBarParser::new();
    let result = parser
        .parse("IntelliJ IDEA", "my-project \u{2013} Main.java")
        .unwrap();
    assert_eq!(result.content_label, "Main.java");
    assert_eq!(result.content_type, ContentType::File);
}

#[test]
fn intellij_dash_fallback_three_parts() {
    let parser = TitleBarParser::new();
    let result = parser
        .parse("IntelliJ IDEA", "myproject - Main.java - IntelliJ IDEA")
        .unwrap();
    assert_eq!(result.content_label, "Main.java");
    assert_eq!(result.content_type, ContentType::File);
    assert!((result.confidence - 0.70).abs() < f32::EPSILON);
}

#[test]
fn intellij_dash_fallback_two_parts() {
    let parser = TitleBarParser::new();
    let result = parser
        .parse("IntelliJ IDEA", "myproject - Main.java")
        .unwrap();
    assert_eq!(result.content_label, "myproject");
    assert_eq!(result.content_type, ContentType::File);
    assert!((result.confidence - 0.60).abs() < f32::EPSILON);
}

#[test]
fn excel_file_extraction() {
    let parser = TitleBarParser::new();
    let result = parser
        .parse("Microsoft Excel", "Budget_2026.xlsx - Excel")
        .unwrap();
    assert_eq!(result.content_label, "Budget_2026.xlsx");
    assert_eq!(result.content_type, ContentType::File);
}

#[test]
fn figma_em_dash_extraction() {
    let parser = TitleBarParser::new();
    let result = parser
        .parse("Figma", "Design System \u{2013} Figma")
        .unwrap();
    assert_eq!(result.content_label, "Design System");
    assert_eq!(result.content_type, ContentType::File);
}

#[test]
fn notion_page_extraction() {
    let parser = TitleBarParser::new();
    let result = parser
        .parse("Notion", "Sprint Planning / Team Workspace")
        .unwrap();
    assert_eq!(result.content_label, "Sprint Planning");
    assert_eq!(result.content_type, ContentType::WebPage);
}

#[test]
fn container_detection_rdp() {
    let parser = TitleBarParser::new();
    assert_eq!(
        parser.is_container_app("Microsoft Remote Desktop"),
        Some(ContainerType::Rdp)
    );
}

#[test]
fn container_detection_vm() {
    let parser = TitleBarParser::new();
    assert_eq!(
        parser.is_container_app("Parallels Desktop"),
        Some(ContainerType::Vm)
    );
    assert_eq!(
        parser.is_container_app("VMware Fusion"),
        Some(ContainerType::Vm)
    );
    assert_eq!(parser.is_container_app("UTM"), Some(ContainerType::Vm));
}

#[test]
fn container_detection_vnc() {
    let parser = TitleBarParser::new();
    assert_eq!(
        parser.is_container_app("VNC Viewer"),
        Some(ContainerType::Vnc)
    );
}

#[test]
fn container_detection_citrix() {
    let parser = TitleBarParser::new();
    assert_eq!(
        parser.is_container_app("Citrix Workspace"),
        Some(ContainerType::Citrix)
    );
}

#[test]
fn container_detection_unknown() {
    let parser = TitleBarParser::new();
    assert_eq!(parser.is_container_app("Visual Studio Code"), None);
}

#[test]
fn unknown_app_generic_parse() {
    let parser = TitleBarParser::new();
    let result = parser.parse("SomeApp", "Document Title - SomeApp").unwrap();
    assert_eq!(result.content_label, "Document Title");
    assert_eq!(result.content_type, ContentType::Unknown);
    assert!(result.confidence <= 0.50);
}

#[test]
fn empty_title_returns_none() {
    let parser = TitleBarParser::new();
    assert!(parser.parse("VSCode", "").is_none());
}

#[test]
fn title_with_no_separator() {
    let parser = TitleBarParser::new();
    let result = parser.parse("UnknownApp", "Just a title").unwrap();
    assert_eq!(result.content_label, "Just a title");
    assert_eq!(result.content_type, ContentType::Unknown);
}

#[test]
fn discord_channel_extraction() {
    let parser = TitleBarParser::new();
    let result = parser.parse("Discord", "#dev-chat | My Server").unwrap();
    assert_eq!(result.content_label, "#dev-chat");
    assert_eq!(result.content_type, ContentType::Channel);
}

// ── Task 1: Project detection from IDE title bars ──────────────────

#[test]
fn vscode_project_extraction() {
    let parser = TitleBarParser::new();
    let result = parser
        .parse(
            "Visual Studio Code",
            "main.rs - oneshim-client - Visual Studio Code",
        )
        .unwrap();
    assert_eq!(result.content_label, "main.rs");
    assert_eq!(result.project, Some("oneshim-client".to_string()));
}

#[test]
fn cursor_project_extraction() {
    let parser = TitleBarParser::new();
    let result = parser
        .parse("Cursor", "lib.rs - my-project - Cursor")
        .unwrap();
    assert_eq!(result.content_label, "lib.rs");
    assert_eq!(result.project, Some("my-project".to_string()));
}

#[test]
fn vscode_two_parts_no_project() {
    // Only file + app name, no project in the middle
    let parser = TitleBarParser::new();
    let result = parser
        .parse("Visual Studio Code", "Welcome - Visual Studio Code")
        .unwrap();
    assert_eq!(result.content_label, "Welcome");
    assert_eq!(result.project, None);
}

#[test]
fn jetbrains_em_dash_project_extraction() {
    let parser = TitleBarParser::new();
    let result = parser
        .parse("IntelliJ IDEA", "oneshim-client \u{2013} Main.java")
        .unwrap();
    assert_eq!(result.content_label, "Main.java");
    assert_eq!(result.project, Some("oneshim-client".to_string()));
}

#[test]
fn jetbrains_dash_three_parts_project() {
    let parser = TitleBarParser::new();
    let result = parser
        .parse("IntelliJ IDEA", "myproject - Main.java - IntelliJ IDEA")
        .unwrap();
    assert_eq!(result.content_label, "Main.java");
    assert_eq!(result.project, Some("myproject".to_string()));
}

#[test]
fn pycharm_project_extraction() {
    let parser = TitleBarParser::new();
    let result = parser
        .parse("PyCharm", "backend-api \u{2013} settings.py")
        .unwrap();
    assert_eq!(result.content_label, "settings.py");
    assert_eq!(result.project, Some("backend-api".to_string()));
}

#[test]
fn rustrover_project_extraction() {
    let parser = TitleBarParser::new();
    let result = parser
        .parse("RustRover", "oneshim-client \u{2013} lib.rs")
        .unwrap();
    assert_eq!(result.content_label, "lib.rs");
    assert_eq!(result.project, Some("oneshim-client".to_string()));
}

#[test]
fn xcode_em_dash_project_extraction() {
    let parser = TitleBarParser::new();
    let result = parser
        .parse("Xcode", "oneshim-client \u{2014} main.swift")
        .unwrap();
    assert_eq!(result.content_label, "main.swift");
    assert_eq!(result.content_type, ContentType::File);
    assert_eq!(result.project, Some("oneshim-client".to_string()));
    assert!((result.confidence - 0.90).abs() < f32::EPSILON);
}

#[test]
fn xcode_en_dash_project_extraction() {
    let parser = TitleBarParser::new();
    let result = parser
        .parse("Xcode", "MyApp \u{2013} ViewController.swift")
        .unwrap();
    assert_eq!(result.content_label, "ViewController.swift");
    assert_eq!(result.project, Some("MyApp".to_string()));
}

#[test]
fn xcode_dash_fallback() {
    let parser = TitleBarParser::new();
    let result = parser.parse("Xcode", "MyApp - AppDelegate.swift").unwrap();
    assert_eq!(result.content_label, "AppDelegate.swift");
    assert_eq!(result.project, Some("MyApp".to_string()));
}

// ── Task 1: Project detection from terminal CWD paths ──────────────

#[test]
fn terminal_project_from_cwd_path() {
    let parser = TitleBarParser::new();
    let result = parser
        .parse("iTerm2", "user@host: ~/projects/oneshim-client")
        .unwrap();
    assert_eq!(result.content_label, "~/projects/oneshim-client");
    assert_eq!(result.project, Some("oneshim-client".to_string()));
}

#[test]
fn terminal_project_from_absolute_path() {
    let parser = TitleBarParser::new();
    let result = parser
        .parse("Terminal", "user@host: /home/dev/code/my-app")
        .unwrap();
    assert_eq!(result.project, Some("my-app".to_string()));
}

#[test]
fn terminal_home_dir_no_project() {
    let parser = TitleBarParser::new();
    let result = parser.parse("Terminal", "user@host: ~").unwrap();
    assert_eq!(result.content_label, "~");
    assert_eq!(result.project, None);
}

#[test]
fn terminal_root_dir_no_project() {
    let parser = TitleBarParser::new();
    let result = parser.parse("Alacritty", "root@host: /").unwrap();
    assert_eq!(result.content_label, "/");
    assert_eq!(result.project, None);
}

#[test]
fn terminal_trailing_slash_stripped() {
    let parser = TitleBarParser::new();
    let result = parser
        .parse("Kitty", "user@host: ~/projects/oneshim/")
        .unwrap();
    assert_eq!(result.project, Some("oneshim".to_string()));
}

// ── Task 1: Non-IDE apps have no project field ─────────────────────

#[test]
fn browser_no_project() {
    let parser = TitleBarParser::new();
    let result = parser
        .parse("Google Chrome", "Rust Programming Language - Google Chrome")
        .unwrap();
    assert_eq!(result.project, None);
}

#[test]
fn communication_no_project() {
    let parser = TitleBarParser::new();
    let result = parser
        .parse("Slack", "#general | ONESHIM Workspace")
        .unwrap();
    assert_eq!(result.project, None);
}

#[test]
fn generic_no_project() {
    let parser = TitleBarParser::new();
    let result = parser.parse("SomeApp", "Document Title - SomeApp").unwrap();
    assert_eq!(result.project, None);
}

// ── Task 2: URL domain extraction from browser titles ──────────────

#[test]
fn browser_domain_hint_gmail() {
    let parser = TitleBarParser::new();
    let result = parser
        .parse("Google Chrome", "Gmail - Inbox (3) - Google Chrome")
        .unwrap();
    assert_eq!(result.domain_hint, Some("gmail".to_string()));
}

#[test]
fn browser_domain_hint_github() {
    let parser = TitleBarParser::new();
    let result = parser
        .parse(
            "Google Chrome",
            "GitHub - pseudotop/maekon-client - Google Chrome",
        )
        .unwrap();
    assert_eq!(result.content_label, "GitHub - pseudotop/maekon-client");
    assert_eq!(result.domain_hint, Some("github".to_string()));
}

#[test]
fn browser_domain_hint_stackoverflow() {
    let parser = TitleBarParser::new();
    let result = parser
        .parse("Arc", "Stack Overflow - How to parse titles - Arc")
        .unwrap();
    assert_eq!(result.domain_hint, Some("stackoverflow".to_string()));
}

#[test]
fn browser_domain_hint_jira() {
    let parser = TitleBarParser::new();
    let result = parser
        .parse("Firefox", "Jira - PROJ-123 Sprint Board - Mozilla Firefox")
        .unwrap();
    assert_eq!(result.domain_hint, Some("jira".to_string()));
}

#[test]
fn browser_domain_hint_youtube() {
    let parser = TitleBarParser::new();
    let result = parser
        .parse("Google Chrome", "YouTube - Rust Tutorial - Google Chrome")
        .unwrap();
    assert_eq!(result.domain_hint, Some("youtube".to_string()));
}

#[test]
fn browser_domain_hint_reddit() {
    let parser = TitleBarParser::new();
    let result = parser.parse("Safari", "Reddit - r/rust - Safari").unwrap();
    assert_eq!(result.domain_hint, Some("reddit".to_string()));
}

#[test]
fn browser_domain_hint_aws() {
    let parser = TitleBarParser::new();
    let result = parser
        .parse("Brave", "AWS Management Console - Brave")
        .unwrap();
    assert_eq!(result.domain_hint, Some("aws".to_string()));
}

#[test]
fn browser_domain_hint_google_docs() {
    let parser = TitleBarParser::new();
    let result = parser
        .parse("Google Chrome", "Google Docs - My Document - Google Chrome")
        .unwrap();
    assert_eq!(result.domain_hint, Some("google-docs".to_string()));
}

#[test]
fn browser_domain_hint_notion() {
    let parser = TitleBarParser::new();
    let result = parser
        .parse("Google Chrome", "Notion - Sprint Planning - Google Chrome")
        .unwrap();
    assert_eq!(result.domain_hint, Some("notion".to_string()));
}

#[test]
fn browser_domain_hint_chatgpt() {
    let parser = TitleBarParser::new();
    let result = parser
        .parse("Google Chrome", "ChatGPT - Google Chrome")
        .unwrap();
    assert_eq!(result.domain_hint, Some("chatgpt".to_string()));
}

#[test]
fn browser_domain_hint_claude() {
    let parser = TitleBarParser::new();
    let result = parser
        .parse("Google Chrome", "Claude - New Conversation - Google Chrome")
        .unwrap();
    assert_eq!(result.domain_hint, Some("claude".to_string()));
}

#[test]
fn browser_no_domain_hint_unknown_site() {
    let parser = TitleBarParser::new();
    let result = parser
        .parse("Google Chrome", "My Personal Blog - Google Chrome")
        .unwrap();
    assert_eq!(result.domain_hint, None);
}

#[test]
fn browser_domain_hint_no_separator() {
    // Browser title without " - " separator
    let parser = TitleBarParser::new();
    let result = parser.parse("Google Chrome", "Gmail").unwrap();
    assert_eq!(result.domain_hint, Some("gmail".to_string()));
}

#[test]
fn browser_domain_hint_mdn() {
    let parser = TitleBarParser::new();
    let result = parser
        .parse("Firefox", "MDN Web Docs - CSS Grid - Mozilla Firefox")
        .unwrap();
    assert_eq!(result.domain_hint, Some("mdn".to_string()));
}

#[test]
fn browser_domain_hint_docs_rs() {
    let parser = TitleBarParser::new();
    let result = parser
        .parse("Google Chrome", "docs.rs - tokio - Google Chrome")
        .unwrap();
    assert_eq!(result.domain_hint, Some("docs-rs".to_string()));
}

#[test]
fn browser_domain_hint_linear() {
    let parser = TitleBarParser::new();
    let result = parser
        .parse("Google Chrome", "Linear - Issue ONS-42 - Google Chrome")
        .unwrap();
    assert_eq!(result.domain_hint, Some("linear".to_string()));
}

// ── Task 2: Non-browser apps have no domain_hint ───────────────────

#[test]
fn ide_no_domain_hint() {
    let parser = TitleBarParser::new();
    let result = parser
        .parse(
            "Visual Studio Code",
            "main.rs - oneshim-client - Visual Studio Code",
        )
        .unwrap();
    assert_eq!(result.domain_hint, None);
}

#[test]
fn terminal_no_domain_hint() {
    let parser = TitleBarParser::new();
    let result = parser
        .parse("iTerm2", "user@host: ~/projects/oneshim")
        .unwrap();
    assert_eq!(result.domain_hint, None);
}

// ── Helper function tests ──────────────────────────────────────────

#[test]
fn extract_project_from_path_basic() {
    assert_eq!(
        extract_project_from_path("~/projects/my-app"),
        Some("my-app".to_string())
    );
}

#[test]
fn extract_project_from_path_absolute() {
    assert_eq!(
        extract_project_from_path("/home/user/code/backend"),
        Some("backend".to_string())
    );
}

#[test]
fn extract_project_from_path_home_only() {
    assert_eq!(extract_project_from_path("~"), None);
}

#[test]
fn extract_project_from_path_root_only() {
    assert_eq!(extract_project_from_path("/"), None);
}

#[test]
fn extract_project_from_path_trailing_slash() {
    assert_eq!(
        extract_project_from_path("~/projects/app/"),
        Some("app".to_string())
    );
}

#[test]
fn extract_domain_hint_known_prefix() {
    assert_eq!(
        extract_domain_hint("GitHub - pseudotop/repo"),
        Some("github".to_string())
    );
}

#[test]
fn extract_domain_hint_unknown_prefix() {
    assert_eq!(extract_domain_hint("Some Random Site - Page"), None);
}

#[test]
fn extract_domain_hint_google_calendar() {
    assert_eq!(
        extract_domain_hint("Google Calendar - March 2026"),
        Some("google-calendar".to_string())
    );
}

#[test]
fn extract_domain_hint_npm() {
    assert_eq!(extract_domain_hint("npm | react"), Some("npm".to_string()));
}

#[test]
fn extract_domain_hint_hacker_news() {
    assert_eq!(
        extract_domain_hint("Hacker News"),
        Some("hackernews".to_string())
    );
}
