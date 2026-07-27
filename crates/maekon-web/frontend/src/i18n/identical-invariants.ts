/**
 * #8096 — Intentional i18n invariants allowlist.
 *
 * These flattened `en.json` leaf keys have Japanese (`ja.json`) values that are
 * *deliberately* byte-identical to English. They are brand names, protocol /
 * technical acronyms, units, proper nouns, placeholder URLs/examples, or
 * language endonyms — terms that a professional Japanese desktop UI leaves in
 * their original form. The completeness guard
 * (`__tests__/ja-localization-completeness.test.ts`) treats any *other*
 * identical `ja` leaf as accidental English fallback and fails.
 *
 * Keep this list narrow: single tokens / brand / protocol only. Full sentences
 * and multi-word phrases must be translated, never allowlisted.
 *
 * Note: these keys are cross-locale invariants — the same terms stay in their
 * canonical form in ko/zh-CN/es as well — so a future per-locale guard can
 * reuse this list as the shared invariant baseline.
 */

export interface InvariantEntry {
  /** Dotted i18n key path (matches a flattened `en.json` leaf). */
  readonly key: string
  /** One-line rationale for why the value stays identical across locales. */
  readonly rationale: string
}

export const IDENTICAL_INVARIANTS: readonly InvariantEntry[] = [
  // --- Brand / product name ---
  { key: 'trayDashboard.title', rationale: 'Brand name "Maekon" — never localized.' },

  // --- Language endonyms (shown in their own script in every locale) ---
  { key: 'settings.english', rationale: 'Language endonym "English" — displayed natively in all locales.' },
  { key: 'settings.korean', rationale: 'Language endonym "한국어" — displayed natively in all locales.' },

  // --- Protocol / technical acronyms & units ---
  { key: 'dashboard.cpu', rationale: 'Hardware acronym "CPU" — universal, unchanged in Japanese UI.' },
  { key: 'settings.bugReportCpu', rationale: 'Hardware acronym "CPU".' },
  { key: 'metrics.cpuPercent', rationale: 'Acronym + unit "CPU %" — compact metric column label.' },
  { key: 'metrics.selfCpu', rationale: 'Hardware acronym "CPU" — agent self-usage row label (#8082).' },
  { key: 'settings.dbSize', rationale: 'Acronym "DB" (database) — used verbatim in Japanese.' },
  { key: 'trackingPanel.ocr', rationale: 'Acronym "OCR" — technical term kept in Japanese.' },
  { key: 'processList.pid', rationale: 'Acronym label "PID:" (process id).' },
  { key: 'reauth.pinLabel', rationale: 'Acronym "PIN" — used verbatim in Japanese.' },
  { key: 'syncTab.deviceId', rationale: 'Acronym "ID" — used verbatim in Japanese.' },
  { key: 'settings.ai.tierOauth', rationale: 'Protocol name "OAuth" — proper noun.' },
  { key: 'settings.ai.tierAws', rationale: 'Brand/proper noun "AWS".' },

  // --- Microsoft API proper noun ---
  { key: 'onboarding.step2WindowsAccessibility', rationale: 'Windows API proper noun "UI Automation".' },
  { key: 'settings.permissionWindowsAccessibilityLabel', rationale: 'Windows API proper noun "UI Automation".' },

  // --- Universal acknowledgment token ---
  { key: 'coaching.ok', rationale: 'Universal acknowledgment token "OK" — used verbatim in Japanese.' },

  // --- Numeric threshold symbols (no translatable text) ---
  { key: 'coaching.habits.missed', rationale: 'Numeric threshold symbol "<50%".' },
  { key: 'coaching.habits.partial', rationale: 'Numeric threshold symbol ">50%".' },

  // --- Placeholder examples: app brand names & example URLs ---
  {
    key: 'settings.excludedAppsPlaceholder',
    rationale: 'App brand-name examples ("1Password, Discord, Slack") in a placeholder.',
  },
  { key: 'settingsAutomation.endpointPlaceholderLlm', rationale: 'Example placeholder URL.' },
  { key: 'settingsAutomation.endpointPlaceholderOcr', rationale: 'Example placeholder URL.' },

  // --- Interpolation + acronym only (no translatable words) ---
  {
    key: 'settingsAutomation.requirementCli',
    rationale: '"{{tool}} CLI" — interpolated tool name + acronym; nothing translatable.',
  },
] as const

/** Set of allowlisted keys for O(1) membership checks in the guard test. */
export const INVARIANT_KEYS: ReadonlySet<string> = new Set(IDENTICAL_INVARIANTS.map((entry) => entry.key))
