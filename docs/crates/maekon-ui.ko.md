[English](./maekon-ui.md) | [한국어](./maekon-ui.ko.md)

# maekon-ui

데스크톱 UI 컴포넌트 크레이트. 시스템 트레이, 알림, 메인 윈도우.

## 역할

- **시스템 트레이**: 백그라운드 실행, 빠른 메뉴
- **데스크톱 알림**: 제안 도착 알림
- **메인 윈도우**: 제안 목록, 설정, 타임라인
- **테마**: 다크/라이트 모드

## 디렉토리 구조

```
maekon-ui/src/
├── lib.rs         # 크레이트 루트
├── tray.rs        # SystemTray - 시스템 트레이
├── notifier.rs    # DesktopNotifierImpl - 알림
├── theme.rs       # Theme - 다크/라이트 테마
└── views/         # UI 뷰 컴포넌트
    ├── mod.rs
    ├── main_window.rs     # 메인 윈도우
    ├── suggestion_popup.rs # 제안 팝업
    ├── context_panel.rs   # 컨텍스트 패널
    ├── status_bar.rs      # 상태바
    ├── timeline_view.rs   # 타임라인
    └── settings.rs        # 설정 화면
```

## 주요 컴포넌트

### SystemTray (tray.rs)

`tray-icon` 기반 시스템 트레이:

```rust
pub struct SystemTray {
    tray_icon: TrayIcon,
    menu: Menu,
}

impl SystemTray {
    pub fn new() -> Result<Self, CoreError> {
        let menu = Menu::new();

        menu.append(&MenuItem::with_id("show", "창 열기"))?;
        menu.append(&MenuSeparator)?;
        menu.append(&MenuItem::with_id("status", "상태: 연결됨"))?;
        menu.append(&MenuSeparator)?;
        menu.append(&MenuItem::with_id("settings", "설정..."))?;
        menu.append(&MenuItem::with_id("quit", "종료"))?;

        let icon = Self::load_icon()?;
        let tray_icon = TrayIcon::new(icon, Some("MAEKON"), Some(&menu))?;

        Ok(Self { tray_icon, menu })
    }

    pub fn set_status(&mut self, status: ConnectionStatus) {
        let text = match status {
            ConnectionStatus::Connected => "상태: 연결됨 ✅",
            ConnectionStatus::Disconnected => "상태: 연결 끊김 ❌",
            ConnectionStatus::Reconnecting => "상태: 재연결 중... 🔄",
        };
        // 메뉴 아이템 업데이트
    }

    pub fn show_indicator(&mut self, has_suggestions: bool) {
        // 새 제안이 있을 때 아이콘에 배지 표시
    }
}
```

**트레이 메뉴**:
| 항목 | 동작 |
|------|------|
| 창 열기 | 메인 윈도우 표시 |
| 상태 | 연결 상태 표시 (읽기 전용) |
| 설정... | 설정 윈도우 열기 |
| 종료 | 앱 종료 |

### DesktopNotifierImpl (notifier.rs)

`notify-rust` 기반 데스크톱 알림 (`DesktopNotifier` 포트):

```rust
pub struct DesktopNotifierImpl;

#[async_trait]
impl DesktopNotifier for DesktopNotifierImpl {
    async fn notify(&self, suggestion: &Suggestion) -> Result<(), CoreError> {
        let title = match suggestion.priority {
            Priority::Critical => "🔴 긴급 제안",
            Priority::High => "🟠 중요 제안",
            Priority::Medium => "💡 제안",
            Priority::Low => "📝 참고",
        };

        let body = Self::truncate(&suggestion.content, 100);

        Notification::new()
            .summary(title)
            .body(&body)
            .appname("MAEKON")
            .timeout(Timeout::Milliseconds(5000))
            .show()
            .map_err(|e| CoreError::Internal(e.to_string()))?;

        Ok(())
    }
}
```

### Theme (theme.rs)

다크/라이트 테마 정의:

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
        // 시스템 설정 감지
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

`iced` 기반 메인 윈도우:

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

제안 상세 팝업:

```rust
pub struct SuggestionPopup {
    suggestion: SuggestionView,
    position: Point,
}

impl SuggestionPopup {
    pub fn view(&self) -> Element<Message> {
        container(
            column![
                // 헤더: 우선순위 배지 + 시간
                row![
                    text(&self.suggestion.priority_badge),
                    horizontal_space(Length::Fill),
                    text(&self.suggestion.created_ago).size(12),
                ],
                // 제목
                text(&self.suggestion.title).size(18),
                // 본문
                scrollable(text(&self.suggestion.body)),
                // 액션 버튼
                row![
                    button("수락").on_press(Message::AcceptSuggestion(
                        self.suggestion.suggestion_id.clone()
                    )),
                    button("거절").on_press(Message::RejectSuggestion(
                        self.suggestion.suggestion_id.clone()
                    )),
                    button("닫기").on_press(Message::DismissSuggestion(
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

하단 상태바:

```rust
pub struct StatusBar {
    connection_status: ConnectionStatus,
    last_sync: Option<DateTime<Utc>>,
    pending_count: usize,
}

impl StatusBar {
    pub fn view(&self) -> Element<Message> {
        let status_indicator = match self.connection_status {
            ConnectionStatus::Connected => text("● 연결됨").color(Color::GREEN),
            ConnectionStatus::Disconnected => text("● 연결 끊김").color(Color::RED),
            ConnectionStatus::Reconnecting => text("● 재연결 중...").color(Color::YELLOW),
        };

        let sync_text = self.last_sync
            .map(|t| format!("마지막 동기화: {}", t.format("%H:%M:%S")))
            .unwrap_or_default();

        let pending_text = if self.pending_count > 0 {
            format!("대기 중: {}", self.pending_count)
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

이벤트 타임라인:

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

설정 화면:

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
            text("설정").size(24),

            // 서버 설정
            text("서버").size(18),
            text_input("서버 URL", &self.config.server.base_url),

            // 모니터링 설정
            text("모니터링").size(18),
            slider(500..=5000, self.config.monitor.poll_interval_ms, |v| {
                Message::ConfigChanged("monitor.poll_interval_ms", v)
            }),

            // 테마 설정
            text("테마").size(18),
            pick_list(&[ThemeMode::System, ThemeMode::Light, ThemeMode::Dark],
                Some(self.theme_mode.clone()),
                Message::ThemeModeChanged),

            // 자동 업데이트
            text("업데이트").size(18),
            checkbox("자동 업데이트 사용", self.config.update.enabled,
                Message::AutoUpdateToggled),
        ]
        .padding(16)
        .into()
    }
}
```

## 의존성

- `iced`: GUI 프레임워크 (0.13)
- `tray-icon`: 시스템 트레이 (0.19)
- `notify-rust`: 데스크톱 알림 (4)
- `maekon-suggestion`: 제안 프레젠터

## 플랫폼 지원

| 기능 | macOS | Windows | Linux |
|------|-------|---------|-------|
| 시스템 트레이 | ✅ | ✅ | ✅ |
| 데스크톱 알림 | ✅ | ✅ | ✅ |
| 다크 모드 감지 | ✅ | ✅ | ⚠️ (DE 의존) |
| 메인 윈도우 | ✅ | ✅ | ✅ |

## 테스트

UI 테스트는 주로 통합 테스트로 수행:

```rust
#[test]
fn test_theme_colors() {
    let dark = Theme::dark();
    let light = Theme::light();

    // 다크 테마는 밝은 텍스트
    assert!(dark.text.r > 0.5);
    // 라이트 테마는 어두운 텍스트
    assert!(light.text.r < 0.5);
}

#[test]
fn test_status_bar_display() {
    let bar = StatusBar {
        connection_status: ConnectionStatus::Connected,
        last_sync: Some(Utc::now()),
        pending_count: 5,
    };

    // 뷰 렌더링 테스트는 iced 테스트 프레임워크 사용
}
```
