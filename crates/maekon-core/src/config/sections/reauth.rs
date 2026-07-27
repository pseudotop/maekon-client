//! Capture-history viewing re-authentication (re-auth) settings.
//!
//! Controls whether OS biometrics (Touch ID / Windows Hello) or an app PIN
//! re-auth is required before entering the captured screenshot timeline /
//! replay views. Since this is a privacy-first product, the **default is
//! enabled (on)** — it protects the capture history from a physical accessor
//! unless the user explicitly turns it off.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Default idle expiry (seconds). Re-auth is required again once this much
/// time has elapsed since the last one. 5 minutes — more relaxed than
/// Recall-style per-view re-auth, but long enough to still block a physical
/// accessor after the user steps away.
pub const DEFAULT_REAUTH_IDLE_TIMEOUT_SECS: u64 = 300;

/// Lower bound for the idle expiry (seconds). 0 is allowed (meaning re-auth
/// on every request), but the UI slider's practical floor is 15 seconds
/// (to avoid a usability collapse from overly frequent prompts).
pub const MIN_REAUTH_IDLE_TIMEOUT_SECS: u64 = 15;

/// Upper bound for the idle expiry (seconds). 1 hour — beyond this the
/// physical-access protection re-auth provides becomes too weak.
pub const MAX_REAUTH_IDLE_TIMEOUT_SECS: u64 = 3600;

fn default_reauth_enabled() -> bool {
    true
}

fn default_reauth_idle_timeout_secs() -> u64 {
    DEFAULT_REAUTH_IDLE_TIMEOUT_SECS
}

/// Capture-history re-authentication settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReauthConfig {
    /// Whether to require re-auth when viewing capture history. Defaults to
    /// true (privacy-first product).
    #[serde(default = "default_reauth_enabled")]
    pub enabled: bool,
    /// Idle expiry in seconds. Re-auth is required again once this much time
    /// has elapsed since the last one.
    #[serde(default = "default_reauth_idle_timeout_secs")]
    pub idle_timeout_secs: u64,
}

impl ReauthConfig {
    /// The idle-expiry `Duration`, clamped to a safe range.
    ///
    /// Guards against a corrupted/malicious config supplying an
    /// unreasonable value (0 = DoS-style prompting, `u64::MAX` = effectively
    /// infinite), so the gate always operates within a sane range.
    #[must_use]
    pub fn effective_idle_timeout(&self) -> Duration {
        let secs = self
            .idle_timeout_secs
            .clamp(MIN_REAUTH_IDLE_TIMEOUT_SECS, MAX_REAUTH_IDLE_TIMEOUT_SECS);
        Duration::from_secs(secs)
    }
}

impl Default for ReauthConfig {
    fn default() -> Self {
        Self {
            enabled: default_reauth_enabled(),
            idle_timeout_secs: default_reauth_idle_timeout_secs(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_enabled_with_five_minute_idle() {
        let config = ReauthConfig::default();
        assert!(
            config.enabled,
            "a privacy-first product defaults re-auth on"
        );
        assert_eq!(config.idle_timeout_secs, 300);
        assert_eq!(config.effective_idle_timeout(), Duration::from_secs(300));
    }

    #[test]
    fn missing_fields_deserialize_to_secure_defaults() {
        let restored: ReauthConfig = serde_json::from_str("{}").expect("must deserialise");
        assert!(
            restored.enabled,
            "a config missing this field must load with re-auth enabled (fail-secure)"
        );
        assert_eq!(restored.idle_timeout_secs, 300);
    }

    #[test]
    fn effective_idle_timeout_clamps_out_of_range() {
        let too_small = ReauthConfig {
            enabled: true,
            idle_timeout_secs: 0,
        };
        assert_eq!(
            too_small.effective_idle_timeout(),
            Duration::from_secs(MIN_REAUTH_IDLE_TIMEOUT_SECS)
        );

        let too_large = ReauthConfig {
            enabled: true,
            idle_timeout_secs: u64::MAX,
        };
        assert_eq!(
            too_large.effective_idle_timeout(),
            Duration::from_secs(MAX_REAUTH_IDLE_TIMEOUT_SECS)
        );
    }

    #[test]
    fn round_trip_preserves_values() {
        let config = ReauthConfig {
            enabled: false,
            idle_timeout_secs: 120,
        };
        let json = serde_json::to_string(&config).expect("serialise");
        let restored: ReauthConfig = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(config, restored);
    }
}
