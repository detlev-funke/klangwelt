//! Aufnahme-Analyse: Tonhöhenkurve → Noten → Tonart → Bereinigung.

use klang_dsp::notes::{hz_to_midi, segment_notes, SegmentConfig};
use klang_dsp::onset::{detect_onsets, estimate_tempo, OnsetConfig};
use klang_dsp::pitch::{detect_pitch, hit_pitches, HitPitch, YinConfig};
use klang_music::melody::MelodyNote;
use klang_music::theory::{estimate_key, snap_to_key, TimedNote};
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyzedNote {
    pub start_secs: f64,
    pub end_secs: f64,
    /// Bereinigter Ton (auf die Tonart gezogen, falls unsauber gesungen).
    pub midi: u8,
    /// Ursprünglich erkannter Ton.
    pub raw_midi: u8,
    pub cents: f32,
    pub velocity: f32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyInfo {
    pub name: String,
    pub tonic: u8,
    pub minor: bool,
    pub confidence: f32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Analysis {
    pub asset_id: String,
    pub duration_secs: f64,
    pub sample_rate: u32,
    /// (Zeit, MIDI-Tonhöhe oder null)
    pub pitch: Vec<(f32, Option<f32>)>,
    pub notes: Vec<AnalyzedNote>,
    pub onsets: Vec<f32>,
    /// Grundton je Anschlag (Schlagzeug, Bass, gezupfte Töne); `midi: null` = tonlos.
    pub hits: Vec<HitPitch>,
    pub tempo_estimate: Option<f32>,
    pub key: Option<KeyInfo>,
}

pub fn analyze(asset_id: &str, samples: &[f32], sample_rate: u32) -> Analysis {
    analyze_with(asset_id, samples, sample_rate, &YinConfig::default())
}

/// Vorgesungene oder gepfiffene Melodie für die Bibliothekssuche: Töne
/// (Dauer in Sekunden, auf die Tonart bereinigt) und die geschätzte Tonart.
pub fn query_melody(samples: &[f32], sample_rate: u32) -> (Vec<MelodyNote>, Option<String>) {
    let a = analyze_with("vorsingen", samples, sample_rate, &YinConfig::voice_and_whistle());
    let notes = a.notes.iter().map(|n| MelodyNote { onset: n.start_secs, duration: (n.end_secs - n.start_secs).max(0.05), midi: n.midi }).collect();
    (notes, a.key.map(|k| k.name))
}

pub fn analyze_with(asset_id: &str, samples: &[f32], sample_rate: u32, yin: &YinConfig) -> Analysis {
    let sr = sample_rate as f32;
    let frames = detect_pitch(samples, sr, yin);
    let onsets = detect_onsets(samples, sr, &OnsetConfig::default());
    let tempo_estimate = estimate_tempo(&onsets.envelope, onsets.hop_secs, 60.0, 200.0);
    let raw = segment_notes(&frames, &onsets.onsets, &SegmentConfig::default());
    let hits = hit_pitches(samples, sr, &onsets.onsets);

    let timed: Vec<TimedNote> = raw
        .iter()
        .map(|n| TimedNote { start: n.start as f64, end: n.end as f64, midi: n.midi, cents: n.cents, velocity: n.velocity })
        .collect();
    let key = estimate_key(&timed);
    let cleaned = match &key {
        Some(k) => snap_to_key(&timed, k),
        None => timed.clone(),
    };

    Analysis {
        asset_id: asset_id.to_string(),
        duration_secs: samples.len() as f64 / sample_rate.max(1) as f64,
        sample_rate,
        pitch: frames.iter().step_by(2).map(|f| (f.time, f.freq.map(hz_to_midi))).collect(),
        notes: cleaned
            .iter()
            .zip(&timed)
            .map(|(c, r)| AnalyzedNote {
                start_secs: c.start,
                end_secs: c.end,
                midi: c.midi,
                raw_midi: r.midi,
                cents: r.cents,
                velocity: c.velocity,
            })
            .collect(),
        onsets: onsets.onsets,
        hits,
        tempo_estimate,
        key: key.map(|k| KeyInfo { name: k.name_de(), tonic: k.tonic, minor: k.minor, confidence: k.confidence }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// „Gesungene“ Melodie: Grundton + Obertöne, Vibrato, Pausen, ein unsauberer Ton.
    fn sing(notes: &[(f32, f32)], sr: f32) -> Vec<f32> {
        let mut out = Vec::new();
        let mut phase = 0.0f32;
        for &(midi, secs) in notes {
            let n = (secs * sr) as usize;
            for i in 0..n {
                let t = i as f32 / sr;
                let vib = 0.15 * (2.0 * std::f32::consts::PI * 5.5 * t).sin();
                let f = 440.0 * 2f32.powf((midi + vib - 69.0) / 12.0);
                phase += 2.0 * std::f32::consts::PI * f / sr;
                let env = (t / 0.02).min(1.0) * ((secs - t) / 0.03).clamp(0.0, 1.0);
                out.push(env * (0.4 * phase.sin() + 0.15 * (2.0 * phase).sin() + 0.06 * (3.0 * phase).sin()));
            }
            out.extend(std::iter::repeat(0.0).take((0.04 * sr) as usize));
        }
        out
    }

    #[test]
    fn recording_pipeline_finds_melody_key_and_fixes_sloppy_note() {
        let sr = 44_100.0;
        // C-Dur-Melodie; der 6. Ton ist ein zu tief gesungenes c'' (72), gemessen ≈ 72,62 → cis.
        let melody = [
            (60.0, 0.35), (64.0, 0.35), (67.0, 0.35), (72.0, 0.5), (71.0, 0.35), (72.62, 0.35),
            (67.0, 0.35), (69.0, 0.35), (65.0, 0.35), (64.0, 0.35), (62.0, 0.35), (60.0, 0.7),
        ];
        let x = sing(&melody, sr);
        let a = analyze("t", &x, sr as u32);
        let got: Vec<u8> = a.notes.iter().map(|n| n.midi).collect();
        assert_eq!(got, vec![60, 64, 67, 72, 71, 72, 67, 69, 65, 64, 62, 60], "{:?}", a.notes);
        assert_eq!(a.notes[5].raw_midi, 73, "unsauberer Ton sollte als cis erkannt und korrigiert werden");
        let key = a.key.expect("Tonart");
        assert_eq!(key.name, "C-Dur");
        assert!(a.pitch.iter().any(|p| p.1.is_some()));
    }
}
