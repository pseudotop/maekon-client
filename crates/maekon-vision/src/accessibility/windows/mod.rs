//! Windows UIAutomation accessibility extractor.
//!
//! Extracts the currently focused UI element via the COM-based IUIAutomation API.
//! The implementation follows the same circuit breaker and PII gating patterns
//! used by the macOS native extractor.
//!
//! COM call sequence (focused element):
//!   1. `CoInitializeEx(COINIT_MULTITHREADED)` — initialize COM on the thread
//!   2. `CoCreateInstance(CUIAutomation)` — obtain `IUIAutomation` interface
//!   3. `IUIAutomation::GetFocusedElement()` — get the focused `IUIAutomationElement`
//!   4. Extract properties:
//!      - `CurrentControlType()` — mapped to a role string
//!      - `CurrentName()` — accessibility label
//!      - `CurrentBoundingRectangle()` — screen position/size
//!      - `GetCurrentPropertyValue(UIA_ValueValuePropertyId)` — text value
//!   5. COM objects are released automatically via `Drop` (type-safe wrappers)
//!
//! Tree traversal (window elements) uses CacheRequest for bulk property
//! fetching. Instead of 3 cross-process COM calls per element (ControlType,
//! Name, BoundingRectangle), a CacheRequest pre-fetches all three properties
//! in a single cross-process call per subtree level via BuildCache walker
//! methods. Falls back to per-property fetching if CacheRequest creation fails.
//!
//! ## Migration Note (vtable → type-safe COM)
//!
//! This module was migrated from raw vtable COM calls (`windows-sys` +
//! manual vtable index constants) to type-safe COM via the `windows` crate
//! (0.62). The `windows` crate provides proper COM interface wrappers
//! (`IUIAutomation`, `IUIAutomationElement`, `IUIAutomationTreeWalker`,
//! `IUIAutomationCacheRequest`) that eliminate the need for hard-coded
//! vtable offsets and `transmute` calls.
//!
//! `windows-sys` is retained for `IsDebuggerPresent` and `CoInitializeEx` /
//! `CoUninitialize` (the `windows` crate's `CoInitializeEx` returns
//! `HRESULT` which requires different error handling).
//!
//! ## Module layout (ADR-013)
//!
//! | File          | Contents                                              |
//! |---------------|-------------------------------------------------------|
//! | `mod.rs`      | `WindowsUiaAccessibility` struct, circuit breaker,    |
//! |               | PII filter, `AccessibilityExtractor` trait impl       |
//! | `com.rs`      | COM operations: focused-element + tree extraction     |
//! | `roles.rs`    | `control_type_to_role()` + UIA constant table         |
//! | `types.rs`    | `RawFocusedElement` intermediate struct               |
//! | `tests.rs`    | Unit + async tests (Windows-only)                     |

#[cfg(target_os = "windows")]
mod com;

#[cfg(target_os = "windows")]
mod roles;

#[cfg(target_os = "windows")]
mod types;

#[cfg(target_os = "windows")]
mod inner {
    use async_trait::async_trait;
    use tracing::{debug, warn};

    use maekon_core::circuit_breaker::CircuitBreaker;
    use maekon_core::config::PiiFilterLevel;
    use maekon_core::error::CoreError;
    use maekon_core::models::focused_element::{AccessibilityElement, FocusedElementInfo};
    use maekon_core::ports::accessibility::AccessibilityExtractor;

    use super::com;
    use super::types::RawFocusedElement;

    // ── Circuit breaker ───────────────────────────────────────────────
    //
    // Delegates to the shared `maekon_core::circuit_breaker::CircuitBreaker`
    // (#7720 E6 consolidation). This module previously hand-rolled its own
    // `AtomicU32` state machine, which had drifted to a version *missing* the
    // `compare_exchange` retry-slot claim (#6007 finding 17) that the shared
    // struct carries — without it, two concurrent callers that both observe
    // the counter at the same retry-interval boundary would both pass the
    // gate and both issue a COM/UIA call.

    const CIRCUIT_BREAKER_THRESHOLD: u32 = 3;
    /// After threshold is hit, retry once every N calls (~30s at 3s poll).
    const CIRCUIT_BREAKER_RETRY_INTERVAL: u32 = 10;

    /// Consecutive COM/UIA failures before the circuit breaker opens.
    static BREAKER: CircuitBreaker =
        CircuitBreaker::new(CIRCUIT_BREAKER_THRESHOLD, CIRCUIT_BREAKER_RETRY_INTERVAL);

    // ── Public extractor struct ───────────────────────────────────────

    pub struct WindowsUiaAccessibility;

    impl Default for WindowsUiaAccessibility {
        fn default() -> Self {
            Self
        }
    }

    impl WindowsUiaAccessibility {
        pub fn new() -> Self {
            Self
        }

        /// Check if a debugger is attached to the current process.
        /// When detected, text extraction is skipped to prevent memory
        /// inspection of sensitive accessibility data.
        fn is_debugger_attached() -> bool {
            // SAFETY: `IsDebuggerPresent` is a Win32 API that takes no arguments and
            // returns a `BOOL`; it only reads the `BeingDebugged` flag in the current
            // process's PEB and involves no pointers, so the call is always sound.
            unsafe { windows_sys::Win32::System::Diagnostics::Debug::IsDebuggerPresent() != 0 }
        }

        // ── Circuit breaker helpers ───────────────────────────────────

        fn circuit_allows() -> bool {
            BREAKER.should_proceed()
        }

        fn record_success() {
            BREAKER.record_success();
        }

        fn record_failure() {
            BREAKER.record_failure();
            if BREAKER.failure_count() == CIRCUIT_BREAKER_THRESHOLD {
                warn!(
                    "WindowsUiaAccessibility: circuit breaker tripped after \
                     {CIRCUIT_BREAKER_THRESHOLD} consecutive failures"
                );
            }
        }

        #[cfg(test)]
        pub(super) fn reset_circuit_for_test() {
            BREAKER.record_success();
        }

        #[cfg(test)]
        pub(super) fn set_circuit_failures_for_test(failures: u32) {
            BREAKER.set_failure_count(failures);
        }

        #[cfg(test)]
        pub(super) fn record_failure_for_test() {
            Self::record_failure();
        }

        #[cfg(test)]
        pub(super) fn circuit_allows_for_test() -> bool {
            Self::circuit_allows()
        }

        // ── PII level gating ──────────────────────────────────────────

        /// Apply PII-level filtering to raw extracted data.
        ///
        /// Level semantics:
        /// - `Strict`: role + position only
        /// - `Standard`: + label + value_length (no text content)
        /// - `Basic`: + sanitized text (PII patterns masked)
        /// - `Off`: full text (requires explicit consent)
        fn filter_by_level(raw: RawFocusedElement, level: PiiFilterLevel) -> FocusedElementInfo {
            // Windows resolves its label from the UIA `name`; the shared
            // per-level redaction is the single source of truth
            // (`crate::accessibility::pii_filter`, #5120).
            //
            // Strict exposes no label, so do NOT materialize it there: `name` is
            // a `Zeroizing<String>` and a `.to_string()` copy would be dropped
            // un-zeroed. Resolve it only when the level uses it (#5131 follow-up).
            let label = if level == PiiFilterLevel::Strict {
                None
            } else {
                raw.name.as_deref().map(|s| s.to_string())
            };
            super::super::pii_filter::apply_pii_level(
                raw.role,
                label,
                // `raw.value` is `Option<Zeroizing<String>>`; deref to `&str`.
                raw.value.as_deref().map(String::as_str),
                raw.position,
                level,
            )
            // raw.name and raw.value (Zeroizing<String>) are dropped here,
            // zeroing memory automatically.
        }
    }

    #[async_trait]
    impl AccessibilityExtractor for WindowsUiaAccessibility {
        async fn extract_focused_element(
            &self,
            pii_level: PiiFilterLevel,
            has_full_text_consent: bool,
        ) -> Result<Option<FocusedElementInfo>, CoreError> {
            if Self::is_debugger_attached() {
                warn!("Debugger detected; skipping accessibility text extraction");
                return Ok(None);
            }
            if !Self::circuit_allows() {
                debug!("WindowsUiaAccessibility: circuit breaker open");
                return Ok(None);
            }

            let effective_level = if pii_level == PiiFilterLevel::Off && !has_full_text_consent {
                debug!(
                    "PII Off requested but full_text_extraction consent missing; \
                     falling back to Standard"
                );
                PiiFilterLevel::Standard
            } else {
                pii_level
            };

            let result = tokio::task::spawn_blocking(com::extract_via_uia)
                .await
                .map_err(|e| CoreError::Internal {
                    code: maekon_core::error_codes::InternalCode::Generic,
                    message: format!("UIA blocking task failed: {e}"),
                })?;

            match result {
                Some(raw) => {
                    Self::record_success();
                    let filtered = Self::filter_by_level(raw, effective_level);
                    debug!(role = %filtered.role, "UIA focused element extracted");
                    Ok(Some(filtered))
                }
                None => {
                    Self::record_failure();
                    Ok(None)
                }
            }
        }

        async fn extract_window_elements(
            &self,
            max_depth: u32,
            max_elements: usize,
            pii_level: PiiFilterLevel,
            has_full_text_consent: bool,
        ) -> Result<Vec<AccessibilityElement>, CoreError> {
            if Self::is_debugger_attached() {
                return Ok(Vec::new());
            }
            if !Self::circuit_allows() {
                return Ok(Vec::new());
            }

            let effective_level = if pii_level == PiiFilterLevel::Off && !has_full_text_consent {
                PiiFilterLevel::Standard
            } else {
                pii_level
            };

            let result = tokio::task::spawn_blocking(move || {
                com::extract_tree_via_uia(max_depth, max_elements)
            })
            .await
            .map_err(|e| CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: format!("UIA tree traversal task failed: {e}"),
            })?;

            if result.is_empty() {
                Self::record_failure();
            } else {
                Self::record_success();
            }

            Ok(result
                .into_iter()
                .map(|(role, name, bounds)| {
                    let label = if effective_level == PiiFilterLevel::Strict {
                        String::new()
                    } else {
                        name.unwrap_or_default()
                    };
                    AccessibilityElement {
                        role,
                        // Mask the accessibility name at the configured level
                        // (review4 V15); Strict already yields an empty label, Off
                        // is an identity pass.
                        label: crate::privacy::sanitize_title_with_level(&label, effective_level),
                        bounds,
                    }
                })
                .collect())
        }

        fn has_permission(&self) -> bool {
            // Windows UIAutomation does not require special permissions
            true
        }

        fn name(&self) -> &str {
            "windows-uia-accessibility"
        }
    }
}

#[cfg(target_os = "windows")]
pub use inner::WindowsUiaAccessibility;

#[cfg(test)]
#[cfg(target_os = "windows")]
mod tests {
    include!("tests.rs");
}
