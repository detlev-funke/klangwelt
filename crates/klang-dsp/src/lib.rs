//! Klangwelt DSP – all self-written, without external DSP libraries.
//!
//! * [`features`] – measurable sound properties (brightness, warmth, attack, decay)
//! * [`fft`]     – radix-2 FFT
//! * [`fx`]      – sound shaping (warmth, shimmer, color, roughness, distance, width)
//! * [`hits`]    – assign the hits of a recording to drums (bass drum, snare, hi-hat)
//! * [`pad`]     – synthesizer for pads with a drawn envelope
//! * [`pitch`]   – pitch detection (YIN)
//! * [`notes`]   – pitch curve → individual notes
//! * [`onset`]   – onsets (spectral flux) and tempo estimation
//! * [`stretch`] – change tempo/pitch (WSOLA + sinc resampling)
//! * [`synth`]   – sound generation (plucked-string model, drums) – germ of the sound workshop
//! * [`util`]    – small helpers (level, median, waveform overview)

pub mod bass;
pub mod drums;
pub mod features;
pub mod fft;
pub mod fx;
pub mod hits;
pub mod keys;
pub mod notes;
pub mod onset;
pub mod pad;
pub mod pitch;
pub mod stretch;
pub mod synth;
pub mod util;

#[cfg(test)]
pub(crate) mod testutil {
    pub fn sine(freq: f32, secs: f32, sr: f32, amp: f32) -> Vec<f32> {
        let n = (secs * sr) as usize;
        (0..n)
            .map(|i| amp * (2.0 * std::f32::consts::PI * freq * i as f32 / sr).sin())
            .collect()
    }
}
