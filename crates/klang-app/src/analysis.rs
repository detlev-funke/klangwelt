//! Recording analysis: pitch curve → notes → key → cleanup.

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
    /// Cleaned-up note (pulled into the key if sung out of tune).
    pub midi: u8,
    /// Originally detected note.
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
    /// (time, MIDI pitch or null)
    pub pitch: Vec<(f32, Option<f32>)>,
    pub notes: Vec<AnalyzedNote>,
    pub onsets: Vec<f32>,
    /// Fundamental per hit (drums, bass, plucked notes); `midi: null` = unpitched.
    pub hits: Vec<HitPitch>,
    pub tempo_estimate: Option<f32>,
    pub key: Option<KeyInfo>,
}

pub fn analyze(asset_id: &str, samples: &[f32], sample_rate: u32) -> Analysis {
    analyze_with(asset_id, samples, sample_rate, &YinConfig::default())
}

/// Melody sung or whistled for the library search: notes
/// (duration in seconds, cleaned up to the key) and the estimated key.
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
        // name for display (in the language chosen at the time of the analysis)
        key: key.map(|k| KeyInfo { name: k.name(), tonic: k.tonic, minor: k.minor, confidence: k.confidence }),
    }
}

// ---------------------------------------------------------------- While recording

/// Notes before the key is estimated live (fewer give erratic guesses).
const LIVE_KEY_MIN_NOTES: usize = 6;
/// The live tempo looks at this many seconds and needs this many onsets in them
/// (with fewer, the estimate jumps around).
const LIVE_TEMPO_WINDOW_SECS: f32 = 20.0;
const LIVE_TEMPO_MIN_ONSETS: usize = 6;
/// The tempo is estimated anew after this many seconds of new audio …
const LIVE_TEMPO_EVERY_SECS: f64 = 0.5;
/// … and shown as the median of the last estimates (one octave error among
/// them doesn't show).
const LIVE_TEMPO_MEDIAN_OF: usize = 3;

/// Analysis while recording. The shells' regular `tick` feeds it with the
/// new samples; the UI fetches what is new with [`LiveAnalysis::view`].
///
/// Built from the same parts as [`analyze`] – pitch frames
/// ([`klang_dsp::tracker`]), note segmentation, key and tempo – but
/// measured as the recording comes in: a sketch of what the finished
/// analysis will find, which replaces it after the recording.
pub struct LiveAnalysis {
    /// Number of the recording (the UI starts afresh with a new one).
    take: u32,
    tracker: klang_dsp::tracker::Tracker,
    /// Notes that can't change any more: a silence follows them.
    settled: Vec<TimedNote>,
    /// Pitch frames from here on are segmented again with every update.
    cut: usize,
    /// Notes after the cut – the running phrase may still change them.
    pending: Vec<TimedNote>,
    tempo: Option<f32>,
    /// The last estimates (the shown tempo is their median).
    tempo_recent: Vec<f32>,
    /// Recorded seconds at the last tempo estimate.
    tempo_at: f64,
    key: Option<klang_music::theory::Key>,
}

/// What the UI already has of the running analysis: number of the recording
/// and how many curve points, settled notes and onsets.
#[derive(Clone, Copy, Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LiveCursor {
    pub take: u32,
    pub curve: usize,
    pub notes: usize,
    pub onsets: usize,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LiveNote {
    pub start_secs: f64,
    pub end_secs: f64,
    pub midi: u8,
    pub cents: f32,
}

/// The running analysis for the UI – the growing parts from the cursor on.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveView {
    pub take: u32,
    /// Recorded seconds so far.
    pub seconds: f64,
    /// Level of the newest samples (about 50 ms): RMS in dBFS and peak (0…1).
    pub level_db: f32,
    pub peak: f32,
    /// What sounds right now: pitch as MIDI with fraction, the nearest note
    /// and the deviation from it in cents – null when nothing clear sounds.
    pub pitch: Option<f32>,
    pub note: Option<u8>,
    pub cents: Option<f32>,
    /// Pitch curve (time, MIDI or null) from point number `curve_from` on.
    pub curve_from: usize,
    pub curve: Vec<(f32, Option<f32>)>,
    /// Notes that won't change any more, from number `notes_from` on.
    pub notes_from: usize,
    pub notes: Vec<LiveNote>,
    /// Notes of the running phrase (may still change) – always all of them.
    pub pending: Vec<LiveNote>,
    /// Onsets (seconds) from number `onsets_from` on.
    pub onsets_from: usize,
    pub onsets: Vec<f32>,
    pub tempo_estimate: Option<f32>,
    pub key: Option<KeyInfo>,
}

fn round2(v: f32) -> f32 {
    (v * 100.0).round() / 100.0
}

fn live_note(n: &TimedNote) -> LiveNote {
    LiveNote { start_secs: (n.start * 1000.0).round() / 1000.0, end_secs: (n.end * 1000.0).round() / 1000.0, midi: n.midi, cents: n.cents.round() }
}

impl LiveAnalysis {
    pub fn new(take: u32, sample_rate: u32) -> Self {
        Self {
            take,
            tracker: klang_dsp::tracker::Tracker::new(sample_rate),
            settled: Vec::new(),
            cut: 0,
            pending: Vec::new(),
            tempo: None,
            tempo_recent: Vec::new(),
            tempo_at: 0.0,
            key: None,
        }
    }

    pub fn take(&self) -> u32 {
        self.take
    }

    /// Measures new samples (mono, at the rate given at the start).
    pub fn push(&mut self, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }
        self.tracker.push(samples);
        self.segment();
        let notes = self.settled.len() + self.pending.len();
        self.key = if notes >= LIVE_KEY_MIN_NOTES {
            let all: Vec<TimedNote> = self.settled.iter().chain(&self.pending).copied().collect();
            estimate_key(&all)
        } else {
            None
        };
        let now = self.tracker.seconds();
        if now - self.tempo_at >= LIVE_TEMPO_EVERY_SECS {
            self.tempo_at = now;
            match self.tracker.tempo(LIVE_TEMPO_WINDOW_SECS, LIVE_TEMPO_MIN_ONSETS) {
                Some(t) => {
                    self.tempo_recent.push(t);
                    if self.tempo_recent.len() > LIVE_TEMPO_MEDIAN_OF {
                        self.tempo_recent.remove(0);
                    }
                }
                // no steady beat (any more)
                None => self.tempo_recent.clear(),
            }
            self.tempo = (self.tempo_recent.len() == LIVE_TEMPO_MEDIAN_OF).then(|| klang_dsp::util::median(&mut self.tempo_recent.clone()));
        }
    }

    /// Segments the frames after the cut into notes (as the finished
    /// analysis does). Notes before the last silence that has enough frames
    /// after it for the smoothing can't change any more: they are settled
    /// and the cut moves behind that silence.
    fn segment(&mut self) {
        let cfg = SegmentConfig::default();
        let frames = &self.tracker.frames()[self.cut..];
        if frames.len() < 2 {
            self.pending.clear();
            return;
        }
        let onsets = self.tracker.onsets();
        let from = onsets.partition_point(|&t| t < frames[0].time);
        let notes: Vec<TimedNote> = segment_notes(frames, &onsets[from..], &cfg)
            .iter()
            .map(|n| TimedNote { start: n.start as f64, end: n.end as f64, midi: n.midi, cents: n.cents, velocity: n.velocity })
            .collect();
        let unvoiced = |f: &klang_dsp::pitch::PitchFrame| f.freq.is_none() || f.clarity < cfg.min_clarity;
        let safe = frames.len().saturating_sub(cfg.median_frames / 2 + 1);
        match (0..safe).rev().find(|&j| unvoiced(&frames[j])) {
            Some(j) => {
                let cut_time = frames[j].time as f64;
                let (done, open): (Vec<TimedNote>, Vec<TimedNote>) = notes.into_iter().partition(|n| n.end <= cut_time);
                self.settled.extend(done);
                self.pending = open;
                self.cut += j + 1;
            }
            None => self.pending = notes,
        }
    }

    /// What sounds right now: the median of the newest clear frames, as
    /// long as the newest frame is clear itself.
    fn current_pitch(&self) -> Option<f32> {
        let min_clarity = SegmentConfig::default().min_clarity;
        let frames = self.tracker.frames();
        let clear = |f: &klang_dsp::pitch::PitchFrame| f.freq.filter(|_| f.clarity >= min_clarity).map(hz_to_midi);
        clear(frames.last()?)?;
        let mut recent: Vec<f32> = frames.iter().rev().take(5).filter_map(clear).collect();
        Some(klang_dsp::util::median(&mut recent))
    }

    /// The analysis so far; the growing parts from `since` on (from the
    /// start when `since` belongs to another recording).
    pub fn view(&self, since: LiveCursor) -> LiveView {
        let since = if since.take == self.take { since } else { LiveCursor::default() };
        let frames = self.tracker.frames();
        let curve_from = since.curve.min(frames.len());
        let notes_from = since.notes.min(self.settled.len());
        let onsets = self.tracker.onsets();
        let onsets_from = since.onsets.min(onsets.len());
        let (rms, peak) = self.tracker.level();
        let pitch = self.current_pitch();
        let note = pitch.map(|m| m.round().clamp(0.0, 127.0) as u8);
        LiveView {
            take: self.take,
            seconds: (self.tracker.seconds() * 1000.0).round() / 1000.0,
            level_db: round2(klang_dsp::util::gain_to_db(rms).max(-120.0)),
            peak: round2(peak.min(1.0)),
            pitch: pitch.map(round2),
            note,
            cents: pitch.zip(note).map(|(m, n)| ((m - n as f32) * 100.0).round()),
            curve_from,
            curve: frames[curve_from..].iter().map(|f| ((f.time * 1000.0).round() / 1000.0, f.freq.map(|hz| round2(hz_to_midi(hz))))).collect(),
            notes_from,
            notes: self.settled[notes_from..].iter().map(live_note).collect(),
            pending: self.pending.iter().map(live_note).collect(),
            onsets_from,
            onsets: onsets[onsets_from..].iter().map(|t| (t * 1000.0).round() / 1000.0).collect(),
            tempo_estimate: self.tempo.map(|t| (t * 10.0).round() / 10.0),
            key: self.key.map(|k| KeyInfo { name: k.name(), tonic: k.tonic, minor: k.minor, confidence: k.confidence }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "Sung" melody: fundamental + overtones, vibrato, rests, one out-of-tune note.
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
        // C major melody; the 6th note is a c'' (72) sung too low, measured ≈ 72.62 → C♯.
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
        assert_eq!(key.name, "C major");
        assert!(a.pitch.iter().any(|p| p.1.is_some()));
    }

    const MELODY: [(f32, f32); 12] = [
        (60.0, 0.35), (64.0, 0.35), (67.0, 0.35), (72.0, 0.5), (71.0, 0.35), (72.62, 0.35),
        (67.0, 0.35), (69.0, 0.35), (65.0, 0.35), (64.0, 0.35), (62.0, 0.35), (60.0, 0.7),
    ];

    /// Feeds `x` in pieces, as the shells do every few dozen milliseconds.
    fn live(x: &[f32], sr: u32, chunk: usize) -> LiveAnalysis {
        let mut l = LiveAnalysis::new(1, sr);
        for c in x.chunks(chunk) {
            l.push(c);
        }
        l
    }

    #[test]
    fn live_analysis_finds_the_notes_and_key_of_the_finished_one() {
        let sr = 44_100;
        let x = sing(&MELODY, sr as f32);
        let done = analyze("t", &x, sr);
        let l = live(&x, sr, 2048);
        let v = l.view(LiveCursor::default());
        let notes: Vec<u8> = v.notes.iter().chain(&v.pending).map(|n| n.midi).collect();
        // the finished analysis pulls the sloppy C♯ into the key, live shows what was sung
        let raw: Vec<u8> = done.notes.iter().map(|n| n.raw_midi).collect();
        assert_eq!(notes, raw, "{:?}", v.notes);
        assert!(v.notes.len() >= 11, "all but the last note are settled after their rest: {}", v.notes.len());
        assert_eq!(v.key.as_ref().map(|k| k.name.as_str()), Some("C major"));
        assert!((v.seconds - x.len() as f64 / sr as f64).abs() < 1e-3);
        assert!(v.curve.iter().filter(|p| p.1.is_some()).count() > 300, "{}", v.curve.len());
    }

    #[test]
    fn live_view_reports_the_current_note_and_only_what_is_new() {
        let sr = 48_000;
        // A4 sung 20 cents sharp: tuner shows A4 +20
        let x = sing(&[(69.2, 1.0)], sr as f32);
        let mut l = LiveAnalysis::new(7, sr);
        l.push(&x[..(0.6 * sr as f32) as usize]);
        let v = l.view(LiveCursor::default());
        assert_eq!(v.note, Some(69));
        let c = v.cents.unwrap();
        assert!((c - 20.0).abs() <= 8.0, "{c}");
        assert!(v.level_db > -20.0 && v.level_db < 0.0 && v.peak > 0.3, "{} {}", v.level_db, v.peak);
        assert!(v.tempo_estimate.is_none() && v.key.is_none(), "too little material");

        let cursor = LiveCursor { take: 7, curve: v.curve.len(), notes: v.notes.len(), onsets: v.onsets.len() };
        l.push(&x[(0.6 * sr as f32) as usize..]);
        l.push(&vec![0.0; sr as usize / 5]);
        let w = l.view(cursor);
        assert_eq!(w.curve_from, v.curve.len());
        assert!(!w.curve.is_empty(), "the new points");
        assert!(w.curve[0].0 > v.curve.last().unwrap().0);
        // silence at the end: nothing sounds any more, the note is settled
        assert_eq!(w.pitch, None);
        assert_eq!((w.notes.len(), w.pending.len()), (1, 0), "{:?}", w.notes);
        assert_eq!(w.notes[0].midi, 69);
        // a cursor of another recording gets everything
        let all = l.view(LiveCursor { take: 6, ..cursor });
        assert_eq!(all.curve_from, 0);
        assert_eq!(all.curve.len(), v.curve.len() + w.curve.len());
    }

    #[test]
    fn live_tempo_needs_onsets_and_then_follows_the_beat() {
        let sr = 48_000;
        // a quarter note every 0.5 s (120 BPM), plucked
        let mut x = Vec::new();
        for i in 0..20 {
            let f = 440.0 * 2f32.powf(([0.0, 4.0, 7.0, 12.0][i % 4] - 9.0) / 12.0);
            for j in 0..sr / 2 {
                let t = j as f32 / sr as f32;
                x.push((-t * 4.0).exp() * (t / 0.004).min(1.0) * 0.5 * (2.0 * std::f32::consts::PI * f * t).sin());
            }
        }
        let mut l = LiveAnalysis::new(1, sr);
        l.push(&x[..sr as usize]);
        assert!(l.view(LiveCursor::default()).tempo_estimate.is_none(), "two beats are not enough");
        for c in x[sr as usize..].chunks(1600) {
            l.push(c);
        }
        let bpm = l.view(LiveCursor::default()).tempo_estimate.expect("tempo");
        assert!((bpm - 120.0).abs() < 3.0, "{bpm}");
    }

    /// The melody of the end-to-end tests: C major, a plucked note every 0.5 s.
    fn e2e_melody(sr: u32) -> Vec<f32> {
        let mut x = Vec::new();
        for m in [60.0f32, 64.0, 67.0, 72.0, 67.0, 64.0, 65.0, 69.0, 67.0, 62.0, 59.0, 60.0] {
            let f = 440.0 * 2f32.powf((m - 69.0) / 12.0);
            for i in 0..sr / 2 {
                let t = i as f32 / sr as f32;
                let env = (t / 0.005).min(1.0) * (-t * 2.0).exp() * ((0.46 - t) / 0.02).clamp(0.0, 1.0);
                let w = 2.0 * std::f32::consts::PI * f * t;
                x.push(env * (0.4 * w.sin() + 0.15 * (2.0 * w).sin() + 0.06 * (3.0 * w).sin()));
            }
        }
        x
    }

    #[test]
    fn live_tempo_is_steady_from_the_first_estimate_on() {
        let sr = 48_000;
        let melody: Vec<f32> = e2e_melody(sr).repeat(3);
        // the recording starts anywhere in the melody, sometimes after a moment of silence
        for start in (0..12).map(|k| k * sr as usize / 4) {
            for lead in [0, sr as usize / 5] {
                let mut x = vec![0.0; lead];
                x.extend_from_slice(&melody[start..start + 12 * sr as usize]);
                let mut l = LiveAnalysis::new(1, sr);
                let mut shown = Vec::new();
                for c in x.chunks(2048) {
                    l.push(c);
                    if let Some(t) = l.view(LiveCursor::default()).tempo_estimate {
                        shown.push(((l.tracker.seconds() * 10.0).round() / 10.0, t));
                    }
                }
                let first = shown.first().expect("a tempo is shown");
                assert!(first.0 < 8.0, "shown after {} s", first.0);
                let off: Vec<_> = shown.iter().filter(|(_, t)| (t - 120.0).abs() > 4.0).collect();
                assert!(off.is_empty(), "start {start}, lead {lead}: {off:?}");
            }
        }
    }
}
