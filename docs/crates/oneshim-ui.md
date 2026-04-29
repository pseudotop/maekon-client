> **DEPRECATED as of v0.1.5** — `oneshim-ui` crate was removed when the project migrated from iced GUI to Tauri v2 + React WebView. This document is kept for historical reference only.
> See: [CHANGELOG.md](../../CHANGELOG.md) for migration details.

---

[English](./oneshim-ui.md) | [한국어](./oneshim-ui.ko.md)

# oneshim-ui

The desktop UI component crate. System tray, notifications, and main window.

## Role

- **System Tray**: Background execution, quick menu
- **Desktop Notifications**: Notification when suggestions arrive
- **Main Window**: Suggestion list, settings, timeline
- **Theme**: Dark/light mode

## Directory Structure

```
oneshim-ui/src/
├── lib.rs         # Crate root
├── tray.rs        # SystemTray - system tray
├── notifier.rs    # DesktopNotifierImpl - notifications
├── theme.rs       # Theme - dark/light theme
└── views/         # UI view components
    ├── mod.rs
    ├── main_window.rs     # Main window
    ├── suggestion_popup.rs # Suggestion popup
    ├── context_panel.rs   # Context panel
    ├── status_bar.rs      # Status bar
    ├── timeline_view.rs   # Timeline
    └── settings.rs        # Settings screen
```

## Key Components

### SystemTray (tray.rs)

System tray based on `tray-icon`:

```rust
pub struct SystemTray {
    tray_icon: TrayIcon,
    menu: Menu,
}

impl SystemTray {
    pub fn new() -> Result<Self, CoreError> {
        let menu = Menu::new();

        menu.append(&MenuItem::with_id("show", "Open Window"))?;
        menu.append(&MenuSeparator)?;
        menu.append(&MenuItem::with_id("status", "Status: Connected"))?;
        menu.append(&MenuSeparator)?;
        menu.append(&MenuItem::with_id("settings", "Settings..."))?;
        menu.append(&MenuItem::with_id("quit", "Quit"))?;

        let icon = Self::load_icon()?;
        let tray_icon = TrayIcon::new(icon, Some("ONESHIM"), Some(&menu))?;

        Ok(Self { tray_icon, menu })
    }

    pub fn set_status(&mut self, status: ConnectionStatus) {
        let text = match status {
            ConnectionStatus::Connected => "Status: Connected ✅",
            ConnectionStatus::Disconnected => "Status: Disconnected ❌",
            ConnectionStatus::Reconnecting => "Status: Reconnecting... 🔄",
        };
        // Update menu item
    }

    pub fn show_indicator(&mut self, has_suggestions: bool) {
        // Show badge on icon when new suggestions are available
    }
}
```

**Tray Menu**:
| Item | Action |
|------|--------|
| Open Window | Show main window |
| Status | Display connection status (read-only) |
| Settings... | Open settings window |
| Quit | Exit the application |

### DesktopNotifierImpl (notifier.rs)

Desktop notifications based on `notify-rust` (`DesktopNotifier` port):

```rust
pub struct DesktopNotifierImpl;

#[async_trait]
impl DesktopNotifier for DesktopNotifierImpl {
    async fn notify(&self, suggestion: &Suggestion) -> Result<(), CoreError> {
        let title = match suggestion.priority {
            Priority::Critical => "🔴 Urgent Suggestion",
            Priority::High => "🟠 Important Suggestion",
            Priority::Medium => "💡 Suggestion",
            Priority::Low => "📝 Note",
        };

        let body = Self::truncate(&suggestion.content, 100);

        Notification::new()
            .summary(title)
            .body(&body)
            .appname("ONESHIM")
            .timeout(Timeout::Milliseconds(5000))
            .show()
            .map_err(|e| CoreError::Internal(e.to_string()))?;

        Ok(())
    }
}
```

### Theme (theme.rs)

Dark/light theme definitions:

```rust
#[derive(Clone)]
pub struct Theme {
    pub background: Color,
    pub surface: Color,
    pub primary: Color,
    pub text: Color,
    pub text_secondary: Color,
    pub border: Color,
    pub success: Color,
    pub warning: Color,
    pub error: Color,
}

impl Theme {
    pub fn dark() -> Self {
        Self {
            background: Color::from_rgb8(30, 30, 30),
            surface: Color::from_rgb8(45, 45, 45),
            primary: Color::from_rgb8(0, 122, 255),
            text: Color::from_rgb8(255, 255, 255),
            text_secondary: Color::from_rgb8(170, 170, 170),
            border: Color::from_rgb8(60, 60, 60),
            success: Color::from_rgb8(48, 209, 88),
            warning: Color::from_rgb8(255, 159, 10),
            error: Color::from_rgb8(255, 69, 58),
        }
    }

    pub fn light() -> Self {
        Self {
            background: Color::from_rgb8(255, 255, 255),
            surface: Color::from_rgb8(242, 242, 247),
            primary: Color::from_rgb8(0, 122, 255),
            text: Color::from_rgb8(0, 0, 0),
            text_secondary: Color::from_rgb8(142, 142, 147),
            border: Color::from_rgb8(209, 209, 214),
            success: Color::from_rgb8(52, 199, 89),
            warning: Color::from_rgb8(255, 149, 0),
            error: Color::from_rgb8(255, 59, 48),
        }
    }

    pub fn system() -> Self {
        // Detect system setting
        if Self::is_dark_mode() {
            Self::dark()
        } else {
            Self::light()
        }
    }
}
```

## Views

### MainWindow (main_window.rs)

Main window based on `iced`:

```rust
pub struct MainWindow {
    suggestions: Vec<SuggestionView>,
    selected: Option<usize>,
    theme: Theme,
}

#[derive(Debug, Clone)]
pub enum Message {
    SuggestionSelected(usize),
    AcceptSuggestion(String),
    RejectSuggestion(String),
    DismissSuggestion(String),
    OpenSettings,
    ThemeChanged(bool),  // true = dark
}

impl Application for MainWindow {
    type Message = Message;

    fn view(&self) -> Element<Message> {
        let header = self.view_header();
        let suggestion_list = self.view_suggestion_list();
        let detail_panel = self.view_detail_panel();

        column![
            header,
            row![
                suggestion_list.width(Length::FillPortion(2)),
                detail_panel.width(Length::FillPortion(3)),
            ]
        ]
        .into()
    }
}
```

### SuggestionPopup (suggestion_popup.rs)

Suggestion detail popup:

```rust
pub struct SuggestionPopup {
    suggestion: SuggestionView,
    position: Point,
}

impl SuggestionPopup {
    pub fn view(&self) -> Element<Message> {
        container(
            column![
                // Header: priority badge + time
                row![
                    text(&self.suggestion.priority_badge),
                    horizontal_space(Length::Fill),
                    text(&self.suggestion.created_ago).size(12),
                ],
                // Title
                text(&self.suggestion.title).size(18),
                // Body
                scrollable(text(&self.suggestion.body)),
                // Action buttons
                row![
                    button("Accept").on_press(Message::AcceptSuggestion(
                        self.suggestion.suggestion_id.clone()
                    )),
                    button("Reject").on_press(Message::RejectSuggestion(
                        self.suggestion.suggestion_id.clone()
                    )),
                    button("Close").on_press(Message::DismissSuggestion(
                        self.suggestion.suggestion_id.clone()
                    )),
                ]
            ]
        )
        .padding(16)
        .style(PopupStyle)
        .into()
    }
}
```

### StatusBar (status_bar.rs)

Bottom status bar:

```rust
pub struct StatusBar {
    connection_status: ConnectionStatus,
    last_sync: Option<DateTime<Utc>>,
    pending_count: usize,
}

impl StatusBar {
    pub fn view(&self) -> Element<Message> {
        let status_indicator = match self.connection_status {
            ConnectionStatus::Connected => text("● Connected").color(Color::GREEN),
            ConnectionStatus::Disconnected => text("● Disconnected").color(Color::RED),
            ConnectionStatus::Reconnecting => text("● Reconnecting...").color(Color::YELLOW),
        };

        let sync_text = self.last_sync
            .map(|t| format!("Last sync: {}", t.format("%H:%M:%S")))
            .unwrap_or_default();

        let pending_text = if self.pending_count > 0 {
            format!("Pending: {}", self.pending_count)
        } else {
            String::new()
        };

        row![
            status_indicator,
            horizontal_space(Length::Fill),
            text(&sync_text).size(12),
            text(&pending_text).size(12),
        ]
        .padding(8)
        .into()
    }
}
```

### TimelineView (timeline_view.rs)

Event timeline:

```rust
pub struct TimelineView {
    events: Vec<TimelineEvent>,
    selected_range: (DateTime<Utc>, DateTime<Utc>),
}

pub struct TimelineEvent {
    pub timestamp: DateTime<Utc>,
    pub event_type: EventType,
    pub title: String,
    pub has_frame: bool,
}

impl TimelineView {
    pub fn view(&self) -> Element<Message> {
        let timeline = self.events.iter().map(|event| {
            let icon = match event.event_type {
                EventType::WindowFocus => "🪟",
                EventType::ApplicationSwitch => "📱",
                EventType::KeyboardInput => "⌨️",
                _ => "•",
            };

            row![
                text(event.timestamp.format("%H:%M:%S").to_string()).size(10),
                text(icon),
                text(&event.title),
                if event.has_frame { text("📷") } else { text("") },
            ]
        });

        scrollable(column(timeline)).into()
    }
}
```

### Settings (settings.rs)

Settings screen:

```rust
pub struct SettingsView {
    config: AppConfig,
    theme_mode: ThemeMode,
}

#[derive(Clone)]
pub enum ThemeMode {
    System,
    Light,
    Dark,
}

impl SettingsView {
    pub fn view(&self) -> Element<Message> {
        column![
            text("Settings").size(24),

            // Server settings
            text("Server").size(18),
            text_input("Server URL", &self.config.server.base_url),

            // Monitoring settings
            text("Monitoring").size(18),
            slider(500..=5000, self.config.monitor.poll_interval_ms, |v| {
                Message::ConfigChanged("monitor.poll_interval_ms", v)
            }),

            // Theme settings
            text("Theme").size(18),
            pick_list(&[ThemeMode::System, ThemeMode::Light, ThemeMode::Dark],
                Some(self.theme_mode.clone()),
                Message::ThemeModeChanged),

            // Auto update
            text("Update").size(18),
            checkbox("Enable auto update", self.config.update.enabled,
                Message::AutoUpdateToggled),
        ]
        .padding(16)
        .into()
    }
}
```

## Dependencies

- `iced`: GUI framework (0.13)
- `tray-icon`: System tray (0.19)
- `notify-rust`: Desktop notifications (4)
- `oneshim-suggestion`: Suggestion presenter

## Platform Support

| Feature | macOS | Windows | Linux |
|---------|-------|---------|-------|
| System Tray | ✅ | ✅ | ✅ |
| Desktop Notifications | ✅ | ✅ | ✅ |
| Dark Mode Detection | ✅ | ✅ | ⚠️ (DE dependent) |
| Main Window | ✅ | ✅ | ✅ |

## Tests

UI tests are primarily performed as integration tests:

```rust
#[test]
fn test_theme_colors() {
    let dark = Theme::dark();
    let light = Theme::light();

    // Dark theme has bright text
    assert!(dark.text.r > 0.5);
    // Light theme has dark text
    assert!(light.text.r < 0.5);
}

#[test]
fn test_status_bar_display() {
    let bar = StatusBar {
        connection_status: ConnectionStatus::Connected,
        last_sync: Some(Utc::now()),
        pending_count: 5,
    };

    // View rendering tests use the iced test framework
}
```
