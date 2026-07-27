[English](./product-terminology.md) | [한국어](./product-terminology.ko.md)

# Product Terminology and High-risk Copy

This guide is the source of truth for Maekon user-facing terminology. It covers
privacy, consent, deletion, provider or AI egress, integrations, automation,
suggestions, and update recovery copy. The Korean wording is the canonical
product-language decision; every safety-relevant change must retain equivalent
meaning in all five supported locales.

## Writing rules

1. Lead with the user action and the observable result. Avoid legal or internal
   implementation jargon when a plain product term is accurate.
2. Distinguish stopping future collection from deleting existing data.
3. A destructive confirmation must state the target, device or storage scope,
   consequence, and any files that remain outside Maekon.
4. Never turn a best-effort request into a completed-deletion claim. The current
   consent-withdrawal command returns success only after local database deletion
   and frame-file deletion complete; failures remain visible to the user.
5. Show a localized recovery instruction first. Raw provider, HTTP, parser, or
   operating-system errors belong in a collapsed “Technical details” disclosure.
6. Route and navigation labels must describe the screen that actually exists.

## Preferred terms

| Concept | English | Korean canonical | Japanese | Simplified Chinese | Spanish |
| --- | --- | --- | --- | --- | --- |
| Withdraw consent and delete app-managed data | Withdraw consent and delete data | 동의 철회 및 데이터 삭제 | 同意の撤回とデータ削除 | 撤回同意并删除数据 | Retirar el consentimiento y eliminar datos |
| Stop future collection only | Turn monitoring off | 모니터링 끄기 | モニタリングをオフ | 关闭监控 | Desactivar la monitorización |
| Delete all app-managed data | Delete all data | 모든 데이터 삭제 | すべてのデータを削除 | 删除所有数据 | Eliminar todos los datos |
| Select release stream | Update channel | 업데이트 채널 | アップデートチャネル | 更新频道 | Canal de actualización |
| Expand raw failure text | Technical details | 기술 세부 정보 | 技術的な詳細 | 技术详情 | Detalles técnicos |
| Data leaving the device | External transfer | 외부 전송 | 外部送信 | 外部传输 | Transferencia externa |
| Service handling externally sent data | Provider | 제공자 | プロバイダー | 提供商 | Proveedor |

Use `delete` / `삭제` for the product action. Reserve `erasure` and legal article
references for policy, audit, and engineering documents where that distinction
is necessary.

## High-risk inventory and decisions

| Surface | Resource or route | Risk found | Decision |
| --- | --- | --- | --- |
| Consent withdrawal | `privacy.consent.withdraw.*` | Korean “소거” was unnatural and the action scope was implicit | Name deletion directly, state that Maekon-managed data on this device is deleted, and keep exported or backed-up files as an explicit exception |
| Update navigation | `sidebar.updateChannel`, `/updates/channel` | “Update history” promised a history view while the route selects a channel | Name the route “Update channel” in every locale |
| Update failure | `updates.statusCheckFailed`, `updates.actionFailed` | Backend HTTP/parser text could become the primary user message | Show localized recovery copy; retain raw text only in collapsed technical details |
| Install action | `updates.readyToInstallMsg`, `updates.installNow` | Korean, Chinese, and Spanish resources retained English fallbacks | Localize the two actions in all supported locales |
| Provider and AI egress | `privacy.consent.microphone.*`, `privacy.consent.unredactedExternalOcr.*` | Safety meaning can drift when copy changes | Keep provider, payload, opt-in, and already-sent-data scope explicit; require five-locale key parity tests |

## Verification contract

- `product-copy-parity.test.ts` locks the five-locale deletion, external-file
  exception, update-channel, recovery-copy, and install-action decisions.
- `UpdatePanel.test.tsx` verifies that a raw updater failure is collapsed behind
  localized recovery copy.
- For a copy change that can alter layout, review the consent confirmation and
  update status/channel surfaces at 1024×768 and 1280×800 before merge.
- Keep UI strings in locale resources; do not add locale-specific literals to
  components or selectors.

Changes to destructive scope or legal meaning require privacy/legal review in
addition to product review. Translation-only cleanup that preserves this
contract still requires the five-locale parity checks.
