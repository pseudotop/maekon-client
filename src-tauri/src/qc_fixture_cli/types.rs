use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SeedReport {
    pub(super) data_dir: String,
    pub(super) frames: usize,
    pub(super) events: usize,
    pub(super) already_seeded: bool,
}

impl fmt::Display for SeedReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "QC history fixture ready: data_dir={} frames={} events={} already_seeded={}",
            self.data_dir, self.frames, self.events, self.already_seeded
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SuggestionSeedReport {
    pub(super) data_dir: String,
    pub(super) suggestions: usize,
    pub(super) already_seeded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClaimsSeedReport {
    pub(super) data_dir: String,
    pub(super) claims: usize,
    pub(super) segments: usize,
    pub(super) edges: usize,
    pub(super) already_seeded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AudioSeedReport {
    pub(super) data_dir: String,
    pub(super) microphone_consent: bool,
    pub(super) synthetic_capture: bool,
    pub(super) cloud_stt_disabled: bool,
    pub(super) stale_state_removed: bool,
}

impl fmt::Display for ClaimsSeedReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "QC claims fixture ready: data_dir={} claims={} segments={} edges={} already_seeded={}",
            self.data_dir, self.claims, self.segments, self.edges, self.already_seeded
        )
    }
}

impl fmt::Display for SuggestionSeedReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "QC suggestion fixture ready: data_dir={} suggestions={} already_seeded={}",
            self.data_dir, self.suggestions, self.already_seeded
        )
    }
}

impl fmt::Display for AudioSeedReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "QC audio fixture ready: data_dir={} microphone_consent={} synthetic_capture={} cloud_stt_disabled={} stale_state_removed={}",
            self.data_dir,
            self.microphone_consent,
            self.synthetic_capture,
            self.cloud_stt_disabled,
            self.stale_state_removed
        )
    }
}
