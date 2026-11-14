//! Klangwelt-DSP – alles selbst geschrieben, ohne externe DSP-Bibliotheken.
//!
//! * [`features`] – messbare Klangeigenschaften (Helligkeit, Wärme, Anschlag, Ausklingen)
//! * [`fft`]     – Radix-2-FFT
//! * [`fx`]      – Klangformung (Wärme, Schwebung, Farbe, Rauheit, Entfernung, Breite)
//! * [`hits`]    – Anschläge einer Aufnahme Trommeln zuordnen (Bassdrum, Snare, Hi-Hat)
//! * [`pad`]     – Synthesizer für Flächen mit gezeichnetem Verlauf
//! * [`pitch`]   – Tonhöhenerkennung (YIN)
//! * [`notes`]   – Tonhöhenkurve → einzelne Noten
//! * [`onset`]   – Einsätze (Spectral Flux) und Tempo-Schätzung
//! * [`stretch`] – Tempo/Tonhöhe ändern (WSOLA + Sinc-Resampling)
//! * [`synth`]   – Klangerzeugung (Zupf-Modell, Drums) – Keimzelle der Soundwerkstatt
//! * [`util`]    – kleine Helfer (Pegel, Median, Wellenform-Übersicht)

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
