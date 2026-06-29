/**
 * Strip Unicode control characters that could visually disguise untrusted text
 * (Trojan-Source / bidi-override spoofing, CVE-2021-42574).
 *
 * Applied to EVERY untrusted field rendered in the approval modals — not just
 * `args` — so a malicious or compromised backend / Codex app-server cannot make
 * the displayed process name, summary, or network host differ from what is
 * actually being approved. React escapes HTML but does NOT neutralize bidi
 * (U+202E) / zero-width spoofing, so this explicit strip is required (#6829).
 *
 * Removes, by code point: C0 controls (0x00–0x1F), DEL + C1 (0x7F–0x9F), the
 * Arabic Letter Mark (0x061C), zero-width + bidi marks/overrides (0x200B–0x200F),
 * line/paragraph separators and bidi embeddings/overrides (0x2028–0x202F), the
 * bidi isolates LRI/RLI/FSI/PDI (0x2066–0x2069), and the BOM (0xFEFF). The
 * isolates and ALM go beyond the original args-only regex to fully cover the
 * Trojan-Source bidi set. Uses numeric ranges rather than a regex literal so the
 * source carries no embedded control bytes.
 */
function isDisguisingControlChar(codePoint: number): boolean {
  return (
    codePoint <= 0x1f ||
    (codePoint >= 0x7f && codePoint <= 0x9f) ||
    codePoint === 0x061c ||
    (codePoint >= 0x200b && codePoint <= 0x200f) ||
    (codePoint >= 0x2028 && codePoint <= 0x202f) ||
    (codePoint >= 0x2066 && codePoint <= 0x2069) ||
    codePoint === 0xfeff
  )
}

/** Remove control / bidi-override / zero-width characters from a display string. */
export function sanitizeControlChars(value: string): string {
  let result = ''
  for (const char of value) {
    if (!isDisguisingControlChar(char.codePointAt(0) ?? 0)) {
      result += char
    }
  }
  return result
}
