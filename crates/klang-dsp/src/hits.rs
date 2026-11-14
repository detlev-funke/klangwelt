//! Anschläge einer Aufnahme Trommeln zuordnen – für „Audio → Rhythmus-Spur“.
//!
//! Vom Anfang jedes Anschlags (bis 100 ms, höchstens bis zum nächsten) wird
//! gemessen, wie sich die Energie auf drei Bänder verteilt: tief (unter
//! ≈ 150 Hz), Mitte und hoch (über ≈ 5 kHz). Bassdrum: überwiegend tief.
//! Hi-Hat (auch Shaker, Becken): viel Höhe, kaum Tiefe. Alles dazwischen –
//! Snare, Clap, Rimshot, höhere Toms – wird zur Snare; tiefe Toms können als
//! Bassdrum durchgehen. Ein erkannter tiefer Grundton (unter ≈ 80 Hz) hilft
//! bei knappen Fällen.

use crate::drums::{HAT_CLOSED, KICK, SNARE};

/// Trommel einer Rhythmus-Spur aus einer Aufnahme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Drum {
    Kick,
    Snare,
    HiHat,
}

impl Drum {
    /// Taste (General MIDI) der Schlagzeug-Spur.
    pub fn midi(self) -> u8 {
        match self {
            Drum::Kick => KICK,
            Drum::Snare => SNARE,
            Drum::HiHat => HAT_CLOSED,
        }
    }
}

/// Energieanteile (0…1) im tiefen und im hohen Band.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bands {
    pub low: f32,
    pub high: f32,
}

/// Zweistufiger Tiefpass (je eine Polstelle) bei `fc` Hz.
fn lowpass(x: &[f32], sr: f32, fc: f32) -> Vec<f32> {
    let a = (-2.0 * std::f32::consts::PI * fc / sr).exp();
    let (mut y1, mut y2) = (0.0f32, 0.0f32);
    x.iter()
        .map(|&v| {
            y1 = (1.0 - a) * v + a * y1;
            y2 = (1.0 - a) * y1 + a * y2;
            y2
        })
        .collect()
}

fn energy(v: &[f32]) -> f32 {
    v.iter().map(|s| s * s).sum::<f32>()
}

/// Energie (nicht Anteile) im tiefen, mittleren und hohen Band.
fn band_energy(x: &[f32], sr: f32) -> [f32; 3] {
    let total = energy(x);
    let low = energy(&lowpass(x, sr, 150.0));
    let below_5k = lowpass(x, sr, 5000.0);
    let high = x.iter().zip(&below_5k).map(|(a, b)| (a - b) * (a - b)).sum::<f32>();
    [low, (total - low - high).max(0.0), high]
}

fn shares(e: [f32; 3]) -> Bands {
    let total: f32 = e.iter().sum();
    if total <= 1e-12 {
        return Bands { low: 0.0, high: 0.0 };
    }
    Bands { low: (e[0] / total).min(1.0), high: (e[2] / total).min(1.0) }
}

pub fn bands(x: &[f32], sr: f32) -> Bands {
    shares(band_energy(x, sr))
}

/// Was ein Anschlag hinzufügt: Bandanteile und Energiezuwachs je Band
/// gegenüber dem gleich langen Abschnitt davor.
fn measure(samples: &[f32], sr: f32, time: f32, end: f32) -> (Bands, [f32; 3]) {
    let a = ((time.max(0.0) * sr) as usize).min(samples.len());
    let len = ((end - time).clamp(0.02, 0.1) * sr) as usize;
    let b = (a + len).min(samples.len());
    let after = band_energy(&samples[a..b], sr);
    let before = band_energy(&samples[a.saturating_sub(b - a)..a], sr);
    let rise = [0, 1, 2].map(|k| (after[k] - before[k]).max(0.0));
    let bd = if rise.iter().sum::<f32>() > 0.05 * after.iter().sum::<f32>() { shares(rise) } else { shares(after) };
    (bd, rise)
}

fn decide(bd: Bands, midi: Option<f32>) -> Drum {
    let deep_tone = midi.is_some_and(|m| m < 40.0);
    if bd.low >= 0.35 || (deep_tone && bd.low >= 0.2) {
        Drum::Kick
    } else if bd.high >= 0.5 && bd.low < 0.1 {
        Drum::HiHat
    } else {
        Drum::Snare
    }
}

/// Ordnet einen Anschlag (Einsatz `time`, Abschnittsende `end`, beides
/// Sekunden; `midi` = erkannter Grundton) einer Trommel zu. Gemessen wird,
/// was gegenüber dem Abschnitt davor *hinzukommt* – eine ausklingende
/// Bassdrum unter der nächsten Hi-Hat zählt so nicht mit.
pub fn classify(samples: &[f32], sr: f32, time: f32, end: f32, midi: Option<f32>) -> Drum {
    decide(measure(samples, sr, time, end).0, midi)
}

/// Alle Anschläge einer Aufnahme (Einsatz, Abschnittsende, Grundton) zuordnen.
/// Eine Bassdrum, die so viel Höhe mitbringt wie die Hi-Hats derselben
/// Aufnahme, bekommt die Hi-Hat dazu (Hi-Hat auf der Zählzeit) – neben einer
/// Bassdrum ist eine Hi-Hat am Energieanteil allein nicht zu erkennen.
pub fn classify_hits(samples: &[f32], sr: f32, hits: &[(f32, f32, Option<f32>)]) -> Vec<Vec<Drum>> {
    let measured: Vec<(Drum, f32)> = hits
        .iter()
        .map(|&(t, e, midi)| {
            let (bd, rise) = measure(samples, sr, t, e);
            (decide(bd, midi), rise[2])
        })
        .collect();
    let mut hat_highs: Vec<f32> = measured.iter().filter(|(d, _)| *d == Drum::HiHat).map(|(_, h)| *h).collect();
    hat_highs.sort_by(f32::total_cmp);
    let typical_hat = (hat_highs.len() >= 2).then(|| hat_highs[hat_highs.len() / 2]);
    measured
        .into_iter()
        .map(|(d, high)| match (d, typical_hat) {
            (Drum::Kick, Some(h)) if high >= 0.4 * h => vec![Drum::Kick, Drum::HiHat],
            _ => vec![d],
        })
        .collect()
}

/// Einsätze im tiefen Band (unter ≈ 150 Hz) mit Stärke 0…1: Bassdrums gehen in
/// der allgemeinen Einsatzerkennung (über alle Frequenzen) neben lauten
/// Becken leicht unter. Ein Einsatz ist ein Energiesprung um mindestens 6 dB
/// binnen 20 ms auf mindestens 8 % der lautesten Stelle.
pub fn low_onsets(samples: &[f32], sr: f32) -> Vec<(f32, f32)> {
    let low = lowpass(samples, sr, 150.0);
    let hop = ((0.005 * sr) as usize).max(1);
    let win = ((0.02 * sr) as usize).max(1);
    let env: Vec<f32> = (0..low.len().saturating_sub(win) / hop).map(|k| energy(&low[k * hop..k * hop + win])).collect();
    let max = env.iter().copied().fold(0.0f32, f32::max);
    if max <= 1e-9 {
        return Vec::new();
    }
    let lag = (win / hop).max(1);
    let rise = |k: usize| if k >= lag { env[k] - env[k - lag] } else { env[k] };
    let mut out: Vec<(f32, f32)> = Vec::new();
    for k in 1..env.len().saturating_sub(1) {
        let before = if k >= lag { env[k - lag] } else { 0.0 };
        if env[k] < 0.08 * max || env[k] < 4.0 * before {
            continue;
        }
        // steilster Anstieg in der Nachbarschaft (±30 ms)
        let r = rise(k);
        let span = (0.03 * sr) as usize / hop;
        if (k.saturating_sub(span)..(k + span + 1).min(env.len())).any(|j| j != k && rise(j) > r) {
            continue;
        }
        let t = (k * hop) as f32 / sr;
        if out.last().is_some_and(|(p, _)| t - p < 0.08) {
            continue;
        }
        // Stärke: Spitze der folgenden 50 ms relativ zur lautesten Stelle
        let peak = env[k..(k + (0.05 * sr) as usize / hop + 1).min(env.len())].iter().copied().fold(0.0f32, f32::max);
        out.push((t, (peak / max).sqrt().min(1.0)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drums::{self, DrumKit};

    #[test]
    fn synthesized_drums_are_recognised() {
        let sr = 44_100.0;
        let kit = DrumKit::default();
        let cases = [
            (drums::KICK, Drum::Kick),
            (drums::SNARE, Drum::Snare),
            (drums::CLAP, Drum::Snare),
            (drums::RIM, Drum::Snare),
            (drums::HAT_CLOSED, Drum::HiHat),
            (drums::HAT_OPEN, Drum::HiHat),
            (drums::SHAKER, Drum::HiHat),
        ];
        for (midi, want) in cases {
            for (vel, seed) in [(0.9, 1), (0.5, 7)] {
                let x = drums::render_hit(&kit, midi, vel, sr, seed).unwrap();
                let got = classify(&x, sr, 0.0, 0.3, None);
                assert_eq!(got, want, "Taste {midi} (Anschlag {vel}): {:?}", bands(&x[..(0.06 * sr) as usize], sr));
            }
        }
        // tiefere und höhere Bassdrum
        for grundton in [28.0, 36.0, 40.0] {
            let k = DrumKit { kick: drums::KickParams { grundton, ..kit.kick }, ..kit };
            let x = drums::render_hit(&k, drums::KICK, 0.8, sr, 3).unwrap();
            assert_eq!(classify(&x, sr, 0.0, 0.3, None), Drum::Kick, "Grundton {grundton}");
        }
    }

    /// Takt bei 120 BPM aus Klängen des eigenen Schlagzeugs: (Taste, Sekunde).
    fn beat(hits: &[(u8, f32)], sr: f32) -> Vec<f32> {
        let kit = DrumKit::default();
        let mut out = vec![0.0f32; (2.6 * sr) as usize];
        for (k, &(midi, t)) in hits.iter().enumerate() {
            let x = drums::render_hit(&kit, midi, 0.9, sr, k as u32 + 1).unwrap();
            let a = (t * sr) as usize;
            for (i, v) in x.iter().enumerate() {
                if let Some(o) = out.get_mut(a + i) {
                    *o += v;
                }
            }
        }
        out
    }

    #[test]
    fn hat_over_a_ringing_kick_stays_a_hat_and_kick_with_hat_gives_both() {
        let sr = 44_100.0;
        let x = beat(&[(drums::KICK, 0.0), (drums::HAT_CLOSED, 0.25), (drums::SNARE, 0.5), (drums::KICK, 1.0), (drums::HAT_CLOSED, 1.0)], sr);
        assert_eq!(classify(&x, sr, 0.25, 0.5, None), Drum::HiHat, "Bassdrum klingt noch nach");
        assert_eq!(classify(&x, sr, 0.5, 1.0, None), Drum::Snare);
        // mit weiteren Hi-Hats in der Aufnahme: Bassdrum + Hi-Hat bei 1,0 erkannt, bei 0 nur die Bassdrum
        let more = beat(&[(drums::KICK, 0.0), (drums::HAT_CLOSED, 0.25), (drums::SNARE, 0.5), (drums::HAT_CLOSED, 0.75), (drums::KICK, 1.0), (drums::HAT_CLOSED, 1.0)], sr);
        let got = classify_hits(&more, sr, &[(0.0, 0.25, None), (0.25, 0.5, None), (0.5, 0.75, None), (0.75, 1.0, None), (1.0, 1.4, None)]);
        assert_eq!(got, vec![vec![Drum::Kick], vec![Drum::HiHat], vec![Drum::Snare], vec![Drum::HiHat], vec![Drum::Kick, Drum::HiHat]]);
    }

    #[test]
    fn low_band_finds_kicks_between_loud_hats() {
        let sr = 44_100.0;
        let mut hits = vec![(drums::KICK, 0.0), (drums::KICK, 1.0), (drums::KICK, 2.0), (drums::SNARE, 0.5), (drums::SNARE, 1.5)];
        for k in 0..8 {
            hits.push((drums::HAT_OPEN, 0.25 * k as f32 + 0.125));
        }
        let x = beat(&hits, sr);
        let found: Vec<f32> = low_onsets(&x, sr).into_iter().map(|(t, _)| t).collect();
        assert_eq!(found.len(), 3, "nur die Bassdrums: {found:?}");
        for (t, want) in found.iter().zip([0.0, 1.0, 2.0]) {
            assert!((t - want).abs() < 0.02, "Bassdrum bei {want}: {t}");
        }
        assert!(low_onsets(&vec![0.0; 1000], sr).is_empty());
    }

    #[test]
    fn silence_and_offsets_are_safe() {
        let sr = 8000.0;
        let x = vec![0.0f32; 400];
        assert_eq!(classify(&x, sr, 0.0, 0.1, None), Drum::Snare);
        assert_eq!(classify(&x, sr, 10.0, 10.1, None), Drum::Snare, "hinter dem Ende");
        assert_eq!(classify(&[], sr, 0.0, 0.1, None), Drum::Snare);
    }
}
