//! Tonarten und Bereinigung aufgenommener Noten.

use serde::{Deserialize, Serialize};

const MAJOR_PROFILE: [f64; 12] = [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88];
const MINOR_PROFILE: [f64; 12] = [6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17];
const MAJOR_SCALE: [u8; 7] = [0, 2, 4, 5, 7, 9, 11];
/// Natürliches Moll plus Leitton (harmonisches Moll).
const MINOR_SCALE: [u8; 8] = [0, 2, 3, 5, 7, 8, 10, 11];

const NAMES_MAJOR: [&str; 12] = ["C", "Des", "D", "Es", "E", "F", "Fis", "G", "As", "A", "B", "H"];
const NAMES_MINOR: [&str; 12] = ["c", "cis", "d", "es", "e", "f", "fis", "g", "gis", "a", "b", "h"];

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Key {
    /// Grundton als Tonhöhenklasse (0 = C).
    pub tonic: u8,
    pub minor: bool,
    /// Korrelation mit dem Tonartprofil (−1…1); bei Vorzeichen aus der Partitur 1.
    #[serde(default)]
    pub confidence: f32,
}

impl Key {
    /// Aus MusicXML-Vorzeichen (`<fifths>`) und Modus.
    pub fn from_fifths(fifths: i8, minor: bool) -> Self {
        let major_tonic = (fifths as i32 * 7).rem_euclid(12) as u8;
        let tonic = if minor { (major_tonic + 9) % 12 } else { major_tonic };
        Self { tonic, minor, confidence: 1.0 }
    }

    /// Deutsche Schreibweise, z. B. „Es-Dur“, „fis-Moll“.
    pub fn name_de(&self) -> String {
        if self.minor {
            format!("{}-Moll", NAMES_MINOR[self.tonic as usize % 12])
        } else {
            format!("{}-Dur", NAMES_MAJOR[self.tonic as usize % 12])
        }
    }

    pub fn contains(&self, midi: u8) -> bool {
        let pc = (midi as i32 - self.tonic as i32).rem_euclid(12) as u8;
        if self.minor {
            MINOR_SCALE.contains(&pc)
        } else {
            MAJOR_SCALE.contains(&pc)
        }
    }

    /// Tonhöhenklasse der Dominante.
    pub fn dominant(&self) -> u8 {
        (self.tonic + 7) % 12
    }
}

fn pearson(a: &[f64; 12], b: &[f64; 12]) -> f64 {
    let ma = a.iter().sum::<f64>() / 12.0;
    let mb = b.iter().sum::<f64>() / 12.0;
    let (mut num, mut da, mut db) = (0.0, 0.0, 0.0);
    for i in 0..12 {
        num += (a[i] - ma) * (b[i] - mb);
        da += (a[i] - ma).powi(2);
        db += (b[i] - mb).powi(2);
    }
    if da <= 0.0 || db <= 0.0 {
        0.0
    } else {
        num / (da.sqrt() * db.sqrt())
    }
}

/// Krumhansl-Kessler: Histogramm der Tonhöhenklassen (z. B. dauer-gewichtet).
/// Vorzeichen ohne Angabe des Tongeschlechts (MuseScore lässt `<mode>` oft
/// weg): Dur oder parallele Molltonart nach Tonverteilung (Krumhansl) und
/// Schlusston im Bass.
pub fn key_from_fifths_guess(fifths: i8, hist: &[f64; 12], final_bass_pc: Option<u8>) -> Key {
    let major = Key::from_fifths(fifths, false);
    let minor = Key::from_fifths(fifths, true);
    let fit = |k: Key, profile: &[f64; 12]| {
        let mut rotated = [0.0; 12];
        for i in 0..12 {
            rotated[(i + k.tonic as usize) % 12] = profile[i];
        }
        pearson(hist, &rotated) + if final_bass_pc == Some(k.tonic) { 0.25 } else { 0.0 }
    };
    if hist.iter().sum::<f64>() > 0.0 && fit(minor, &MINOR_PROFILE) > fit(major, &MAJOR_PROFILE) {
        minor
    } else {
        major
    }
}

pub fn estimate_key_from_histogram(hist: &[f64; 12]) -> Option<Key> {
    if hist.iter().sum::<f64>() <= 0.0 {
        return None;
    }
    let mut best: Option<Key> = None;
    for tonic in 0..12u8 {
        for (minor, profile) in [(false, &MAJOR_PROFILE), (true, &MINOR_PROFILE)] {
            let mut rotated = [0.0; 12];
            for i in 0..12 {
                rotated[(i + tonic as usize) % 12] = profile[i];
            }
            let r = pearson(hist, &rotated) as f32;
            if best.map(|b| r > b.confidence).unwrap_or(true) {
                best = Some(Key { tonic, minor, confidence: r });
            }
        }
    }
    best
}

/// Note aus einer Aufnahme (Zeiten in Sekunden).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimedNote {
    pub start: f64,
    pub end: f64,
    pub midi: u8,
    /// Gemessene Abweichung vom MIDI-Ton in Cent.
    pub cents: f32,
    pub velocity: f32,
}

pub fn estimate_key(notes: &[TimedNote]) -> Option<Key> {
    if notes.len() < 4 {
        return None;
    }
    let mut hist = [0.0; 12];
    for n in notes {
        hist[n.midi as usize % 12] += (n.end - n.start).max(0.0);
    }
    estimate_key_from_histogram(&hist)
}

/// Leiterfremde Töne, deren gemessene Tonhöhe in Richtung eines Leitertons
/// abweicht (≥ 20 Cent), werden auf diesen gezogen – typisch für leicht
/// unsauber gesungene Töne. Gewollte Chromatik (sauber intoniert) bleibt.
pub fn snap_to_key(notes: &[TimedNote], key: &Key) -> Vec<TimedNote> {
    notes
        .iter()
        .map(|n| {
            if key.contains(n.midi) || n.cents.abs() < 20.0 {
                return *n;
            }
            let target = if n.cents > 0.0 { n.midi.saturating_add(1) } else { n.midi.saturating_sub(1) };
            if key.contains(target) {
                let shift = if n.cents > 0.0 { -100.0 } else { 100.0 };
                TimedNote { midi: target, cents: n.cents + shift, ..*n }
            } else {
                *n
            }
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuantizedNote {
    pub start_beats: f64,
    pub length_beats: f64,
    pub midi: u8,
    pub velocity: f32,
}

/// Rastet Anfänge und Längen auf ein Raster (in Schlägen) ein und
/// entfernt Überlappungen.
pub fn quantize(notes: &[TimedNote], seconds_per_beat: f64, grid_beats: f64) -> Vec<QuantizedNote> {
    let snap = |b: f64| (b / grid_beats).round() * grid_beats;
    let mut out: Vec<QuantizedNote> = notes
        .iter()
        .map(|n| {
            let start = snap(n.start / seconds_per_beat).max(0.0);
            let end = snap(n.end / seconds_per_beat).max(start + grid_beats);
            QuantizedNote { start_beats: start, length_beats: end - start, midi: n.midi, velocity: n.velocity }
        })
        .collect();
    out.sort_by(|a, b| a.start_beats.total_cmp(&b.start_beats));
    // Gleiche Startposition: lauteste Note behalten.
    out.dedup_by(|b, a| {
        if (a.start_beats - b.start_beats).abs() < 1e-9 {
            if b.velocity > a.velocity {
                *a = *b;
            }
            true
        } else {
            false
        }
    });
    for i in 0..out.len().saturating_sub(1) {
        let next = out[i + 1].start_beats;
        if out[i].start_beats + out[i].length_beats > next {
            out[i].length_beats = next - out[i].start_beats;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tn(start: f64, end: f64, midi: u8, cents: f32) -> TimedNote {
        TimedNote { start, end, midi, cents, velocity: 0.8 }
    }

    #[test]
    fn key_names_and_fifths() {
        assert_eq!(Key::from_fifths(-3, false).name_de(), "Es-Dur");
        assert_eq!(Key::from_fifths(3, true).name_de(), "fis-Moll");
        assert_eq!(Key::from_fifths(0, true).name_de(), "a-Moll");
        assert_eq!(Key::from_fifths(5, false).name_de(), "H-Dur");
    }

    #[test]
    fn guesses_major_or_relative_minor_without_mode() {
        // a-Moll: viel A, C, E, dazu Gis als Leitton, Schluss auf A
        let mut minor = [0.0f64; 12];
        for (pc, d) in [(9, 6.0), (0, 3.0), (4, 4.0), (8, 1.5), (11, 1.0), (2, 1.5), (5, 1.0)] {
            minor[pc] = d;
        }
        assert_eq!(key_from_fifths_guess(0, &minor, Some(9)).name_de(), "a-Moll");
        // C-Dur: viel C, E, G, Schluss auf C
        let mut major = [0.0f64; 12];
        for (pc, d) in [(0, 6.0), (4, 4.0), (7, 5.0), (2, 2.0), (5, 1.5), (9, 1.5), (11, 1.5)] {
            major[pc] = d;
        }
        assert_eq!(key_from_fifths_guess(0, &major, Some(0)).name_de(), "C-Dur");
        assert_eq!(key_from_fifths_guess(1, &[0.0; 12], None).name_de(), "G-Dur", "ohne Töne: Dur");
    }

    #[test]
    fn detects_g_major_scale() {
        let scale = [67, 69, 71, 72, 74, 76, 78, 79, 74, 71, 67];
        let notes: Vec<TimedNote> = scale
            .iter()
            .enumerate()
            .map(|(i, &m)| tn(i as f64 * 0.5, i as f64 * 0.5 + if m % 12 == 7 { 1.0 } else { 0.4 }, m, 0.0))
            .collect();
        let k = estimate_key(&notes).unwrap();
        assert_eq!((k.tonic, k.minor), (7, false), "{}", k.name_de());
    }

    #[test]
    fn snaps_sloppy_notes_but_keeps_clean_chromatic_ones() {
        let key = Key::from_fifths(0, false);
        let out = snap_to_key(&[tn(0.0, 0.5, 61, -35.0), tn(0.5, 1.0, 66, 3.0)], &key);
        assert_eq!(out[0].midi, 60);
        assert_eq!(out[1].midi, 66);
    }

    #[test]
    fn quantizes_to_sixteenths() {
        let spb = 0.5; // 120 BPM
        let q = quantize(&[tn(0.02, 0.48, 60, 0.0), tn(0.51, 0.77, 62, 0.0), tn(0.74, 1.3, 64, 0.0)], spb, 0.25);
        assert_eq!(q.len(), 3);
        assert_eq!((q[0].start_beats, q[0].length_beats), (0.0, 1.0));
        assert_eq!((q[1].start_beats, q[1].length_beats), (1.0, 0.5));
        assert_eq!((q[2].start_beats, q[2].length_beats), (1.5, 1.0));
    }
}
