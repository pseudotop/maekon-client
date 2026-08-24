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
<p align="center">
  <a href="https://maekon.dev">ウェブサイト</a> · <a href="https://docs.maekon.dev">ドキュメント</a> · <a href="https://github.com/pseudotop/maekon-client/releases">リリース</a>
</p>


# Maekon

> **デスクトップの作業活動を、日々のフォーカス成果へ。**
> Maekonはローカルの作業シグナルをフォーカスタイムライン、次の行動候補、ポリシーゲート付き自動化パスに整理します。

MaekonはONESHIMなしでも独立して利用できるApache-2.0 local-firstデスクトップエージェントです。ローカルコンテキストの収集、ユーザーが確認する次の行動候補、ポリシーゲート付き自動化、内蔵ダッシュボードを提供します。RustとTauri v2（Reactフロントエンドを包むWebViewシェル）で構築されており、macOS、Windows、Linuxでネイティブパフォーマンスを発揮します。

公開チャネルは招待制Global Alpha向けの初期prereleaseです。Stable releaseや運用準備完了を示すものではありません。

## Source Buildクイックスタート

公開リポジトリは利用可能で、このsource snapshotは`v0.0.1-rc.10` release candidate向けです。対応するGitHub Releaseとassetが実在する場合にのみ公開済みとして扱ってください。GitHubの`latest` endpointはprereleaseを除外するため、release binaryの検証ではinstall guideのversion固定commandを使用してください。開発とdebug buildはローカルのsource checkoutから実行します。

```bash
git clone https://github.com/pseudotop/maekon-client.git
cd maekon-client

# Build the two bundled prerequisites the Tauri config requires before the app
# can run from source (a fresh checkout has neither yet):
#   1) the web dashboard frontend  -> crates/maekon-web/frontend/dist
#   2) the sandbox-worker sidecar   -> src-tauri/maekon-sandbox-worker-<target-triple>
(cd crates/maekon-web/frontend && pnpm install && pnpm build)
cargo build -p maekon-sandbox-worker
cp target/debug/maekon-sandbox-worker \
  "src-tauri/maekon-sandbox-worker-$(rustc -vV | sed -n 's/host: //p')"

# Run Maekon from source
./scripts/cargo-cache.sh run -p maekon-app -- --offline
```

リリースインストーラーのコマンドは下記のインストール文書に記載されています。Prereleaseのversion固定、署名検証の強制、アンインストール方法：
- English: [`docs/install.md`](./docs/install.md)
- Korean: [`docs/install.ko.md`](./docs/install.ko.md)

## Maekonを選ぶ理由

- **活動をガバナンスされたワークインサイトに整理**: コンテキスト、タイムライン、フォーカスパターン、中断、承認済み自動化パスをひとつの場所で追跡します。
- **軽量なオンデバイス処理**: Edge処理（デルタエンコーディング、サムネイル、OCR）により転送量を削減し、高速なレスポンスを維持します。
- **Global Alphaでデスクトップスタックを評価**: Prereleaseにはクロスプラットフォームソース、更新基盤、システムトレイ統合、ローカルWebダッシュボードが含まれます。利用前に対象buildとplatformを検証してください。

## 対象ユーザー

- フォーカスパターンと作業コンテキストを可視化したい個人コントリビューター
- 豊富なデスクトップシグナルを活用してAI支援ワークフローツールを構築するチーム
- モジュール式で高性能なクライアントと明確なアーキテクチャ境界を求める開発者

## 2分クイックスタート

```bash
# 1) Standaloneモードで実行（セキュリティ重視の環境に推奨）
./scripts/cargo-cache.sh run -p maekon-app -- --offline

# 2) ローカルダッシュボードを開く
# http://localhost:10090
```

Standaloneモードは現在利用可能です。

Connectedモードはopt-inプレビューパスとしてのみ提供されています。
Global AlphaではStandaloneモードが現在のデフォルト評価パスです。

## セキュリティとプライバシーの概要

- PIIフィルタリングレベル（Off/Basic/Standard/Strict）がビジョンパイプラインに適用されます
- ローカルデータはSQLiteに保存され、保持ポリシーで管理されます
- 自動化にはポリシー検証、サンドボックスプロファイル、ローカル監査ログが必要です
- セキュリティ報告および対応ポリシー: [SECURITY.md](./SECURITY.md)
- Alphaフィードバック・プライバシー要求・参加撤回（現在の受付状態）: [maekon.dev/alpha-feedback](https://maekon.dev/alpha-feedback)
- Standalone整合性ベースライン: [docs/security/standalone-integrity-baseline.md](./docs/security/standalone-integrity-baseline.md)
- 整合性運用ランブック: [docs/security/integrity-runbook.md](./docs/security/integrity-runbook.md)
- ドキュメントインデックス: [docs/README.md](./docs/README.md)
- リリースチェックリスト: [docs/release-checklist.md](./docs/release-checklist.md)
- 自動化プレイブックテンプレート: [docs/guides/automation-playbook-templates.md](./docs/guides/automation-playbook-templates.md)
- Standalone導入ランブック: [docs/guides/standalone-adoption-runbook.md](./docs/guides/standalone-adoption-runbook.md)
- 最初の5分ガイド: [docs/guides/first-5-minutes.md](./docs/guides/first-5-minutes.md)
- 自動化イベントコントラクト: [docs/contracts/automation-event-contract.md](./docs/contracts/automation-event-contract.md)
- AIプロバイダーコントラクト: [docs/contracts/ai-provider-contract.md](./docs/contracts/ai-provider-contract.md)

### ソースで直接検証する

上記のプライバシーに関する主張はマーケティング文言ではありません — 各主張は、このリポジトリで直接読み、ビルドし、テストできるコードに対応しています。READMEとソースは同一の検証済みツリーから一緒にexportされるため、この表は常にすぐ隣にあるコードを説明しています。

| 主張 | 検証場所 |
|---|---|
| 除外/機密アプリはアップロード時ではなく**キャプチャ時点で**除外されます | [`crates/maekon-vision/src/privacy/detection.rs`](./crates/maekon-vision/src/privacy/detection.rs) (`should_exclude_by_policy`)、キャプチャゲート配線: [`src-tauri/src/scheduler/loops/monitor_phases.rs`](./src-tauri/src/scheduler/loops/monitor_phases.rs) |
| Egress policyの対象として宣言されたruntime pathはローカル台帳に記録され、アプリ内で閲覧できます (Privacy → Egress ledger) | [`src-tauri/src/scheduler/egress_policy.rs`](./src-tauri/src/scheduler/egress_policy.rs) + リーダールート: [`crates/maekon-web/src/routes.rs`](./crates/maekon-web/src/routes.rs) |
| メモリグラフが蓄積したユーザーに関する信念(claims)は閲覧・ワンクリック撤回が可能です (Privacy → Claims) | claimsルート: [`crates/maekon-web/src/routes.rs`](./crates/maekon-web/src/routes.rs) |
| 同意はfail-closedです: 有効な同意がなければキャプチャしません | [`crates/maekon-core/src/consent.rs`](./crates/maekon-core/src/consent.rs) |
| Vision pipelineの対象pathは、文書化された保存またはegress stepの前に設定済みPII filterを適用します | [`crates/maekon-vision/src/privacy/`](./crates/maekon-vision/src/privacy/) |
| サポート対象の自動化実行pathはpolicy・sandbox・audit componentを通るよう設計されています | [`crates/maekon-automation/src/`](./crates/maekon-automation/src/) |

### ソース同期ポリシー

このリポジトリはMaekon内部ソースの**検証済みスナップショットexport**です。スナップショットはリリース単位で検証後にexportされ — リリースタグが検証済み状態を示し、リポジトリは内部のすべてのコミットではなくリリースを追跡します。READMEとコードは常に同じツリーから生成されるため、上記の主張とコードのリンクは今読んでいるチェックアウトを正確に指しています。

## 機能

### コア機能
- **リアルタイムコンテキストモニタリング**: アクティブウィンドウ、システムリソース、ユーザーアクティビティを追跡します
- **Edgeイメージ処理**: スクリーンショットキャプチャ、デルタエンコーディング、サムネイル、OCR
- **ポリシーゲート付き自動化**: 承認済みアクションをポリシー検査、サンドボックス隔離、監査ログ経由で実行します
- **サーバー連携機能（プレビュー / Opt-in）**: 確認可能な次の行動候補とフィードバック同期は段階的検証用に提供されており、デフォルトのStandaloneパスではありません
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

- Rust 1.88.0以降
- macOS 10.15+ / Windows 10+ / Linux (X11/Wayland)

## 開発者向けクイックスタート（ソースからビルド）

### ビルド

```bash
# 埋め込みWebダッシュボードアセットのビルド（パッケージング/リリースビルド前に必須）
./scripts/build-frontend.sh

# 開発ビルド
./scripts/cargo-cache.sh build -p maekon-app

# リリースビルド
./scripts/cargo-cache.sh build --release -p maekon-app

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
./scripts/cargo-cache.sh test -p maekon-web
./scripts/cargo-cache.sh build -p maekon-app
```

`sccache`がインストールされていない場合、ラッパーは通常の`cargo`にフォールバックします。

`cargo-cache.sh`はローカルディスクの膨張を防ぐためにtargetサイズのガードレールも適用します:
- ソフトリミット（`MAEKON_TARGET_SOFT_LIMIT_MB`、デフォルト`8192`）: `target/debug/incremental`を削除し、まだ大きい場合は`target/debug/deps`も削除
- ハードリミット（`MAEKON_TARGET_HARD_LIMIT_MB`、デフォルト`12288`）: さらに`target/debug/build`も削除
- 自動削除の切り替え: `MAEKON_TARGET_AUTO_PRUNE=1`（デフォルト） / `0`（無効化）
- 現在のキャッシュ状態の確認: `./scripts/cargo-cache.sh --status`

リミットのカスタマイズ例:
```bash
MAEKON_TARGET_SOFT_LIMIT_MB=4096 \
MAEKON_TARGET_HARD_LIMIT_MB=6144 \
./scripts/cargo-cache.sh test --workspace
```

### 実行

```bash
# Standaloneモード（推奨）
./scripts/cargo-cache.sh run -p maekon-app -- --offline
```

Connectedモードはプレビュー専用であり、明示的なサーバー/認証設定が必要です。
環境でConnectedモードの検証が完了していない限り、StandaloneモードをGlobal Alphaのデフォルトパスとして使用してください。

macOS headless CI/リモートデバッグセッションなど、WindowServerがなくトレイの初期化が失敗する可能性がある場合:
```bash
MAEKON_DISABLE_TRAY=1 ./scripts/cargo-cache.sh run -p maekon-app -- --offline --gui
```
これは非対話型のsmoke/debugパスでのみ使用してください。

### テスト

```bash
# Rustテスト
./scripts/cargo-cache.sh test --workspace

# E2Eテスト — Webダッシュボード
cd crates/maekon-web/frontend && pnpm test:e2e

# リント（ポリシー: CIで警告ゼロ）
./scripts/cargo-cache.sh clippy --workspace

# フォーマットチェック
./scripts/cargo-cache.sh fmt --check

# 言語 / i18n品質チェック
./scripts/check-language.sh
# i18nのみのチェック
./scripts/check-language.sh i18n
# スコープ限定スキャン（例）
./scripts/check-language.sh non-english --path crates/maekon-web/frontend/src
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

対応するGitHub Releaseにassetが公開された後で、以下の`v0.0.1-rc.10`固定commandを実行してください。

macOS / Linux:
```bash
curl -fsSL -o /tmp/maekon-install.sh \
  https://raw.githubusercontent.com/pseudotop/maekon-client/v0.0.1-rc.10/scripts/install.sh
MAEKON_VERSION=v0.0.1-rc.10 bash /tmp/maekon-install.sh --require-signature
```

Windows (PowerShell):
```powershell
$tmp = Join-Path $env:TEMP "maekon-install.ps1"
Invoke-WebRequest -UseBasicParsing `
  -Uri "https://raw.githubusercontent.com/pseudotop/maekon-client/v0.0.1-rc.10/scripts/install.ps1" `
  -OutFile $tmp
powershell -ExecutionPolicy Bypass -File $tmp -Version v0.0.1-rc.10 -RequireSignature
```

### リリースアセット

[Releases](https://github.com/pseudotop/maekon-client/releases)からダウンロードできます:

Maekon はアプリの表示名です。現在のリリースファイル名は、インストーラー、
アップデーター、チェックサム互換性のために意図的に `maekon-*` 形式を維持します。

| プラットフォーム | ファイル |
|--------|------|
| macOS Universal（DMGインストーラー） | `maekon-macos-universal.dmg` |
| macOS Universal（PKGインストーラー） | `maekon-macos-universal.pkg` |
| macOS Universal | `maekon-macos-universal.tar.gz` |
| macOS Apple Silicon | `maekon-macos-arm64.tar.gz` |
| macOS Intel | `maekon-macos-x64.tar.gz` |
| Windows x64 (zip) | `maekon-windows-x64.zip` |
| Windows x64 (MSI) | `maekon-app-*.msi` |
| Linux x64（DEBパッケージ） | `maekon-*.deb` |
| Linux x64 | `maekon-linux-x64.tar.gz` |

## 設定

### 環境変数

互換性メモ: `MAEKON_*` 環境変数、`maekon` CLIコマンド、
`com.maekon.app`、既存のconfig/dataパスは、このリリースラインで
安定した技術識別子として維持します。

| 変数 | 説明 | デフォルト |
|------|------|--------|
| `MAEKON_TESSDATA` | Tesseractデータパス | （任意） |
| `MAEKON_DISABLE_TRAY` | システムトレイ初期化のスキップ（headless CI/リモートGUI smoke専用） | `0` |
| `RUST_LOG` | ログレベル | `info` |

ログイン資格情報は環境変数から読み込みません。**設定 → 一般 → アカウント**
からサインインしてください（`--features server` 付きのビルドが必要）。
サーバーURLは **設定 → 詳細設定 → ネットワークとサーバー** で設定します。

### 設定ファイル

`~/.config/maekon/config.json` (Linux) / `~/Library/Application Support/com.maekon.app/config.json` (macOS) / `%APPDATA%\maekon\agent\config.json` (Windows):

```json
{
  "server": {
    "base_url": "https://api.example.com",
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

Hexagonal Architecture（Ports & Adapters）に従う15パッケージのCargo workspaceです。14個のクレイトは`crates/`配下にあり、メインバイナリ/composition rootは`src-tauri/`（Tauri v2、パッケージ名`maekon-app`）です。

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
│   ├── maekon-core/       # ドメインモデル + Portトレイト + エラー + 設定
│   ├── maekon-network/    # HTTP/SSE/WebSocket/gRPC、圧縮、認証
│   ├── maekon-suggestion/ # 提案の受信と処理
│   ├── maekon-storage/    # SQLiteローカルストレージ + スキーママイグレーション
│   ├── maekon-monitor/    # システム指標、アクティブウィンドウ、活動追跡
│   ├── maekon-vision/     # 画面キャプチャ、デルタエンコーディング、OCR、PIIフィルター
│   ├── maekon-web/        # ローカルWebダッシュボード（Axum REST + React）
│   ├── maekon-automation/ # 自動化コントロール、ポリシー、監査ログ
│   ├── maekon-analysis/   # LLM分析パイプライン、regime分類
│   ├── maekon-embedding/  # ベクトル埋め込み + INT8量子化
│   ├── maekon-audio/      # 音声キャプチャ + STTパイプライン
│   ├── maekon-sandbox-worker/ # out-of-processサンドボックス実行器
│   ├── maekon-api-contracts/ # 共有API型契約
│   └── maekon-lint/       # workspace lintツール
└── docs/
    ├── crates/             # クレイトごとの詳細ドキュメント
    ├── architecture/       # ADRドキュメント（ADR-001〜ADR-004）
    └── migration/          # マイグレーションドキュメント
```

### クレイトドキュメント

| クレイト | 役割 | ドキュメント |
|----------|------|------|
| maekon-core | ドメインモデル、Portインターフェース | [詳細](./docs/crates/maekon-core.md) |
| maekon-network | HTTP/SSE/WebSocket/gRPC、圧縮、認証 | [詳細](./docs/crates/maekon-network.md) |
| maekon-vision | キャプチャ、デルタエンコーディング、OCR | [詳細](./docs/crates/maekon-vision.md) |
| maekon-monitor | システム指標、アクティブウィンドウ | [詳細](./docs/crates/maekon-monitor.md) |
| maekon-storage | SQLite、オフラインストレージ | [詳細](./docs/crates/maekon-storage.md) |
| maekon-suggestion | 提案キュー、フィードバック | [詳細](./docs/crates/maekon-suggestion.md) |
| maekon-web | ローカルWebダッシュボード、REST API | [詳細](./docs/crates/maekon-web.md) |
| maekon-automation | 自動化コントロール、監査ログ | [詳細](./docs/crates/maekon-automation.md) |
| maekon-analysis | LLM分析パイプライン、regime分類 | — |
| maekon-embedding | ベクトル埋め込み、INT8量子化 | — |
| maekon-audio | 音声キャプチャ、STTパイプライン | — |
| maekon-sandbox-worker | サンドボックス化された自動化アクション実行器 | — |
| maekon-api-contracts | 共有API型契約 | — |
| maekon-lint | workspace lintツール（language-check） | — |

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

1. `maekon-core`でPortトレイトを定義します
2. 該当するクレイトでAdapterを実装します
3. `src-tauri/src/main.rs`でDIを配線します
4. テストを追加します

### インストーラーのビルド

macOS .appバンドル:
```bash
./scripts/cargo-cache.sh install cargo-bundle
./scripts/cargo-cache.sh bundle --release -p maekon-app
```

Windows .msi:
```bash
./scripts/cargo-cache.sh install cargo-wix
./scripts/cargo-cache.sh wix -p maekon-app
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
