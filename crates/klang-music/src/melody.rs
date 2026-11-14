//! Melody lines and their features.
//!
//! A melody is described as a sequence of transitions:
//! * interval in semitones → independent of the key (transposition)
//! * ratio of consecutive note spacings (IOI) → independent of the tempo
//!
//! This way a hummed melody finds its original, regardless of the register
//! and how fast it was hummed.

use crate::score::ScoreNote;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MelodyNote {
    /// Onset in an arbitrary time unit (quarters or seconds – only ratios count).
    pub onset: f64,
    pub duration: f64,
    pub midi: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Feature {
    /// Interval to the next note, limited to ±24.
    pub interval: i8,
    /// round(2·log2(IOI_new / IOI_old)), limited to ±4 (half octaves of the duration).
    pub rhythm: i8,
}

impl Feature {
    /// Rough contour: large/small step down/up or repeated note.
    pub fn contour(self) -> i8 {
        match self.interval {
            i if i <= -3 => -2,
            -2..=-1 => -1,
            0 => 0,
            1..=2 => 1,
            _ => 2,
        }
    }
}

/// Top voice: the highest note per onset time (notes sorted by onset).
pub fn skyline(notes: &[ScoreNote]) -> Vec<MelodyNote> {
    let mut out: Vec<MelodyNote> = Vec::with_capacity(notes.len());
    for n in notes {
        match out.last_mut() {
            Some(last) if (last.onset - n.onset).abs() < 1e-6 => {
                if n.midi > last.midi {
                    last.midi = n.midi;
                    last.duration = n.duration;
                }
            }
            _ => out.push(MelodyNote { onset: n.onset, duration: n.duration, midi: n.midi }),
        }
    }
    out
}

pub fn features(notes: &[MelodyNote]) -> Vec<Feature> {
    let n = notes.len();
    if n < 2 {
        return Vec::new();
    }
    let ioi: Vec<f64> = (0..n)
        .map(|i| {
            let d = if i + 1 < n { notes[i + 1].onset - notes[i].onset } else { notes[i].duration };
            d.max(1e-6)
        })
        .collect();
    (0..n - 1)
        .map(|i| Feature {
            interval: (notes[i + 1].midi as i16 - notes[i].midi as i16).clamp(-24, 24) as i8,
            rhythm: (2.0 * (ioi[i + 1] / ioi[i]).log2()).round().clamp(-4.0, 4.0) as i8,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mel(notes: &[(f64, f64, u8)]) -> Vec<MelodyNote> {
        notes.iter().map(|&(onset, duration, midi)| MelodyNote { onset, duration, midi }).collect()
    }

    #[test]
    fn features_are_transposition_and_tempo_invariant() {
        let a = mel(&[(0.0, 1.0, 60), (1.0, 1.0, 62), (2.0, 2.0, 64), (4.0, 1.0, 62)]);
        let b = mel(&[(0.0, 0.5, 67), (0.5, 0.5, 69), (1.0, 1.0, 71), (2.0, 0.5, 69)]);
        assert_eq!(features(&a), features(&b));
        assert_eq!(features(&a)[1], Feature { interval: 2, rhythm: 2 });
    }

    #[test]
    fn skyline_takes_top_note() {
        let notes = [
            ScoreNote { onset: 0.0, duration: 1.0, midi: 60 },
            ScoreNote { onset: 0.0, duration: 1.0, midi: 67 },
            ScoreNote { onset: 1.0, duration: 1.0, midi: 65 },
        ];
        let s = skyline(&notes);
        assert_eq!(s.iter().map(|n| n.midi).collect::<Vec<_>>(), vec![67, 65]);
    }
}
