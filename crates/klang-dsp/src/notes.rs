//! Individual notes are derived from a pitch curve (segmentation).
//!
//! Approach: median smoothing of the MIDI curve, then a new note at
//! silence, at a pitch jump or at a detected onset
//! (repeated identical notes). Fragments that are too short are discarded.

use crate::pitch::PitchFrame;
use crate::util::median;
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteEvent {
    pub start: f32,
    pub end: f32,
    pub midi: u8,
    /// Deviation of the measured pitch from the rounded MIDI note.
    pub cents: f32,
    /// 0…1, relative to the loudest point of the recording.
    pub velocity: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct SegmentConfig {
    pub min_note_secs: f32,
    pub split_semitones: f32,
    pub median_frames: usize,
    pub min_clarity: f32,
}

impl Default for SegmentConfig {
    fn default() -> Self {
        Self {
            min_note_secs: 0.08,
            split_semitones: 0.6,
            median_frames: 5,
            min_clarity: 0.5,
        }
    }
}

pub fn hz_to_midi(hz: f32) -> f32 {
    69.0 + 12.0 * (hz / 440.0).log2()
}

pub fn midi_to_hz(midi: f32) -> f32 {
    440.0 * 2f32.powf((midi - 69.0) / 12.0)
}

/// `onsets` must be sorted in ascending order (seconds).
pub fn segment_notes(frames: &[PitchFrame], onsets: &[f32], cfg: &SegmentConfig) -> Vec<NoteEvent> {
    if frames.len() < 2 {
        return Vec::new();
    }
    let hop = (frames[1].time - frames[0].time).max(1e-4);

    let raw: Vec<Option<f32>> = frames
        .iter()
        .map(|f| match f.freq {
            Some(hz) if f.clarity >= cfg.min_clarity => Some(hz_to_midi(hz)),
            _ => None,
        })
        .collect();

    let half = cfg.median_frames / 2;
    let mut scratch = Vec::with_capacity(cfg.median_frames + 1);
    let smooth: Vec<Option<f32>> = (0..raw.len())
        .map(|i| {
            raw[i]?;
            scratch.clear();
            let lo = i.saturating_sub(half);
            let hi = (i + half + 1).min(raw.len());
            scratch.extend(raw[lo..hi].iter().flatten().copied());
            Some(median(&mut scratch))
        })
        .collect();

    let max_rms = frames.iter().fold(0.0f32, |m, f| m.max(f.rms)).max(1e-9);
    let ctx = Ctx { frames, smooth: &smooth, hop, max_rms, cfg };

    let mut notes = Vec::new();
    let mut seg: Vec<usize> = Vec::new();
    let mut onset_idx = 0;
    for i in 0..frames.len() {
        let t = frames[i].time;
        let mut onset_here = false;
        while onset_idx < onsets.len() && onsets[onset_idx] <= t {
            onset_here = true;
            onset_idx += 1;
        }
        match smooth[i] {
            None => ctx.flush(&mut seg, &mut notes),
            Some(m) => {
                if let Some(&first) = seg.first() {
                    let mut recent: Vec<f32> =
                        seg.iter().rev().take(8).filter_map(|&j| smooth[j]).collect();
                    let center = median(&mut recent);
                    let dur = (i - first) as f32 * hop;
                    if (m - center).abs() > cfg.split_semitones
                        || (onset_here && dur >= cfg.min_note_secs && ctx.has_dip(&seg, i))
                    {
                        ctx.flush(&mut seg, &mut notes);
                    }
                }
                seg.push(i);
            }
        }
    }
    ctx.flush(&mut seg, &mut notes);
    notes
}

struct Ctx<'a> {
    frames: &'a [PitchFrame],
    smooth: &'a [Option<f32>],
    hop: f32,
    max_rms: f32,
    cfg: &'a SegmentConfig,
}

impl Ctx<'_> {
    /// True re-striking of the same note: around the onset the
    /// volume falls clearly below the typical level of the running note.
    /// Vibrato or timbre changes do produce flux peaks, but no dip.
    fn has_dip(&self, seg: &[usize], i: usize) -> bool {
        let k = ((0.04 / self.hop).ceil() as usize).max(1);
        let lo = i.saturating_sub(k);
        let hi = (i + k + 1).min(self.frames.len());
        let dip = self.frames[lo..hi].iter().map(|f| f.rms).fold(f32::MAX, f32::min);
        let mut levels: Vec<f32> = seg.iter().map(|&j| self.frames[j].rms).collect();
        dip < 0.6 * median(&mut levels)
    }

    fn flush(&self, seg: &mut Vec<usize>, out: &mut Vec<NoteEvent>) {
        let (Some(&first), Some(&last)) = (seg.first(), seg.last()) else {
            return;
        };
        let start = self.frames[first].time - self.hop * 0.5;
        let end = self.frames[last].time + self.hop * 0.5;
        if end - start >= self.cfg.min_note_secs {
            let mut vals: Vec<f32> = seg.iter().filter_map(|&j| self.smooth[j]).collect();
            let m = median(&mut vals);
            let midi = m.round().clamp(0.0, 127.0);
            let rms = seg.iter().map(|&j| self.frames[j].rms).sum::<f32>() / seg.len() as f32;
            out.push(NoteEvent {
                start: start.max(0.0),
                end,
                midi: midi as u8,
                cents: (m - midi) * 100.0,
                velocity: (rms / self.max_rms).sqrt().clamp(0.05, 1.0),
            });
        }
        seg.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pitch::{detect_pitch, YinConfig};
    use crate::testutil::sine;

    #[test]
    fn midi_conversion() {
        assert!((hz_to_midi(440.0) - 69.0).abs() < 1e-4);
        assert!((midi_to_hz(60.0) - 261.63).abs() < 0.01);
    }

    #[test]
    fn segments_a_little_melody() {
        let sr = 44_100.0;
        let mut x = Vec::new();
        for &f in &[220.0f32, 246.94, 261.63, 220.0] {
            x.extend(sine(f, 0.3, sr, 0.4));
            x.extend(vec![0.0; (0.05 * sr) as usize]);
        }
        let frames = detect_pitch(&x, sr, &YinConfig::default());
        let notes = segment_notes(&frames, &[], &SegmentConfig::default());
        let midis: Vec<u8> = notes.iter().map(|n| n.midi).collect();
        assert_eq!(midis, vec![57, 59, 60, 57]);
        for n in &notes {
            assert!(n.cents.abs() < 20.0);
            assert!((n.end - n.start - 0.3).abs() < 0.08, "{n:?}");
        }
    }

    #[test]
    fn onsets_split_repeated_notes_only_when_reattacked() {
        let sr = 44_100.0;
        // Same note, briefly quieter at 0.5 s (re-struck).
        let mut x = sine(330.0, 1.0, sr, 0.4);
        for (i, v) in x.iter_mut().enumerate() {
            let t = i as f32 / sr;
            let d = ((t - 0.5).abs() / 0.03).min(1.0);
            *v *= 0.1 + 0.9 * d;
        }
        let frames = detect_pitch(&x, sr, &YinConfig::default());
        let notes = segment_notes(&frames, &[0.5], &SegmentConfig::default());
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert!(notes.iter().all(|n| n.midi == 64));

        // Without a volume dip (e.g. vibrato peak in the flux) it stays one note.
        let steady = sine(330.0, 1.0, sr, 0.4);
        let frames = detect_pitch(&steady, sr, &YinConfig::default());
        assert_eq!(segment_notes(&frames, &[0.5], &SegmentConfig::default()).len(), 1);
    }
}
