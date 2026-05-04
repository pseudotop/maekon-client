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

> [!IMPORTANT]
> This legacy repository is in transition. Active public development, install
> scripts, releases, and security reporting for Maekon Client now live in
> [`pseudotop/maekon-client`](https://github.com/pseudotop/maekon-client).
> This repository remains available for transition history and redirect/security
> maintenance until the final archive step.

# Maekon

> **デスクトップの作業活動を、日々のフォーカス成果へ。**
> Maekonはローカルの作業シグナルをリアルタイムのフォーカスタイムラインと実行可能な提案に変換します。

AI支援によるオフィス生産性向上のためのデスクトップクライアントです。ローカルコンテキストの収集、リアルタイム提案、内蔵ダッシュボードを提供します。RustとTauri v2（Reactフロントエンドを包むWebViewシェル）で構築されており、macOS、Windows、Linuxでネイティブパフォーマンスを発揮します。

## 30秒でインストール

macOS / Linux:
```bash
curl -fsSL -o /tmp/oneshim-install.sh \
  https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/install.sh
bash /tmp/oneshim-install.sh
```

Windows (PowerShell):
```powershell
$tmp = Join-Path $env:TEMP "oneshim-install.ps1"
Invoke-WebRequest -UseBasicParsing `
  -Uri "https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/install.ps1" `
  -OutFile $tmp
powershell -ExecutionPolicy Bypass -File $tmp
```

バージョン固定、署名検証の強制、アンインストール方法：
- English: [`docs/install.md`](./docs/install.md)
- Korean: [`docs/install.ko.md`](./docs/install.ko.md)

## Maekonを選ぶ理由

- **活動を実行可能なインサイトに変換**: コンテキスト、タイムライン、フォーカスパターン、中断をひとつの場所で追跡します。
- **軽量なオンデバイス処理**: Edge処理（デルタエンコーディング、サムネイル、OCR）により転送量を削減し、高速なレスポンスを維持します。
- **本番環境レベルのデスクトップスタック**: クロスプラットフォームバイナリ、自動アップデート、システムトレイ統合、ローカルWebダッシュボードを備えています。

## 対象ユーザー

- フォーカスパターンと作業コンテキストを可視化したい個人コントリビューター
- 豊富なデスクトップシグナルを活用してAI支援ワークフローツールを構築するチーム
- モジュール式で高性能なクライアントと明確なアーキテクチャ境界を求める開発者

## 2分クイックスタート

```bash
# 1) Standaloneモードで実行（セキュリティ重視の環境に推奨）
./scripts/cargo-cache.sh run -p oneshim-app -- --offline

# 2) ローカルダッシュボードを開く
# http://localhost:10090
```

Standaloneモードは現在利用可能です。

Connectedモードはopt-inプレビューパスとしてのみ提供されています。
リリース運用環境ではStandaloneモードがデフォルトの本番パスです。

## セキュリティとプライバシーの概要

- PIIフィルタリングレベル（Off/Basic/Standard/Strict）がビジョンパイプラインに適用されます
- ローカルデータはSQLiteに保存され、保持ポリシーで管理されます
- セキュリティ報告および対応ポリシー: [SECURITY.md](./SECURITY.md)
- Standalone整合性ベースライン: [docs/security/standalone-integrity-baseline.md](./docs/security/standalone-integrity-baseline.md)
- 整合性運用ランブック: [docs/security/integrity-runbook.md](./docs/security/integrity-runbook.md)
- ドキュメントインデックス: [docs/README.md](./docs/README.md)
- 自動化プレイブックテンプレート: [docs/guides/automation-playbook-templates.md](./docs/guides/automation-playbook-templates.md)
- Standalone導入ランブック: [docs/guides/standalone-adoption-runbook.md](./docs/guides/standalone-adoption-runbook.md)
- 最初の5分ガイド: [docs/guides/first-5-minutes.md](./docs/guides/first-5-minutes.md)
- 自動化イベントコントラクト: [docs/contracts/automation-event-contract.md](./docs/contracts/automation-event-contract.md)
- AIプロバイダーコントラクト: [docs/contracts/ai-provider-contract.md](./docs/contracts/ai-provider-contract.md)

## 機能

### コア機能
- **リアルタイムコンテキストモニタリング**: アクティブウィンドウ、システムリソース、ユーザーアクティビティを追跡します
- **Edgeイメージ処理**: スクリーンショットキャプチャ、デルタエンコーディング、サムネイル、OCR
- **サーバー連携機能（プレビュー / Opt-in）**: リアルタイム提案とフィードバック同期は段階的検証用に提供されており、デフォルトの本番パスではありません
- **システムトレイ**: バックグラウンドで実行され、クイックアクセスが可能です
- **自動アップデート**: GitHub Releasesに基づく自動アップデート
- **クロスプラットフォーム**: macOS、Windows、Linuxをサポートします

### ローカルWebダッシュボード (http://localhost:10090)
- **ダッシュボード**: リアルタイムシステム指標、CPU/メモリチャート、アプリ使用時間
- **タイムライン**: スクリーンショットタイムライン、タグフィルタリング、ライトボックスビューアー
- **レポート**: 週次/月次アクティビティレポート、生産性分析
- **セッションリプレイ**: アプリセグメントの可視化を含むセッションリプレイ
- **フォーカス分析**: フォーカス分析、中断追跡、ローカル提案
- **設定**: 設定管理、データエクスポート/バックアップ

### デスクトップ通知
- **アイドル通知**: 30分以上の非アクティブ状態でトリガー
- **長時間セッション通知**: 60分以上の継続作業でトリガー
- **高使用率通知**: CPU/メモリが90%を超えるとトリガー
- **フォーカス提案**: 休憩リマインダー、フォーカスタイムのスケジューリング、コンテキスト復元

## 動作要件

- Rust 1.77.1以降
- macOS 10.15+ / Windows 10+ / Linux (X11/Wayland)

## 開発者向けクイックスタート（ソースからビルド）

### ビルド

```bash
# 埋め込みWebダッシュボードアセットのビルド（パッケージング/リリースビルド前に必須）
./scripts/build-frontend.sh

# 開発ビルド
./scripts/cargo-cache.sh build -p oneshim-app

# リリースビルド
./scripts/cargo-cache.sh build --release -p oneshim-app

# デスクトップアプリのビルド（Tauri v2、v0.1.5以降）
cd src-tauri && cargo tauri build

# フロントエンドHMR付き開発サーバーの起動（v0.1.5以降）
cd src-tauri && cargo tauri dev
```

### ビルドキャッシュ（ローカル開発に推奨）

```bash
# オプション: sccacheのインストール
brew install sccache

# キャッシュを使用するRustビルドヘルパーラッパー
./scripts/cargo-cache.sh check --workspace
./scripts/cargo-cache.sh test -p oneshim-web
./scripts/cargo-cache.sh build -p oneshim-app
```

`sccache`がインストールされていない場合、ラッパーは通常の`cargo`にフォールバックします。

`cargo-cache.sh`はローカルディスクの膨張を防ぐためにtargetサイズのガードレールも適用します:
- ソフトリミット（`ONESHIM_TARGET_SOFT_LIMIT_MB`、デフォルト`8192`）: `target/debug/incremental`を削除し、まだ大きい場合は`target/debug/deps`も削除
- ハードリミット（`ONESHIM_TARGET_HARD_LIMIT_MB`、デフォルト`12288`）: さらに`target/debug/build`も削除
- 自動削除の切り替え: `ONESHIM_TARGET_AUTO_PRUNE=1`（デフォルト） / `0`（無効化）
- 現在のキャッシュ状態の確認: `./scripts/cargo-cache.sh --status`

リミットのカスタマイズ例:
```bash
ONESHIM_TARGET_SOFT_LIMIT_MB=4096 \
ONESHIM_TARGET_HARD_LIMIT_MB=6144 \
./scripts/cargo-cache.sh test --workspace
```

### 実行

```bash
# Standaloneモード（推奨）
./scripts/cargo-cache.sh run -p oneshim-app -- --offline
```

Connectedモードはプレビュー専用であり、明示的なサーバー/認証設定が必要です。
環境でConnectedモードの検証が完了していない限り、Standaloneモードをデフォルトの本番パスとして使用してください。

macOS headless CI/リモートデバッグセッションなど、WindowServerがなくトレイの初期化が失敗する可能性がある場合:
```bash
ONESHIM_DISABLE_TRAY=1 ./scripts/cargo-cache.sh run -p oneshim-app -- --offline --gui
```
これは非対話型のsmoke/debugパスでのみ使用してください。

### テスト

```bash
# Rustテスト
./scripts/cargo-cache.sh test --workspace

# E2Eテスト — Webダッシュボード
cd crates/oneshim-web/frontend && pnpm test:e2e

# リント（ポリシー: CIで警告ゼロ）
./scripts/cargo-cache.sh clippy --workspace

# フォーマットチェック
./scripts/cargo-cache.sh fmt --check

# 言語 / i18n品質チェック
./scripts/check-language.sh
# i18nのみのチェック
./scripts/check-language.sh i18n
# スコープ限定スキャン（例）
./scripts/check-language.sh non-english --path crates/oneshim-web/frontend/src
# オプション: strictモード（ハードコードされたUIコピーの警告でも失敗）
./scripts/check-language.sh --strict-i18n
```

### macOS WindowServer Smoke（セルフホスト）

実際のmacOS GUIブートストラップをライブWindowServerセッションで検証するには:
- ワークフロー: `.github/workflows/macos-windowserver-gui-smoke.yml`
- ランナーラベル: `self-hosted`, `macOS`, `windowserver`

## インストール

インストールガイド:
- English: [`docs/install.md`](./docs/install.md)
- Korean: [`docs/install.ko.md`](./docs/install.ko.md)

### クイックインストール（ターミナル）

macOS / Linux:
```bash
curl -fsSL -o /tmp/oneshim-install.sh \
  https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/install.sh
bash /tmp/oneshim-install.sh
```

Windows (PowerShell):
```powershell
$tmp = Join-Path $env:TEMP "oneshim-install.ps1"
Invoke-WebRequest -UseBasicParsing `
  -Uri "https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/install.ps1" `
  -OutFile $tmp
powershell -ExecutionPolicy Bypass -File $tmp
```

### リリースアセット

[Releases](https://github.com/pseudotop/maekon-client/releases)からダウンロードできます:

Maekon はアプリの表示名です。現在のリリースファイル名は、インストーラー、
アップデーター、チェックサム互換性のために意図的に `oneshim-*` 形式を維持します。

| プラットフォーム | ファイル |
|--------|------|
| macOS Universal（DMGインストーラー） | `oneshim-macos-universal.dmg` |
| macOS Universal（PKGインストーラー） | `oneshim-macos-universal.pkg` |
| macOS Universal | `oneshim-macos-universal.tar.gz` |
| macOS Apple Silicon | `oneshim-macos-arm64.tar.gz` |
| macOS Intel | `oneshim-macos-x64.tar.gz` |
| Windows x64 (zip) | `oneshim-windows-x64.zip` |
| Windows x64 (MSI) | `oneshim-app-*.msi` |

Linux の公開リリースアーティファクトは、upstream の Tauri/Wry GTK
ランタイムスタックに文書化済みの `glib 0.18.x` advisory 例外が残っている
間、一時的に保留しています。Linux のソースビルドは引き続き利用できます。
詳しくは [`docs/install.md`](./docs/install.md) を参照してください。

## 設定

### 環境変数

互換性メモ: `ONESHIM_*` 環境変数、`oneshim` CLIコマンド、
`com.oneshim.client`、既存のconfig/dataパスは、このリリースラインで
安定した技術識別子として維持します。

| 変数 | 説明 | デフォルト |
|------|------|--------|
| `ONESHIM_EMAIL` | ログインメールアドレス（Connectedモード専用） | （Standaloneでは任意） |
| `ONESHIM_PASSWORD` | ログインパスワード（Connectedモード専用） | （Standaloneでは任意） |
| `ONESHIM_TESSDATA` | Tesseractデータパス | （任意） |
| `ONESHIM_DISABLE_TRAY` | システムトレイ初期化のスキップ（headless CI/リモートGUI smoke専用） | `0` |
| `RUST_LOG` | ログレベル | `info` |

### 設定ファイル

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
    "repo_name": "maekon-client",
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

## アーキテクチャ

Hexagonal Architecture（Ports & Adapters）に従う15パッケージのCargo workspaceです。14個のクレイトは`crates/`配下にあり、メインバイナリ/composition rootは`src-tauri/`（Tauri v2、パッケージ名`oneshim-app`）です。

```
maekon-client/
├── src-tauri/              # Tauri v2バイナリエントリーポイント + composition root
│   ├── src/
│   │   ├── main.rs         # Tauriアプリビルダー + DI配線
│   │   ├── tray.rs         # システムトレイメニュー
│   │   ├── commands/       # Tauri IPCコマンド
│   │   └── scheduler/      # バックグラウンドスケジューラー
│   └── tauri.conf.json     # Tauri設定
├── crates/
│   ├── oneshim-core/       # ドメインモデル + Portトレイト + エラー + 設定
│   ├── oneshim-network/    # HTTP/SSE/WebSocket/gRPC、圧縮、認証
│   ├── oneshim-suggestion/ # 提案の受信と処理
│   ├── oneshim-storage/    # SQLiteローカルストレージ + スキーママイグレーション
│   ├── oneshim-monitor/    # システム指標、アクティブウィンドウ、活動追跡
│   ├── oneshim-vision/     # 画面キャプチャ、デルタエンコーディング、OCR、PIIフィルター
│   ├── oneshim-web/        # ローカルWebダッシュボード（Axum REST + React）
│   ├── oneshim-automation/ # 自動化コントロール、ポリシー、監査ログ
│   ├── oneshim-analysis/   # LLM分析パイプライン、regime分類
│   ├── oneshim-embedding/  # ベクトル埋め込み + INT8量子化
│   ├── oneshim-audio/      # 音声キャプチャ + STTパイプライン
│   ├── oneshim-sandbox-worker/ # out-of-processサンドボックス実行器
│   ├── oneshim-api-contracts/ # 共有API型契約
│   └── oneshim-lint/       # workspace lintツール
└── docs/
    ├── crates/             # クレイトごとの詳細ドキュメント
    ├── architecture/       # ADRドキュメント（ADR-001〜ADR-004）
    └── migration/          # マイグレーションドキュメント
```

### クレイトドキュメント

| クレイト | 役割 | ドキュメント |
|----------|------|------|
| oneshim-core | ドメインモデル、Portインターフェース | [詳細](./docs/crates/oneshim-core.md) |
| oneshim-network | HTTP/SSE/WebSocket/gRPC、圧縮、認証 | [詳細](./docs/crates/oneshim-network.md) |
| oneshim-vision | キャプチャ、デルタエンコーディング、OCR | [詳細](./docs/crates/oneshim-vision.md) |
| oneshim-monitor | システム指標、アクティブウィンドウ | [詳細](./docs/crates/oneshim-monitor.md) |
| oneshim-storage | SQLite、オフラインストレージ | [詳細](./docs/crates/oneshim-storage.md) |
| oneshim-suggestion | 提案キュー、フィードバック | [詳細](./docs/crates/oneshim-suggestion.md) |
| oneshim-web | ローカルWebダッシュボード、REST API | [詳細](./docs/crates/oneshim-web.md) |
| oneshim-automation | 自動化コントロール、監査ログ | [詳細](./docs/crates/oneshim-automation.md) |
| oneshim-analysis | LLM分析パイプライン、regime分類 | — |
| oneshim-embedding | ベクトル埋め込み、INT8量子化 | — |
| oneshim-audio | 音声キャプチャ、STTパイプライン | — |
| oneshim-sandbox-worker | サンドボックス化された自動化アクション実行器 | — |
| oneshim-api-contracts | 共有API型契約 | — |
| oneshim-lint | workspace lintツール（language-check） | — |

ドキュメントの全体索引: [docs/crates/README.md](./docs/crates/README.md)

コントリビューション手順は[CONTRIBUTING.md](./CONTRIBUTING.md)を参照してください。

ドキュメントの言語および一貫性ルールは[docs/DOCUMENTATION_POLICY.md](./docs/DOCUMENTATION_POLICY.md)で定義されています。
韓国語翻訳: [README.ko.md](./README.ko.md)
韓国語ポリシードキュメント: [docs/DOCUMENTATION_POLICY.ko.md](./docs/DOCUMENTATION_POLICY.ko.md)

## 開発

### コードスタイル

- **言語**: 英語ファーストのドキュメント、主要な公開ガイドには韓国語の付属ドキュメントを提供
- **フォーマット**: `cargo fmt`のデフォルト設定
- **リント**: `cargo clippy`で警告ゼロ

### 新機能の追加

1. `oneshim-core`でPortトレイトを定義します
2. 該当するクレイトでAdapterを実装します
3. `src-tauri/src/main.rs`でDIを配線します
4. テストを追加します

### インストーラーのビルド

macOS .appバンドル:
```bash
./scripts/cargo-cache.sh install cargo-bundle
./scripts/cargo-cache.sh bundle --release -p oneshim-app
```

Windows .msi:
```bash
./scripts/cargo-cache.sh install cargo-wix
./scripts/cargo-cache.sh wix -p oneshim-app
```

## ライセンス

Apache License 2.0 — [LICENSE](./LICENSE)を参照

- [コントリビューションガイド](./CONTRIBUTING.md)
- [行動規範](./CODE_OF_CONDUCT.md)
- [セキュリティポリシー](./SECURITY.md)

## コントリビューション

1. Fork
2. 機能ブランチを作成します（`git checkout -b feature/amazing`）
3. 変更をコミットします（`git commit -m 'Add amazing feature'`）
4. ブランチをプッシュします（`git push origin feature/amazing`）
5. Pull Requestを作成します
