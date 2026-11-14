//! Synthesizer für Flächen und lange Klänge (subtraktiv, mehrstimmig).
//!
//! Je Ton mehrere leicht verstimmte Oszillatoren (Sinus ↔ Sägezahn ↔
//! Rechteck, bandbegrenzt per PolyBLEP), ein Sub-Oszillator eine Oktave
//! tiefer und „Schimmer“ eine Oktave höher, dahinter ein resonanter Tiefpass
//! (2 × SVF). Der zeitliche Verlauf ist **gezeichnet** ([`Verlauf`]):
//! Lautstärke und Helligkeit als Kurven über die Zeit ab Tonbeginn, dazu ein
//! Puls im Takt und langsame Bewegung (LFO) im Filter.

use crate::bass::{poly_blep, Svf};
use crate::notes::midi_to_hz;
use crate::synth::{NoteSpec, Rng};
use serde::{Deserialize, Serialize};
use std::f32::consts::PI;

/// Klangregler, alle 0…1.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SynthParams {
    /// eine ↔ sieben Stimmen je Ton (Unisono)
    pub stimmen: f32,
    /// rein ↔ stark verstimmt (Abstand der Stimmen, bis 30 Cent)
    pub verstimmung: f32,
    /// rund (Sinus) ↔ Sägezahn ↔ hohl (Rechteck)
    pub wellenform: f32,
    /// Sinus eine Oktave tiefer
    pub sub: f32,
    /// Sägezahn eine Oktave höher (Glanz)
    pub schimmer: f32,
    /// dunkel ↔ hell (Grundstellung des Filters, die Kurve „hell“ wirkt darauf)
    pub filter: f32,
    /// weich ↔ singend (Betonung an der Grenzfrequenz)
    pub resonanz: f32,
    /// ruhig ↔ wogend (Tiefe der langsamen Filterbewegung)
    pub bewegung: f32,
    /// langsam ↔ schnell (Tempo dieser Bewegung, 0,05–6 Hz)
    pub tempo: f32,
}

impl Default for SynthParams {
    fn default() -> Self {
        Self { stimmen: 0.5, verstimmung: 0.35, wellenform: 0.45, sub: 0.25, schimmer: 0.15, filter: 0.45, resonanz: 0.2, bewegung: 0.25, tempo: 0.3 }
    }
}

/// Zeitlicher Verlauf eines Tons. Kurvenpunkte: [Sekunden ab Tonbeginn, Wert 0…1].
/// Nach dem letzten Punkt hält der Wert, solange der Ton gehalten wird; nach dem
/// Loslassen klingt er in `ausklang` Sekunden aus.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Verlauf {
    pub laut: Vec<[f32; 2]>,
    pub hell: Vec<[f32; 2]>,
    pub ausklang: f32,
    /// Puls im Takt: 0 = aus, 1 = Viertel, 2 = Achtel, 3 = Sechzehntel
    pub puls: u8,
    /// Wie tief der Puls die Lautstärke absenkt (0…1)
    pub pulstiefe: f32,
}

impl Default for Verlauf {
    fn default() -> Self {
        VERLAUF_PRESETS[0].verlauf()
    }
}

/// Eine Vorlage für den Verlauf.
pub struct VerlaufPreset {
    pub name: &'static str,
    pub beschreibung: &'static str,
    pub laut: &'static [[f32; 2]],
    pub hell: &'static [[f32; 2]],
    pub ausklang: f32,
    pub puls: u8,
    pub pulstiefe: f32,
}

impl VerlaufPreset {
    pub fn verlauf(&self) -> Verlauf {
        Verlauf { laut: self.laut.to_vec(), hell: self.hell.to_vec(), ausklang: self.ausklang, puls: self.puls, pulstiefe: self.pulstiefe }
    }
}

pub const VERLAUF_PRESETS: [VerlaufPreset; 8] = [
    VerlaufPreset {
        name: "Anschwellen",
        beschreibung: "schwillt langsam an und öffnet sich",
        laut: &[[0.0, 0.0], [1.6, 0.85], [2.8, 1.0]],
        hell: &[[0.0, 0.2], [2.2, 0.75], [3.5, 0.6]],
        ausklang: 1.6,
        puls: 0,
        pulstiefe: 0.0,
    },
    VerlaufPreset {
        name: "Glocke",
        beschreibung: "Anschlag, dann langes Ausklingen",
        laut: &[[0.0, 0.0], [0.01, 1.0], [0.5, 0.55], [1.8, 0.28], [4.0, 0.12]],
        hell: &[[0.0, 1.0], [0.3, 0.62], [1.5, 0.35], [4.0, 0.2]],
        ausklang: 2.2,
        puls: 0,
        pulstiefe: 0.0,
    },
    VerlaufPreset {
        name: "Atmen",
        beschreibung: "hebt und senkt sich im Atemtempo",
        laut: &[[0.0, 0.0], [1.0, 0.85], [2.0, 0.45], [3.0, 0.85], [4.0, 0.45], [5.0, 0.85]],
        hell: &[[0.0, 0.3], [1.0, 0.7], [2.0, 0.35], [3.0, 0.75], [4.0, 0.35], [5.0, 0.7]],
        ausklang: 1.8,
        puls: 0,
        pulstiefe: 0.0,
    },
    VerlaufPreset {
        name: "Aufblühen",
        beschreibung: "gleich da, wird immer heller",
        laut: &[[0.0, 0.0], [0.4, 0.6], [2.5, 0.9]],
        hell: &[[0.0, 0.08], [3.0, 0.95]],
        ausklang: 1.2,
        puls: 0,
        pulstiefe: 0.0,
    },
    VerlaufPreset {
        name: "Pulsieren",
        beschreibung: "pocht in Achteln",
        laut: &[[0.0, 0.0], [0.08, 0.9]],
        hell: &[[0.0, 0.45], [4.0, 0.7]],
        ausklang: 0.5,
        puls: 2,
        pulstiefe: 0.85,
    },
    VerlaufPreset {
        name: "Wellen",
        beschreibung: "wogt auf und ab",
        laut: &[[0.0, 0.0], [0.8, 0.7], [1.6, 0.95], [2.4, 0.6], [3.2, 0.95], [4.0, 0.6], [4.8, 0.95]],
        hell: &[[0.0, 0.2], [0.8, 0.6], [1.6, 0.3], [2.4, 0.7], [3.2, 0.3], [4.0, 0.7], [4.8, 0.3]],
        ausklang: 2.0,
        puls: 0,
        pulstiefe: 0.0,
    },
    VerlaufPreset {
        name: "Gate",
        beschreibung: "zerhackt in Sechzehnteln",
        laut: &[[0.0, 0.0], [0.02, 1.0]],
        hell: &[[0.0, 0.35], [4.0, 0.85]],
        ausklang: 0.3,
        puls: 3,
        pulstiefe: 1.0,
    },
    VerlaufPreset {
        name: "Stich",
        beschreibung: "kurz und gezupft, für Akzente",
        laut: &[[0.0, 0.0], [0.005, 1.0], [0.25, 0.35], [0.8, 0.0]],
        hell: &[[0.0, 0.9], [0.2, 0.4], [0.8, 0.2]],
        ausklang: 0.3,
        puls: 0,
        pulstiefe: 0.0,
    },
];

/// Wert einer Kurve zur Zeit `t` (linear zwischen den Punkten, außerhalb gehalten).
pub fn curve_at(points: &[[f32; 2]], t: f32) -> f32 {
    let Some(first) = points.first() else { return 1.0 };
    if t <= first[0] {
        return first[1];
    }
    for w in points.windows(2) {
        let ([t0, v0], [t1, v1]) = (w[0], w[1]);
        if t <= t1 {
            let span = (t1 - t0).max(1e-6);
            return v0 + (v1 - v0) * (t - t0) / span;
        }
    }
    points.last().map(|p| p[1]).unwrap_or(1.0)
}

/// Pulsfaktor (1 = voll) an der Stelle `phase` (0…1) einer Pulsperiode – mit
/// kurzen Rampen, damit es nicht knackt.
fn gate(phase: f32, depth: f32) -> f32 {
    const E: f32 = 0.05;
    let off = if phase < 0.5 - E {
        0.0
    } else if phase < 0.5 {
        (phase - (0.5 - E)) / E
    } else if phase < 1.0 - E {
        1.0
    } else {
        1.0 - (phase - (1.0 - E)) / E
    };
    1.0 - depth.clamp(0.0, 1.0) * off
}

/// Dauer einer Pulsperiode in Sekunden (`spb` = Sekunden je Schlag).
pub fn puls_period(puls: u8, spb: f32) -> Option<f32> {
    match puls {
        1 => Some(spb),
        2 => Some(spb * 0.5),
        3 => Some(spb * 0.25),
        _ => None,
    }
}

/// Lautstärke des Verlaufs zur Zeit `t` bei einem Ton der Länge `held` (ohne Puls).
pub fn level_at(v: &Verlauf, t: f32, held: f32) -> f32 {
    if t < held {
        curve_at(&v.laut, t)
    } else {
        let tail = (t - held) / (v.ausklang.max(0.02) / 4.0);
        curve_at(&v.laut, held) * (-tail).exp()
    }
}

fn voice(n: &NoteSpec, sr: f32, p: &SynthParams, v: &Verlauf, spb: f32, seed: u32, out: &mut [f32]) {
    let freq = midi_to_hz(n.midi);
    let held = n.dur_secs.max(0.02);
    let len = (((held + v.ausklang.max(0.02)) * sr) as usize).min(out.len());
    let u = 1 + (p.stimmen.clamp(0.0, 1.0) * 6.0).round() as usize;
    let spread = p.verstimmung.clamp(0.0, 1.0) * 30.0;
    let mut rng = Rng(seed.wrapping_mul(2_654_435_761) | 1);
    let ratios: Vec<f32> = (0..u)
        .map(|k| {
            let cents = if u == 1 { 0.0 } else { (k as f32 / (u - 1) as f32 * 2.0 - 1.0) * spread };
            2f32.powf(cents / 1200.0)
        })
        .collect();
    let mut phases: Vec<f32> = (0..u).map(|_| rng.next().abs()).collect();
    let (mut sub_ph, mut shim_ph) = (0.0f32, rng.next().abs());
    let w = p.wellenform.clamp(0.0, 1.0);
    let norm = 0.62 / (u as f32).sqrt();
    let k_res = 2.0 - 1.85 * p.resonanz.clamp(0.0, 1.0);
    let lfo_rate = 0.05 + 5.95 * p.tempo.clamp(0.0, 1.0).powi(2);
    let lfo_phase = rng.next().abs() * 2.0 * PI;
    let period = puls_period(v.puls, spb);
    let velocity = n.velocity.clamp(0.0, 1.0);
    let modulated = n.modulated();
    let (mut f1, mut f2) = (Svf::default(), Svf::default());
    let mut g = 0.0;
    for (i, o) in out.iter_mut().enumerate().take(len) {
        let t = i as f32 / sr;
        if i % 16 == 0 {
            let hb = curve_at(&v.hell, t.min(held));
            let lfo = p.bewegung.clamp(0.0, 1.0) * 1.4 * (2.0 * PI * lfo_rate * t + lfo_phase).sin();
            let oct = 4.3 + 5.2 * p.filter.clamp(0.0, 1.0) + 6.0 * (hb - 0.5) + 1.2 * n.brightness.clamp(-1.0, 1.0) + lfo;
            let cut = (20.0 * 2f32.powf(oct)).clamp(30.0, sr * 0.45);
            g = (PI * cut / sr).tan();
        }
        let f = if modulated { freq * 2f32.powf(n.pitch_offset(t) / 12.0) } else { freq };
        let mut sum = 0.0;
        for (ph, r) in phases.iter_mut().zip(&ratios) {
            let dt = (f * r / sr).min(0.45);
            *ph += dt;
            if *ph >= 1.0 {
                *ph -= 1.0;
            }
            let saw = 2.0 * *ph - 1.0 - poly_blep(*ph, dt);
            sum += if w < 0.5 {
                let a = w / 0.5;
                (1.0 - a) * (2.0 * PI * *ph).sin() + a * saw
            } else {
                let b = (w - 0.5) / 0.5;
                let square = (if *ph < 0.5 { 1.0 } else { -1.0 }) + poly_blep(*ph, dt) - poly_blep((*ph + 0.5) % 1.0, dt);
                (1.0 - b) * saw + b * 0.8 * square
            };
        }
        let dt = f / sr;
        sub_ph = (sub_ph + 0.5 * dt) % 1.0;
        let sdt = (2.0 * dt).min(0.45);
        shim_ph += sdt;
        if shim_ph >= 1.0 {
            shim_ph -= 1.0;
        }
        let shimmer = 2.0 * shim_ph - 1.0 - poly_blep(shim_ph, sdt);
        let x = sum * norm + p.sub.clamp(0.0, 1.0) * 0.55 * (2.0 * PI * sub_ph).sin() + p.schimmer.clamp(0.0, 1.0) * 0.3 * shimmer;
        let y = f2.lowpass(f1.lowpass(x, g, k_res), g, 1.414);
        let pulse = period.map(|per| gate((t % per) / per, v.pulstiefe)).unwrap_or(1.0);
        // weiche Kanten: 2 ms ein, Ende ohne Knacken
        let edge = (t / 0.002).min(1.0) * ((len - i) as f32 / (0.003 * sr)).min(1.0);
        *o += y * level_at(v, t, held) * pulse * edge * velocity * 0.3;
    }
}

/// Rendert Noten mit dem Synthesizer. `spb` = Sekunden je Schlag (für den Puls).
/// Der Ausklang darf über `total_secs` hinausreichen (bis zu 2 s).
pub fn render_synth(notes: &[NoteSpec], total_secs: f32, sr: f32, p: &SynthParams, v: &Verlauf, spb: f32) -> Vec<f32> {
    let tail = v.ausklang.clamp(0.0, 2.0);
    let mut out = vec![0f32; ((total_secs.max(0.0) + tail + 0.05) * sr) as usize];
    for (i, n) in notes.iter().enumerate() {
        let start = (n.start_secs.max(0.0) * sr) as usize;
        if start >= out.len() {
            continue;
        }
        voice(n, sr, p, v, spb, i as u32 * 7919 + 31, &mut out[start..]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::analyze_sound;

    const SR: f32 = 48_000.0;

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    fn chord(secs: f32) -> Vec<NoteSpec> {
        [57.0, 60.0, 64.0].iter().map(|&m| NoteSpec::new(0.0, secs, m, 0.9)).collect()
    }

    #[test]
    fn curves_interpolate_and_hold() {
        let c = [[0.0, 0.0], [1.0, 1.0], [2.0, 0.5]];
        assert_eq!(curve_at(&c, -1.0), 0.0);
        assert!((curve_at(&c, 0.5) - 0.5).abs() < 1e-6);
        assert!((curve_at(&c, 1.5) - 0.75).abs() < 1e-6);
        assert_eq!(curve_at(&c, 9.0), 0.5);
        assert_eq!(curve_at(&[], 3.0), 1.0);
    }

    #[test]
    fn swell_follows_the_drawn_loudness() {
        let v = Verlauf::default(); // Anschwellen
        let x = render_synth(&chord(3.0), 3.0, SR, &SynthParams::default(), &v, 0.5);
        assert!(x.iter().all(|s| s.is_finite()));
        let at = |t: f32| rms(&x[(t * SR) as usize..((t + 0.1) * SR) as usize]);
        assert!(at(0.05) < 0.3 * at(2.5), "leise Anfang {} → {}", at(0.05), at(2.5));
        // nach dem Loslassen klingt es aus
        assert!(at(4.5) < 0.1 * at(2.5));
        let bell = VERLAUF_PRESETS.iter().find(|p| p.name == "Glocke").unwrap().verlauf();
        let y = render_synth(&chord(3.0), 3.0, SR, &SynthParams::default(), &bell, 0.5);
        let bt = |t: f32| rms(&y[(t * SR) as usize..((t + 0.1) * SR) as usize]);
        assert!(bt(0.02) > 2.0 * bt(2.5), "Glocke: laut am Anfang");
    }

    #[test]
    fn brightness_curve_opens_the_filter() {
        let mut v = Verlauf::default();
        v.laut = vec![[0.0, 1.0]];
        v.hell = vec![[0.0, 0.0], [2.0, 1.0]];
        let x = render_synth(&chord(2.2), 2.2, SR, &SynthParams::default(), &v, 0.5);
        let early = analyze_sound(&x[(0.1 * SR) as usize..(0.5 * SR) as usize], SR).centroid_hz;
        let late = analyze_sound(&x[(1.6 * SR) as usize..(2.0 * SR) as usize], SR).centroid_hz;
        assert!(late > early * 1.8, "dunkel {early:.0} Hz → hell {late:.0} Hz");
    }

    #[test]
    fn pulse_chops_in_time() {
        let gatev = VERLAUF_PRESETS.iter().find(|p| p.name == "Gate").unwrap().verlauf();
        let spb = 0.5; // 120 BPM → Sechzehntel = 125 ms
        let x = render_synth(&chord(2.0), 2.0, SR, &SynthParams::default(), &gatev, spb);
        let per = 0.125;
        let on = rms(&x[((1.0 + 0.1 * per) * SR) as usize..((1.0 + 0.4 * per) * SR) as usize]);
        let off = rms(&x[((1.0 + 0.6 * per) * SR) as usize..((1.0 + 0.9 * per) * SR) as usize]);
        assert!(off < 0.1 * on, "an {on} / aus {off}");
    }

    #[test]
    fn more_voices_and_detune_sound_wider() {
        let v = VerlaufPreset::verlauf(&VERLAUF_PRESETS[3]);
        let thin = SynthParams { stimmen: 0.0, verstimmung: 0.0, ..Default::default() };
        let wide = SynthParams { stimmen: 1.0, verstimmung: 1.0, ..Default::default() };
        let env = |p: &SynthParams| -> f32 {
            let x = render_synth(&[NoteSpec::new(0.0, 2.0, 57.0, 0.9)], 2.0, SR, p, &v, 0.5);
            let blocks: Vec<f32> = x[(1.0 * SR) as usize..(1.9 * SR) as usize].chunks(480).map(rms).collect();
            let (lo, hi) = blocks.iter().fold((f32::MAX, 0f32), |(a, b), &v| (a.min(v), b.max(v)));
            (hi - lo) / hi.max(1e-6)
        };
        assert!(env(&wide) > env(&thin) + 0.05, "Schwebung durch Verstimmung");
    }
}
