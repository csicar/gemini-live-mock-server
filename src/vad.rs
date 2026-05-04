/// Energy-based Voice Activity Detection (VAD)
///
/// Detects speech start and end based on audio energy levels.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadState {
    Silence,
    Speech,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadEvent {
    SpeechStart,
    SpeechEnd,
}

pub struct Vad {
    state: VadState,
    energy_threshold: f32,
    silence_frames_threshold: u32,
    silence_frame_count: u32,
}

impl Vad {
    pub fn new(energy_threshold: f32, silence_frames_threshold: u32) -> Self {
        Self {
            state: VadState::Silence,
            energy_threshold,
            silence_frames_threshold,
            silence_frame_count: 0,
        }
    }

    /// Process a chunk of PCM audio data (16-bit signed integers)
    /// Returns any state change event
    pub fn process(&mut self, pcm_data: &[i16]) -> Option<VadEvent> {
        let energy = calculate_rms(pcm_data);
        let is_speech = energy > self.energy_threshold;

        match self.state {
            VadState::Silence => {
                if is_speech {
                    self.state = VadState::Speech;
                    self.silence_frame_count = 0;
                    Some(VadEvent::SpeechStart)
                } else {
                    None
                }
            }
            VadState::Speech => {
                if is_speech {
                    self.silence_frame_count = 0;
                    None
                } else {
                    self.silence_frame_count += 1;
                    if self.silence_frame_count >= self.silence_frames_threshold {
                        self.state = VadState::Silence;
                        self.silence_frame_count = 0;
                        Some(VadEvent::SpeechEnd)
                    } else {
                        None
                    }
                }
            }
        }
    }

    /// Reset the VAD state
    pub fn reset(&mut self) {
        self.state = VadState::Silence;
        self.silence_frame_count = 0;
    }

    /// Get the current state
    pub fn state(&self) -> VadState {
        self.state
    }
}

/// Calculate the RMS (Root Mean Square) energy of PCM audio samples
fn calculate_rms(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }

    let sum_squares: f64 = samples
        .iter()
        .map(|&s| {
            let normalized = s as f64 / i16::MAX as f64;
            normalized * normalized
        })
        .sum();

    (sum_squares / samples.len() as f64).sqrt() as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rms_silence() {
        let samples = vec![0i16; 100];
        assert_eq!(calculate_rms(&samples), 0.0);
    }

    #[test]
    fn test_rms_loud() {
        let samples = vec![i16::MAX; 100];
        let rms = calculate_rms(&samples);
        assert!((rms - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_vad_speech_detection() {
        let mut vad = Vad::new(0.01, 3);

        // Silence
        let silence = vec![0i16; 100];
        assert_eq!(vad.process(&silence), None);
        assert_eq!(vad.state(), VadState::Silence);

        // Speech starts
        let loud = vec![i16::MAX / 2; 100];
        assert_eq!(vad.process(&loud), Some(VadEvent::SpeechStart));
        assert_eq!(vad.state(), VadState::Speech);

        // Speech continues
        assert_eq!(vad.process(&loud), None);

        // Silence detected but not long enough
        assert_eq!(vad.process(&silence), None);
        assert_eq!(vad.process(&silence), None);
        assert_eq!(vad.state(), VadState::Speech);

        // Silence long enough - speech ends
        assert_eq!(vad.process(&silence), Some(VadEvent::SpeechEnd));
        assert_eq!(vad.state(), VadState::Silence);
    }
}
