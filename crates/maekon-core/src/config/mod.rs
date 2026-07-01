// config/ — application configuration module (ADR-003 directory module).
//
// Split from a single 892-line file per ADR-013:
//   app_config   — AppConfig struct + methods + tests
//   enums        — shared config enums (PiiFilterLevel, AiProviderType, etc.)
//   sections/    — per-domain config section structs
//
// Public API (AppConfig, all section types) unchanged via re-exports.
mod app_config;
mod enums;
mod managed;
mod sections;

// ── Public re-exports (external API) ────────────────────────────────
pub use app_config::AppConfig;
pub use enums::*;
pub use managed::{
    load_managed_config, ManagedAudio, ManagedAuditPolicy, ManagedConfig, ManagedEffectiveReport,
    ManagedEffectiveSetting, ManagedFeaturePin, ManagedFeaturePins, ManagedPermissionProfilePolicy,
    ManagedPrivacy, ManagedProviderSurfacePolicy, ManagedSandboxNetworkPolicy,
    ManagedSignaturePolicy, ManagedTelemetry, ManagedUpdate, ManagedVision,
};
pub use sections::*;
