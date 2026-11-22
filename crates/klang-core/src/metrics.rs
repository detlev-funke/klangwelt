//! Key figures of a project – the history keeps them for every step.

use crate::model::{ClipSource, InstrumentKind, Project, TrackKind};
use serde::{Deserialize, Serialize};

/// What a project holds at one point in time: notes, length, tracks, tempo …
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Metrics {
    /// Notes in all clips.
    pub notes: u32,
    pub tracks: u32,
    pub clips: u32,
    /// Clips with recordings (audio).
    pub audio_clips: u32,
    /// End of the last clip in beats.
    pub length_beats: f64,
    pub tempo_bpm: f64,
    pub beats_per_bar: u32,
    pub beat_unit: u32,
    /// Lowest and highest pitch of all notes (MIDI), if there are notes.
    pub pitch_range: Option<(u8, u8)>,
    /// How long each pitch class sounds (C … B, in beats) on the audible melodic
    /// tracks – the app estimates the key from it.
    pub pitch_classes: [f32; 12],
}

impl Metrics {
    pub fn of(p: &Project) -> Self {
        let mut m = Metrics {
            tracks: p.tracks.len() as u32,
            tempo_bpm: p.tempo_bpm,
            beats_per_bar: p.beats_per_bar,
            beat_unit: p.beat_unit,
            ..Default::default()
        };
        let mut hist = [0.0f64; 12];
        for t in &p.tracks {
            let melodic = t.kind == TrackKind::Instrument
                && t.instrument.as_ref().map(|i| i.kind) != Some(InstrumentKind::Drums)
                && !p.is_muted(t);
            for c in &t.clips {
                m.clips += 1;
                m.length_beats = m.length_beats.max(c.start_beats + c.length_beats(p.tempo_bpm));
                match &c.source {
                    ClipSource::Audio { .. } => m.audio_clips += 1,
                    ClipSource::Notes { notes, .. } => {
                        m.notes += notes.len() as u32;
                        for n in notes {
                            m.pitch_range = Some(match m.pitch_range {
                                Some((lo, hi)) => (lo.min(n.pitch), hi.max(n.pitch)),
                                None => (n.pitch, n.pitch),
                            });
                            if melodic {
                                hist[(n.pitch % 12) as usize] += n.length_beats;
                            }
                        }
                    }
                }
            }
        }
        m.length_beats = (m.length_beats * 1e6).round() / 1e6;
        for (i, v) in hist.iter().enumerate() {
            m.pitch_classes[i] = *v as f32;
        }
        m
    }

    /// Length in seconds at the tempo of that time.
    pub fn length_secs(&self) -> f64 {
        if self.tempo_bpm > 0.0 { self.length_beats * 60.0 / self.tempo_bpm } else { 0.0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Clip, Id, Instrument, Note, Track};

    #[test]
    fn counts_notes_length_and_pitch_classes() {
        let mut p = Project::default();
        assert_eq!(Metrics::of(&p).notes, 0);
        let mut lead = Track::new(Id(1), "Lead", TrackKind::Instrument);
        lead.clips.push(Clip {
            id: Id(2),
            name: "A".into(),
            start_beats: 4.0,
            source: ClipSource::Notes { length_beats: 8.0, notes: vec![Note::new(60, 0.0, 2.0, 90), Note::new(67, 2.0, 1.0, 90)] },
        });
        let mut drums = Track::new(Id(3), "Drums", TrackKind::Instrument);
        drums.instrument = Some(Instrument { kind: InstrumentKind::Drums, ..Instrument::pluck() });
        drums.clips.push(Clip { id: Id(4), name: "B".into(), start_beats: 0.0, source: ClipSource::Notes { length_beats: 4.0, notes: vec![Note::new(36, 0.0, 0.5, 100)] } });
        let mut rec = Track::new(Id(5), "Rec", TrackKind::Audio);
        rec.clips.push(Clip {
            id: Id(6),
            name: "R".into(),
            start_beats: 0.0,
            source: ClipSource::Audio { asset_id: "a".into(), source_bpm: 120.0, warp: true, duration_secs: 1.0 },
        });
        p.tracks = vec![lead, drums, rec];
        let m = Metrics::of(&p);
        assert_eq!((m.notes, m.tracks, m.clips, m.audio_clips), (3, 3, 3, 1));
        assert_eq!(m.length_beats, 12.0, "lead clip ends at 4 + 8");
        assert_eq!(m.length_secs(), 6.0);
        assert_eq!(m.pitch_range, Some((36, 67)));
        // drums don't count for the key
        assert_eq!(m.pitch_classes[0], 2.0);
        assert_eq!(m.pitch_classes[7], 1.0);
        assert_eq!(m.pitch_classes.iter().sum::<f32>(), 3.0);
    }
}
