//! Energy-based Voice Activity Detection (VAD).
//!
//! Pure computation — no I/O, no async, no mutex. Designed to be owned by
//! the cpal audio callback closure so no synchronization is needed for the
//! VAD state itself.

use std::time::Instant;

/// VAD detector state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadState {
    /// Waiting for start_vad call.
    Idle,
    /// Listening for speech.
    Listening,
    /// Speech detected (energy above threshold).
    SpeechDetected,
    /// Silence after speech — waiting for silence_ms to confirm end.
    SilenceAfterSpeech,
}

/// Events emitted by VadDetector on each process_chunk call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadEvent {
    /// No significant event.
    None,
    /// Speech just started (first chunk above threshold after min_speech_ms).
    SpeechStarted,
    /// Speech is continuing.
    SpeechContinuing,
    /// Speech ended (silence exceeded silence_ms).
    SpeechEnded,
}

/// Energy-based voice activity detector.
///
/// Uses RMS energy to classify audio chunks as speech or silence.
/// State transitions:
/// - Idle/Listening → SpeechDetected (energy > threshold)
/// - SpeechDetected → SilenceAfterSpeech (energy < threshold)
/// - SilenceAfterSpeech → SpeechDetected (energy > threshold, speech resumes)
/// - SilenceAfterSpeech → Idle (silence lasts >= silence_ms, and speech lasted >= min_speech_ms)
pub struct VadDetector {
    threshold: f32,
    silence_ms: u32,
    min_speech_ms: u32,
    state: VadState,
    /// When speech was first detected in the current utterance.
    speech_start: Option<Instant>,
    /// Last time energy was above threshold.
    last_speech: Option<Instant>,
}

impl VadDetector {
    /// Create a new VadDetector.
    ///
    /// - `threshold`: RMS energy threshold (0.0–1.0). Higher = less sensitive.
    /// - `silence_ms`: How long silence must persist to end an utterance.
    /// - `min_speech_ms`: Minimum speech duration to trigger SpeechEnded.
    pub fn new(threshold: f32, silence_ms: u32, min_speech_ms: u32) -> Self {
        Self {
            threshold,
            silence_ms,
            min_speech_ms,
            state: VadState::Idle,
            speech_start: None,
            last_speech: None,
        }
    }

    /// Process a chunk of mono f32 audio samples.
    ///
    /// Returns a `VadEvent` indicating the detected transition.
    /// Call this once per cpal callback with the downmixed mono samples.
    pub fn process_chunk(&mut self, samples: &[f32]) -> VadEvent {
        if samples.is_empty() {
            return VadEvent::None;
        }

        let energy = rms(samples);
        let now = Instant::now();
        let is_speech = energy >= self.threshold;

        match self.state {
            VadState::Idle | VadState::Listening => {
                if is_speech {
                    // First speech chunk — record start time
                    if self.speech_start.is_none() {
                        self.speech_start = Some(now);
                    }
                    self.last_speech = Some(now);
                    self.state = VadState::Listening;

                    // Check if speech has lasted long enough to confirm
                    if let Some(start) = self.speech_start {
                        if now.duration_since(start).as_millis() >= self.min_speech_ms as u128 {
                            self.state = VadState::SpeechDetected;
                            return VadEvent::SpeechStarted;
                        }
                    }
                    VadEvent::None
                } else if self.state == VadState::Listening {
                    // Silence while still in tentative listening — check if we should
                    // abandon (never reached min_speech_ms)
                    if let Some(last) = self.last_speech {
                        if now.duration_since(last).as_millis() >= self.silence_ms as u128 {
                            // Too much silence before min_speech_ms — reset
                            self.state = VadState::Idle;
                            self.speech_start = None;
                            self.last_speech = None;
                        }
                    }
                    VadEvent::None
                } else {
                    VadEvent::None
                }
            }
            VadState::SpeechDetected | VadState::SilenceAfterSpeech => {
                if is_speech {
                    self.last_speech = Some(now);
                    self.state = VadState::SpeechDetected;
                    VadEvent::SpeechContinuing
                } else if let Some(last) = self.last_speech {
                    // Silence — check if silence duration threshold exceeded
                    if now.duration_since(last).as_millis() >= self.silence_ms as u128 {
                        self.state = VadState::Idle;
                        self.speech_start = None;
                        self.last_speech = None;
                        VadEvent::SpeechEnded
                    } else {
                        self.state = VadState::SilenceAfterSpeech;
                        VadEvent::SpeechContinuing
                    }
                } else {
                    VadEvent::None
                }
            }
        }
    }

    /// Reset detector to idle state.
    pub fn reset(&mut self) {
        self.state = VadState::Idle;
        self.speech_start = None;
        self.last_speech = None;
    }

    /// Current detector state.
    pub fn state(&self) -> VadState {
        self.state
    }
}

/// Compute RMS (root-mean-square) energy of a sample buffer.
fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silent_input_returns_none() {
        let mut vad = VadDetector::new(0.02, 800, 300);
        let silence = vec![0.0f32; 1024];
        let event = vad.process_chunk(&silence);
        assert_eq!(event, VadEvent::None);
        assert_eq!(vad.state(), VadState::Idle);
    }

    #[test]
    fn loud_input_starts_speech() {
        let mut vad = VadDetector::new(0.01, 800, 0); // min_speech_ms=0 for immediate start
        let loud = vec![0.5f32; 1024];
        let event = vad.process_chunk(&loud);
        // With min_speech_ms=0, first loud chunk should transition to SpeechDetected
        assert_eq!(event, VadEvent::SpeechStarted);
        assert_eq!(vad.state(), VadState::SpeechDetected);
    }

    #[test]
    fn speech_then_silence_ends() {
        let mut vad = VadDetector::new(0.01, 0, 0); // silence_ms=0, min_speech_ms=0
        let loud = vec![0.5f32; 1024];
        let silence = vec![0.0f32; 1024];

        // Start speech
        let e1 = vad.process_chunk(&loud);
        assert_eq!(e1, VadEvent::SpeechStarted);

        // Silence → transitions to SilenceAfterSpeech
        let e2 = vad.process_chunk(&silence);
        // With silence_ms=0, should immediately end
        assert_eq!(e2, VadEvent::SpeechEnded);
        assert_eq!(vad.state(), VadState::Idle);
    }

    #[test]
    fn short_speech_no_end() {
        // min_speech_ms=5000 — speech won't reach this in a single chunk
        let mut vad = VadDetector::new(0.01, 800, 5000);
        let loud = vec![0.5f32; 1024];
        let silence = vec![0.0f32; 1024];

        // Loud input but not enough time
        let e1 = vad.process_chunk(&loud);
        assert_eq!(e1, VadEvent::None); // Still in Listening, not confirmed
        assert_eq!(vad.state(), VadState::Listening);

        // Silence — should not emit SpeechEnded since speech was never confirmed
        let e2 = vad.process_chunk(&silence);
        assert_eq!(e2, VadEvent::None);
    }

    #[test]
    fn rms_correctness() {
        // RMS of [1.0, -1.0] = sqrt((1+1)/2) = 1.0
        assert!((rms(&[1.0, -1.0]) - 1.0).abs() < 1e-6);

        // RMS of [0.0] = 0.0
        assert!((rms(&[0.0]) - 0.0).abs() < 1e-6);

        // RMS of [3.0, 4.0] = sqrt((9+16)/2) = sqrt(12.5) ≈ 3.5355
        let expected = (12.5f32).sqrt();
        assert!((rms(&[3.0, 4.0]) - expected).abs() < 1e-4);

        // RMS of empty = 0.0
        assert!((rms(&[]) - 0.0).abs() < 1e-6);
    }

    #[test]
    fn reset_clears_state() {
        let mut vad = VadDetector::new(0.01, 0, 0);
        let loud = vec![0.5f32; 1024];

        // Get into SpeechDetected state
        vad.process_chunk(&loud);
        assert_eq!(vad.state(), VadState::SpeechDetected);

        // Reset
        vad.reset();
        assert_eq!(vad.state(), VadState::Idle);

        // Should behave as fresh
        let silence = vec![0.0f32; 1024];
        let event = vad.process_chunk(&silence);
        assert_eq!(event, VadEvent::None);
        assert_eq!(vad.state(), VadState::Idle);
    }

    #[test]
    fn speech_continuing_after_confirmed() {
        let mut vad = VadDetector::new(0.01, 800, 0);
        let loud = vec![0.5f32; 1024];

        let e1 = vad.process_chunk(&loud);
        assert_eq!(e1, VadEvent::SpeechStarted);

        // Continued loud input
        let e2 = vad.process_chunk(&loud);
        assert_eq!(e2, VadEvent::SpeechContinuing);
        assert_eq!(vad.state(), VadState::SpeechDetected);
    }

    #[test]
    fn empty_input_returns_none() {
        let mut vad = VadDetector::new(0.02, 800, 300);
        let event = vad.process_chunk(&[]);
        assert_eq!(event, VadEvent::None);
    }

    #[test]
    fn all_zero_samples_no_speech() {
        let mut vad = VadDetector::new(0.001, 0, 0);
        let silence = vec![0.0f32; 4096];
        // Process multiple chunks — should never leave Idle
        for _ in 0..10 {
            let event = vad.process_chunk(&silence);
            assert_eq!(event, VadEvent::None);
        }
        assert_eq!(vad.state(), VadState::Idle);
    }

    #[test]
    fn above_threshold_triggers_speech() {
        // Use a very low threshold so any non-zero signal triggers
        let mut vad = VadDetector::new(0.001, 800, 0);
        // RMS of [0.1; 1024] = 0.1, well above 0.001
        let speech = vec![0.1f32; 1024];
        let event = vad.process_chunk(&speech);
        assert_eq!(event, VadEvent::SpeechStarted);
        assert_eq!(vad.state(), VadState::SpeechDetected);
    }

    #[test]
    fn below_threshold_no_speech() {
        // Threshold 0.5 — signal with RMS ~0.001 should not trigger
        let mut vad = VadDetector::new(0.5, 800, 0);
        let quiet = vec![0.001f32; 1024];
        let event = vad.process_chunk(&quiet);
        assert_eq!(event, VadEvent::None);
        assert!(
            vad.state() == VadState::Idle || vad.state() == VadState::Listening,
            "should remain idle or listening, got {:?}",
            vad.state()
        );
    }

    #[test]
    fn vad_config_threshold_boundary() {
        // Exactly at threshold — signal RMS equals threshold should count as speech
        let threshold = 0.5f32;
        let mut vad = VadDetector::new(threshold, 0, 0);
        // samples all = threshold → RMS = threshold → energy >= threshold → speech
        let samples = vec![threshold; 1024];
        let event = vad.process_chunk(&samples);
        assert_eq!(event, VadEvent::SpeechStarted);
    }

    #[test]
    fn rms_of_alternating_signal() {
        // Alternating +0.5 / -0.5 → RMS = 0.5
        let samples: Vec<f32> = (0..1024)
            .map(|i| if i % 2 == 0 { 0.5 } else { -0.5 })
            .collect();
        let energy = rms(&samples);
        assert!(
            (energy - 0.5).abs() < 1e-4,
            "expected RMS ~0.5, got {energy}"
        );
    }

    #[test]
    fn multiple_speech_silence_cycles() {
        let mut vad = VadDetector::new(0.01, 0, 0);
        let loud = vec![0.5f32; 1024];
        let silence = vec![0.0f32; 1024];

        // Cycle 1
        assert_eq!(vad.process_chunk(&loud), VadEvent::SpeechStarted);
        assert_eq!(vad.process_chunk(&silence), VadEvent::SpeechEnded);
        assert_eq!(vad.state(), VadState::Idle);

        // Cycle 2 — should work identically after returning to Idle
        assert_eq!(vad.process_chunk(&loud), VadEvent::SpeechStarted);
        assert_eq!(vad.process_chunk(&silence), VadEvent::SpeechEnded);
        assert_eq!(vad.state(), VadState::Idle);
    }
}
