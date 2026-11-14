//! Demo content so the app sounds right away – and to show tempo
//! synchronization: beat (96 BPM) and arpeggio (128 BPM) both run at the project tempo.

use crate::state::AssetPool;
use klang_core::{Clip, ClipSource, Command, Instrument, Note, Project, ProjectPatch, Track, TrackKind};
use klang_dsp::synth::{self, NoteSpec, PluckParams};
use klang_dsp::util;
use klang_i18n::tr;

const SR: u32 = 48_000;

fn drum_loop(bpm: f64, bars: usize) -> Vec<f32> {
    let sr = SR as f32;
    let step = 60.0 / bpm / 4.0; // sixteenth
    let total = (bars as f64 * 16.0 * step * SR as f64) as usize;
    let mut out = vec![0f32; total + SR as usize];
    let (kick, snare) = (synth::kick(sr), synth::snare(sr, 11));
    for bar in 0..bars {
        for s in 0..16 {
            let at = (((bar * 16 + s) as f64) * step * SR as f64) as usize;
            if [0, 7, 8, 10].contains(&s) {
                synth::mix_into(&mut out, &kick, at, if s == 0 { 1.0 } else { 0.8 });
            }
            if s == 4 || s == 12 {
                synth::mix_into(&mut out, &snare, at, 0.7);
            }
            if s % 2 == 0 {
                let open = s == 14 && bar % 2 == 1;
                let h = synth::hat(sr, (bar * 16 + s) as u32 + 3, open);
                synth::mix_into(&mut out, &h, at, if s % 4 == 0 { 0.35 } else { 0.22 });
            }
        }
    }
    out.truncate(total);
    util::normalize(&mut out, 0.8);
    out
}

fn arpeggio(bpm: f64) -> Vec<f32> {
    // A minor, F major, C major, G major – one bar of sixteenth-note arpeggio each.
    let chords: [[u8; 4]; 4] = [[57, 60, 64, 69], [53, 57, 60, 65], [48, 52, 55, 60], [55, 59, 62, 67]];
    let pattern = [0usize, 1, 2, 3, 2, 1, 2, 3, 0, 1, 2, 3, 2, 3, 2, 1];
    let step = 60.0 / bpm as f32 / 4.0;
    let mut notes = Vec::new();
    for (bar, chord) in chords.iter().enumerate() {
        for (i, &p) in pattern.iter().enumerate() {
            notes.push(NoteSpec::new(
                (bar * 16 + i) as f32 * step,
                step * 1.6,
                chord[p] as f32 + 12.0,
                if i % 4 == 0 { 0.9 } else { 0.65 },
            ));
        }
    }
    let params = PluckParams { brightness: 0.75, decay: 0.35, body: 0.2, attack: 0.7 };
    let mut out = synth::render_notes(&notes, 64.0 * step, SR as f32, &params);
    out.truncate((64.0 * step * SR as f32) as usize);
    util::normalize(&mut out, 0.7);
    out
}

fn melody() -> Vec<Note> {
    // (pitch, start, length) in beats – a small melody of our own in A minor.
    let n: [(u8, f64, f64); 18] = [
        (69, 0.0, 1.0), (72, 1.0, 0.5), (71, 1.5, 0.5), (69, 2.0, 1.0), (64, 3.0, 1.0),
        (65, 4.0, 1.0), (69, 5.0, 1.0), (67, 6.0, 1.5),
        (72, 8.0, 1.0), (76, 9.0, 0.5), (74, 9.5, 0.5), (72, 10.0, 1.0), (67, 11.0, 1.0),
        (71, 12.0, 1.0), (74, 13.0, 0.5), (72, 13.5, 0.5), (71, 14.0, 1.0), (69, 15.0, 1.0),
    ];
    n.iter()
        .map(|&(pitch, start, len)| Note::new(pitch, start, len, 96))
        .collect()
}

pub fn demo_command(project: &mut Project, pool: &mut AssetPool) -> Command {
    let drums = drum_loop(96.0, 4);
    let drums_secs = drums.len() as f64 / SR as f64;
    let drums_id = pool.insert(tr!("Beat (96 BPM)"), drums, SR);

    let arp = arpeggio(128.0);
    let arp_secs = arp.len() as f64 / SR as f64;
    let arp_id = pool.insert(tr!("Arpeggio (128 BPM)"), arp, SR);

    let mut beat = Track::new(project.alloc_id(), tr!("Beat"), TrackKind::Audio);
    beat.gain_db = -2.0;
    beat.clips.push(Clip {
        id: project.alloc_id(),
        name: tr!("Beat · 96 BPM"),
        start_beats: 0.0,
        source: ClipSource::Audio { asset_id: drums_id, source_bpm: 96.0, warp: true, duration_secs: drums_secs },
    });

    let mut arp_track = Track::new(project.alloc_id(), tr!("Arpeggio"), TrackKind::Audio);
    arp_track.gain_db = -6.0;
    arp_track.pan = 0.35;
    arp_track.clips.push(Clip {
        id: project.alloc_id(),
        name: tr!("Arpeggio · 128 BPM"),
        start_beats: 0.0,
        source: ClipSource::Audio { asset_id: arp_id, source_bpm: 128.0, warp: true, duration_secs: arp_secs },
    });

    let mut lead = Track::new(project.alloc_id(), tr!("Melody"), TrackKind::Instrument);
    lead.gain_db = -3.0;
    lead.pan = -0.2;
    let mut instrument = Instrument::pluck();
    instrument.preset = "warm".into(); // identifier, do not translate
    for (k, v) in [("helligkeit", 0.4), ("abklingen", 0.7), ("koerper", 0.75), ("anschlag", 0.35)] {
        instrument.params.insert(k.into(), v);
    }
    lead.instrument = Some(instrument);
    lead.clips.push(Clip {
        id: project.alloc_id(),
        name: tr!("Subject"),
        start_beats: 0.0,
        source: ClipSource::Notes { length_beats: 16.0, notes: melody() },
    });

    Command::Batch {
        label: tr!("Load demo"),
        commands: vec![
            Command::SetProject { patch: ProjectPatch { tempo_bpm: Some(110.0), name: Some(tr!("Demo")), ..Default::default() } },
            Command::AddTrack { track: beat, index: None },
            Command::AddTrack { track: arp_track, index: None },
            Command::AddTrack { track: lead, index: None },
        ],
    }
}
