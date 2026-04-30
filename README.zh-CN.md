<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="./assets/brand/logo-full-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="./assets/brand/logo-full-light.svg">
    <img alt="Maekon" src="./assets/brand/logo-full-light.svg" width="400">
  </picture>
</p>

<p align="center">
  <a href="./README.md">English</a> | <a href="./README.ko.md">한국어</a> | <a href="./README.ja.md">日本語</a> | <a href="./README.zh-CN.md">简体中文</a> | <a href="./README.es.md">Español</a>
</p>

# Maekon

> **将零散的桌面活动，转化为每日聚焦成果。**
> Maekon 将本地工作信号转化为实时专注时间线和可操作的建议。

一款用于 AI 辅助办公生产力的桌面客户端 -- 本地上下文采集、实时建议和内置仪表盘。基于 Rust 和 Tauri v2（WebView 外壳 + React 前端）构建，在 macOS、Windows 和 Linux 上提供原生性能。

## 30 秒安装

macOS / Linux:
```bash
curl -fsSL -o /tmp/oneshim-install.sh \
  https://raw.githubusercontent.com/pseudotop/oneshim-client/main/scripts/install.sh
bash /tmp/oneshim-install.sh
```

Windows (PowerShell):
```powershell
$tmp = Join-Path $env:TEMP "oneshim-install.ps1"
Invoke-WebRequest -UseBasicParsing `
  -Uri "https://raw.githubusercontent.com/pseudotop/oneshim-client/main/scripts/install.ps1" `
  -OutFile $tmp
powershell -ExecutionPolicy Bypass -File $tmp
```

如需版本锁定、签名验证和卸载说明：
- 英文: [`docs/install.md`](./docs/install.md)
- 韩文: [`docs/install.ko.md`](./docs/install.ko.md)

## 为什么选择 Maekon

- **将活动转化为可操作的洞察**: 在同一个地方追踪上下文、时间线、专注趋势和中断情况。
- **设备端轻量运行**: 边缘处理（增量编码、缩略图、OCR）减少传输量，保持快速响应。
- **生产级桌面技术栈**: 跨平台二进制文件、自动更新、系统托盘集成和本地 Web 仪表盘。

## 适用人群

- 希望了解自身专注模式和工作上下文的个人贡献者
- 基于丰富桌面信号构建 AI 辅助工作流工具的团队
- 需要模块化、高性能且架构边界清晰的客户端的开发者

## 2 分钟快速开始

```bash
# 1) 以独立模式运行（推荐用于安全敏感环境）
./scripts/cargo-cache.sh run -p oneshim-app -- --offline

# 2) 打开本地仪表盘
# http://localhost:10090
```

独立模式现已可用。

联网模式仅作为可选的预览路径提供。
独立模式仍是正式发布的生产级默认路径。

## 安全与隐私概览

- PII 过滤级别（关闭/基本/标准/严格）在视觉管线中应用
- 本地数据存储在 SQLite 中，并通过保留策略进行管理
- 安全报告与响应策略: [SECURITY.md](./SECURITY.md)
- 独立模式完整性基线: [docs/security/standalone-integrity-baseline.md](./docs/security/standalone-integrity-baseline.md)
- 完整性操作手册: [docs/security/integrity-runbook.md](./docs/security/integrity-runbook.md)
- 文档索引: [docs/README.md](./docs/README.md)
- 公开发布指南: [docs/guides/public-repo-launch-playbook.md](./docs/guides/public-repo-launch-playbook.md)
- 自动化指南模板: [docs/guides/automation-playbook-templates.md](./docs/guides/automation-playbook-templates.md)
- 独立模式采用手册: [docs/guides/standalone-adoption-runbook.md](./docs/guides/standalone-adoption-runbook.md)
- 5 分钟入门指南: [docs/guides/first-5-minutes.md](./docs/guides/first-5-minutes.md)
- 自动化事件契约: [docs/contracts/automation-event-contract.md](./docs/contracts/automation-event-contract.md)
- AI 提供商契约: [docs/contracts/ai-provider-contract.md](./docs/contracts/ai-provider-contract.md)

## 功能特性

### 核心功能
- **实时上下文监控**: 追踪活动窗口、系统资源和用户活动
- **边缘图像处理**: 截图捕获、增量编码、缩略图和 OCR
- **联网服务器功能（预览/可选）**: 实时建议和反馈同步可用于分阶段验证，并非默认生产路径
- **系统托盘**: 在后台运行，支持快速访问
- **自动更新**: 基于 GitHub Releases 的自动更新
- **跨平台**: 支持 macOS、Windows 和 Linux

### 本地 Web 仪表盘 (http://localhost:10090)
- **仪表盘**: 实时系统指标、CPU/内存图表、应用使用时长
- **时间线**: 截图时间线、标签过滤、灯箱查看器
- **报告**: 每周/每月活动报告、生产力分析
- **会话回放**: 带有应用分段可视化的会话回放
- **专注分析**: 专注度分析、中断追踪、本地建议
- **设置**: 配置管理、数据导出/备份

### 桌面通知
- **空闲通知**: 30 分钟以上无操作时触发
- **长时间工作通知**: 连续工作 60 分钟以上时触发
- **高负载通知**: CPU/内存超过 90% 时触发
- **专注建议**: 休息提醒、专注时间安排、上下文恢复

## 系统要求

- Rust 1.77.1 或更高版本
- macOS 10.15+ / Windows 10+ / Linux (X11/Wayland)

## 开发者快速开始（从源码构建）

### 构建

```bash
# 构建嵌入式 Web 仪表盘资源（打包/发布构建前必需）
./scripts/build-frontend.sh

# 开发构建
./scripts/cargo-cache.sh build -p oneshim-app

# 发布构建
./scripts/cargo-cache.sh build --release -p oneshim-app

# 构建桌面应用（Tauri v2，v0.1.5+）
cd src-tauri && cargo tauri build

# 启动开发服务器（前端 HMR，v0.1.5+）
cd src-tauri && cargo tauri dev
```

### 构建缓存（本地开发推荐）

```bash
# 可选：安装 sccache
brew install sccache

# 通过辅助脚本使用 Rust 构建缓存
./scripts/cargo-cache.sh check --workspace
./scripts/cargo-cache.sh test -p oneshim-web
./scripts/cargo-cache.sh build -p oneshim-app
```

如果未安装 `sccache`，该脚本将回退到普通的 `cargo`。

`cargo-cache.sh` 还会强制执行 target 目录大小限制，防止本地磁盘膨胀：
- 软限制（`ONESHIM_TARGET_SOFT_LIMIT_MB`，默认 `8192`）：清理 `target/debug/incremental`，若仍超出则清理 `target/debug/deps`
- 硬限制（`ONESHIM_TARGET_HARD_LIMIT_MB`，默认 `12288`）：额外清理 `target/debug/build`
- 自动清理开关：`ONESHIM_TARGET_AUTO_PRUNE=1`（默认）/ `0`（禁用）
- 查看当前缓存状态：`./scripts/cargo-cache.sh --status`

自定义限制示例：
```bash
ONESHIM_TARGET_SOFT_LIMIT_MB=4096 \
ONESHIM_TARGET_HARD_LIMIT_MB=6144 \
./scripts/cargo-cache.sh test --workspace
```

### 运行

```bash
# 独立模式（推荐）
./scripts/cargo-cache.sh run -p oneshim-app -- --offline
```

联网模式仅为预览版本，需要显式配置服务器/认证信息才能启用。
除非您的环境已验证联网模式，否则请使用独立模式作为默认生产路径。

在无头 CI/远程调试会话中，macOS 托盘初始化可能因缺少 WindowServer 而失败，此时可使用：
```bash
ONESHIM_DISABLE_TRAY=1 ./scripts/cargo-cache.sh run -p oneshim-app -- --offline --gui
```
仅用于非交互式冒烟/调试路径。

### 测试

```bash
# Rust 测试
./scripts/cargo-cache.sh test --workspace

# E2E 测试 —— Web 仪表盘
cd crates/oneshim-web/frontend && pnpm test:e2e

# Lint（策略：CI 中零警告）
./scripts/cargo-cache.sh clippy --workspace

# 格式检查
./scripts/cargo-cache.sh fmt --check

# 语言/国际化质量检查
./scripts/check-language.sh
# 仅检查国际化
./scripts/check-language.sh i18n
# 限定范围扫描（示例）
./scripts/check-language.sh non-english --path crates/oneshim-web/frontend/src
# 可选：严格模式（硬编码 UI 文案警告也会导致失败）
./scripts/check-language.sh --strict-i18n
```

### macOS WindowServer 冒烟测试（自托管）

如需在真实 macOS GUI 环境中验证 WindowServer 会话引导，请运行：
- 工作流: `.github/workflows/macos-windowserver-gui-smoke.yml`
- Runner 标签: `self-hosted`, `macOS`, `windowserver`

## 安装

完整安装指南：
- 英文: [`docs/install.md`](./docs/install.md)
- 韩文: [`docs/install.ko.md`](./docs/install.ko.md)

### 快速安装（终端）

macOS / Linux:
```bash
curl -fsSL -o /tmp/oneshim-install.sh \
  https://raw.githubusercontent.com/pseudotop/oneshim-client/main/scripts/install.sh
bash /tmp/oneshim-install.sh
```

Windows (PowerShell):
```powershell
$tmp = Join-Path $env:TEMP "oneshim-install.ps1"
Invoke-WebRequest -UseBasicParsing `
  -Uri "https://raw.githubusercontent.com/pseudotop/oneshim-client/main/scripts/install.ps1" `
  -OutFile $tmp
powershell -ExecutionPolicy Bypass -File $tmp
```

### 发布产物

从 [Releases](https://github.com/pseudotop/oneshim-client/releases) 下载：

Maekon 是应用显示名称。当前发布文件名会有意保留 `oneshim-*`
格式，以保持安装器、更新器和校验和兼容性。

| 平台 | 文件 |
|--------|------|
| macOS Universal (DMG 安装包) | `oneshim-macos-universal.dmg` |
| macOS Universal (PKG 安装包) | `oneshim-macos-universal.pkg` |
| macOS Universal | `oneshim-macos-universal.tar.gz` |
| macOS Apple Silicon | `oneshim-macos-arm64.tar.gz` |
| macOS Intel | `oneshim-macos-x64.tar.gz` |
| Windows x64 (zip) | `oneshim-windows-x64.zip` |
| Windows x64 (MSI) | `oneshim-app-*.msi` |
| Linux x64 (DEB 安装包) | `oneshim-*.deb` |
| Linux x64 | `oneshim-linux-x64.tar.gz` |

## 配置

### 环境变量

兼容性说明：`ONESHIM_*` 环境变量、`oneshim` CLI 命令、
`com.oneshim.client` 以及现有 config/data 路径在此发布线中仍作为稳定的
技术标识符保留。

| 变量 | 说明 | 默认值 |
|------|------|--------|
| `ONESHIM_EMAIL` | 登录邮箱（仅联网模式） | （独立模式下可选） |
| `ONESHIM_PASSWORD` | 登录密码（仅联网模式） | （独立模式下可选） |
| `ONESHIM_TESSDATA` | Tesseract 数据路径 | （可选） |
| `ONESHIM_DISABLE_TRAY` | 跳过系统托盘初始化（仅用于无头 CI/远程 GUI 冒烟测试） | `0` |
| `RUST_LOG` | 日志级别 | `info` |

### 配置文件

`~/.config/oneshim/config.json` (Linux) / `~/Library/Application Support/com.oneshim.agent/config.json` (macOS) / `%APPDATA%\oneshim\agent\config.json` (Windows):

```json
{
  "server": {
    "base_url": "https://api.oneshim.com",
    "request_timeout_ms": 30000,
    "sse_max_retry_secs": 30
  },
  "monitor": {
    "poll_interval_ms": 1000,
    "sync_interval_ms": 10000,
    "heartbeat_interval_ms": 30000
  },
  "storage": {
    "retention_days": 30,
    "max_storage_mb": 500
  },
  "vision": {
    "capture_throttle_ms": 5000,
    "thumbnail_width": 480,
    "thumbnail_height": 270,
    "ocr_enabled": false
  },
  "update": {
    "enabled": true,
    "repo_owner": "pseudotop",
    "repo_name": "oneshim-client",
    "check_interval_hours": 24,
    "include_prerelease": false
  },
  "web": {
    "enabled": true,
    "port": 10090,
    "allow_external": false
  },
  "notification": {
    "enabled": true,
    "idle_threshold_mins": 30,
    "long_session_threshold_mins": 60,
    "high_usage_threshold_percent": 90
  }
}
```

## 架构

基于 Hexagonal Architecture（Ports & Adapters）的 15 包 Cargo workspace。14 个 crate 位于 `crates/` 下，主二进制/composition root 位于 `src-tauri/`（Tauri v2，包名 `oneshim-app`）。

```
oneshim-client/
├── src-tauri/              # Tauri v2 二进制入口 + composition root
│   ├── src/
│   │   ├── main.rs         # Tauri 应用构建 + 依赖注入布线
│   │   ├── tray.rs         # 系统托盘菜单
│   │   ├── commands/       # Tauri IPC 命令
│   │   └── scheduler/      # 后台调度器
│   └── tauri.conf.json     # Tauri 配置
├── crates/
│   ├── oneshim-core/       # 领域模型 + Port trait + 错误定义 + 配置
│   ├── oneshim-network/    # HTTP/SSE/WebSocket/gRPC、压缩、认证
│   ├── oneshim-suggestion/ # 建议接收与处理
│   ├── oneshim-storage/    # SQLite 本地存储 + schema migration
│   ├── oneshim-monitor/    # 系统指标、活动窗口、活动追踪
│   ├── oneshim-vision/     # 屏幕捕获、增量编码、OCR、PII 过滤
│   ├── oneshim-web/        # 本地 Web 仪表盘（Axum REST + React）
│   ├── oneshim-automation/ # 自动化控制、策略、审计日志
│   ├── oneshim-analysis/   # LLM 分析流水线、regime 分类
│   ├── oneshim-embedding/  # 向量 embedding + INT8 量化
│   ├── oneshim-audio/      # 音频捕获 + STT 流水线
│   ├── oneshim-sandbox-worker/ # out-of-process 沙箱执行器
│   ├── oneshim-api-contracts/ # 共享 API 类型契约
│   └── oneshim-lint/       # workspace lint 工具
└── docs/
    ├── crates/             # 各 crate 详细文档
    ├── architecture/       # ADR 文档（ADR-001~ADR-019；参见 docs/architecture/ADR-*.md）
    └── migration/          # 迁移文档
```

### Crate 文档

| Crate | 职责 | 文档 |
|----------|------|------|
| oneshim-core | 领域模型、Port 接口 | [详情](./docs/crates/oneshim-core.md) |
| oneshim-network | HTTP/SSE/WebSocket/gRPC、压缩、认证 | [详情](./docs/crates/oneshim-network.md) |
| oneshim-vision | 捕获、增量编码、OCR | [详情](./docs/crates/oneshim-vision.md) |
| oneshim-monitor | 系统指标、活动窗口 | [详情](./docs/crates/oneshim-monitor.md) |
| oneshim-storage | SQLite、离线存储 | [详情](./docs/crates/oneshim-storage.md) |
| oneshim-suggestion | 建议队列、反馈 | [详情](./docs/crates/oneshim-suggestion.md) |
| oneshim-web | 本地 Web 仪表盘、REST API | [详情](./docs/crates/oneshim-web.md) |
| oneshim-automation | 自动化控制、审计日志 | [详情](./docs/crates/oneshim-automation.md) |
| oneshim-analysis | LLM 分析流水线、regime 分类 | — |
| oneshim-embedding | 向量 embedding、INT8 量化 | — |
| oneshim-audio | 音频捕获、STT 流水线 | — |
| oneshim-sandbox-worker | 沙箱自动化动作执行器 | — |
| oneshim-api-contracts | 共享 API 类型契约 | — |
| oneshim-lint | workspace lint 工具（language-check） | — |

完整文档索引: [docs/crates/README.md](./docs/crates/README.md)

贡献流程请参阅 [CONTRIBUTING.md](./CONTRIBUTING.md)。

文档语言与一致性规则定义在 [docs/DOCUMENTATION_POLICY.md](./docs/DOCUMENTATION_POLICY.md)。
韩文翻译: [README.ko.md](./README.ko.md)。
韩文配套策略文档: [docs/DOCUMENTATION_POLICY.ko.md](./docs/DOCUMENTATION_POLICY.ko.md)。

## 开发

### 代码风格

- **语言**: 文档以英文为主，关键公开指南提供韩文配套文档
- **格式化**: 使用 `cargo fmt` 默认设置
- **Lint**: 使用 `cargo clippy`，要求零警告

### 添加新功能

1. 在 `oneshim-core` 中定义 Port trait
2. 在对应的 crate 中实现 adapter
3. 在 `src-tauri/src/main.rs` 中完成依赖注入布线
4. 添加测试

### 构建安装包

macOS .app bundle:
```bash
./scripts/cargo-cache.sh install cargo-bundle
./scripts/cargo-cache.sh bundle --release -p oneshim-app
```

Windows .msi:
```bash
./scripts/cargo-cache.sh install cargo-wix
./scripts/cargo-cache.sh wix -p oneshim-app
```

## 许可证

Apache License 2.0 -- 详见 [LICENSE](./LICENSE)

- [贡献指南](./CONTRIBUTING.md)
- [行为准则](./CODE_OF_CONDUCT.md)
- [安全策略](./SECURITY.md)

## 参与贡献

1. Fork 本仓库
2. 创建功能分支 (`git checkout -b feature/amazing`)
3. 提交更改 (`git commit -m 'Add amazing feature'`)
4. 推送分支 (`git push origin feature/amazing`)
5. 创建 Pull Request
