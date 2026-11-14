//! Messages to the mixer as a sequence of numbers – for the path between two
//! WebAssembly instances in the browser (web core in the worker → mixer in the
//! AudioWorklet, via `postMessage`). On the desktop the same
//! [`RtMsg`]s go directly through the ring buffer.
//!
//! Sound buffers and note clips travel separately (once each, with an ID);
//! a message only refers to their IDs. A message is a kind ([`Kind`]) and a
//! list of `f64`.

use crate::{Arrangement, LiveNote, LiveNotes, NoteClip, OrganTable, Patch, PlacedClip, Plan, RtMsg, Shaping};
use std::collections::HashMap;
use std::sync::Arc;

/// Kind of a message.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Play = 1,
    Stop = 2,
    /// [Frame]
    Seek = 3,
    /// [Tempo, beats per bar]
    Tempo = 4,
    /// [an (0/1)]
    Metronome = 5,
    /// [set (0/1), start, end] in beats
    Loop = 6,
    /// [set (0/1), stop, jump back] in beats
    StopAt = 7,
    /// [Tempo (BPM, 0 = unchanged), beats per bar, number of clips, clips …,
    /// note clips …] – the tempo travels along so that position and sound
    /// change in the same block.
    /// Clips: each [buffer, side (0 = none), start frame, level left, level right, reverb];
    /// note clips: each [note clip, track, start (beats), level left, level right, reverb].
    Arrangement = 8,
    /// [clips …] as in `Arrangement`, without tempo and note clips
    Preview = 9,
    PreviewMore = 10,
    /// [on (0/1)] – not for the mixer itself, but for the worklet
    Mute = 11,
    /// Free buffers: [ID, …]
    Free = 12,
    /// Recording: [on (0/1)]
    Record = 13,
    /// A note clip for later arrangements, see [`encode_notes`].
    Notes = 14,
    /// Free note clips: [ID, …]
    FreeNotes = 15,
}

impl Kind {
    pub fn from_u32(k: u32) -> Option<Kind> {
        use Kind::*;
        Some(match k {
            1 => Play,
            2 => Stop,
            3 => Seek,
            4 => Tempo,
            5 => Metronome,
            6 => Loop,
            7 => StopAt,
            8 => Arrangement,
            9 => Preview,
            10 => PreviewMore,
            11 => Mute,
            12 => Free,
            13 => Record,
            14 => Notes,
            15 => FreeNotes,
            _ => return None,
        })
    }
}

/// Numbers per clip in an arrangement message.
pub const CLIP_FIELDS: usize = 6;

/// Numbers per note clip in an arrangement message.
pub const NOTE_CLIP_FIELDS: usize = 6;

/// Numbers per note in a [`Kind::Notes`] message.
pub const NOTE_FIELDS: usize = 12;

/// Numbers before the clips of an `Arrangement` message (tempo, beats per bar).
pub const ARRANGEMENT_HEAD: usize = 2;

/// Header of an `Arrangement` message; the clips belong before it ([`encode_clips`]).
pub fn arrangement_head(tempo: Option<(f64, u32)>) -> [f64; ARRANGEMENT_HEAD] {
    match tempo {
        Some((bpm, beats_per_bar)) if bpm > 0.0 => [bpm, beats_per_bar as f64],
        _ => [0.0, 0.0],
    }
}

fn range(d: &[f64]) -> Option<(f64, f64)> {
    (d.first().copied().unwrap_or(0.0) != 0.0 && d.len() >= 3).then(|| (d[1], d[2]))
}

fn put_range(r: Option<(f64, f64)>) -> Vec<f64> {
    match r {
        Some((a, b)) => vec![1.0, a, b],
        None => vec![0.0, 0.0, 0.0],
    }
}

/// Control message → numbers (arrangements: see [`encode_arrangement`]).
pub fn encode(msg: &RtMsg) -> Option<(Kind, Vec<f64>)> {
    Some(match msg {
        RtMsg::Play => (Kind::Play, vec![]),
        RtMsg::Stop => (Kind::Stop, vec![]),
        RtMsg::Seek(f) => (Kind::Seek, vec![*f as f64]),
        RtMsg::Tempo { bpm, beats_per_bar } => (Kind::Tempo, vec![*bpm, *beats_per_bar as f64]),
        RtMsg::Metronome(on) => (Kind::Metronome, vec![if *on { 1.0 } else { 0.0 }]),
        RtMsg::Loop(r) => (Kind::Loop, put_range(*r)),
        RtMsg::StopAt(r) => (Kind::StopAt, put_range(*r)),
        RtMsg::Arrangement { .. } | RtMsg::Preview(_) | RtMsg::PreviewMore(_) => return None,
    })
}

/// Clips → numbers; `id` returns the ID of a buffer.
pub fn encode_clips(arr: &Arrangement, mut id: impl FnMut(&Arc<[f32]>) -> u32) -> Vec<f64> {
    let mut out = Vec::with_capacity(arr.clips.len() * CLIP_FIELDS);
    for c in arr.clips.iter().filter(|c| !c.data.is_empty()) {
        let data = id(&c.data);
        let side = c.side.as_ref().map(&mut id).unwrap_or(0);
        out.extend_from_slice(&[data as f64, side as f64, c.start_frame as f64, c.gain_l as f64, c.gain_r as f64, c.send as f64]);
    }
    out
}

/// A whole `Arrangement` message: header, clips (`buffer` returns the ID of
/// a buffer) and note clips (`notes` the ID of a note clip).
pub fn encode_arrangement(arr: &Arrangement, tempo: Option<(f64, u32)>, buffer: impl FnMut(&Arc<[f32]>) -> u32, mut notes: impl FnMut(&Arc<LiveNotes>) -> u32) -> Vec<f64> {
    let clips = encode_clips(arr, buffer);
    let mut out = arrangement_head(tempo).to_vec();
    out.push((clips.len() / CLIP_FIELDS) as f64);
    out.extend(clips);
    for n in &arr.notes {
        out.extend_from_slice(&[notes(&n.notes) as f64, n.track as f64, n.start_beats, n.gain_l as f64, n.gain_r as f64, n.send as f64]);
    }
    out
}

/// A note clip as numbers: [ID, length (beats), sound shaping (6), number of
/// organ tables, (low, high buffer) each, patch …, notes …]. `buffer`
/// returns the ID of a buffer (organ tables).
pub fn encode_notes(id: u32, n: &LiveNotes, mut buffer: impl FnMut(&Arc<[f32]>) -> u32) -> Vec<f64> {
    let mut out = vec![id as f64, n.len_beats];
    out.extend(n.shaping.encode());
    out.push(n.tables().len() as f64);
    for t in n.tables() {
        out.push(buffer(&t.low) as f64);
        out.push(buffer(&t.high) as f64);
    }
    n.patch.encode(&mut out);
    for nt in &n.notes {
        let p = &nt.plan;
        out.extend_from_slice(&[
            nt.start,
            nt.len,
            nt.midi as f64,
            nt.velocity as f64,
            nt.vibrato as f64,
            nt.brightness as f64,
            nt.bend as f64,
            p.seed as f64,
            p.glide_from as f64,
            p.gap as f64,
            p.cut as f64,
            p.table as f64,
        ]);
    }
    out
}

/// Numbers → note clip with its ID ([`encode_notes`]); the organ tables come
/// from `buffers`.
pub fn decode_notes(d: &[f64], buffers: &HashMap<u32, Arc<[f32]>>) -> Option<(u32, LiveNotes)> {
    let id = *d.first()? as u32;
    let len_beats = *d.get(1)?;
    let shaping = Shaping::decode(d.get(2..)?)?;
    let mut at = 8;
    let n_tables = *d.get(at)? as usize;
    at += 1;
    let mut tables = Vec::with_capacity(n_tables);
    for _ in 0..n_tables {
        let low = buffers.get(&(*d.get(at)? as u32))?.clone();
        let high = buffers.get(&(*d.get(at + 1)? as u32))?.clone();
        tables.push(OrganTable { low, high });
        at += 2;
    }
    let (patch, used) = Patch::decode(d.get(at..)?)?;
    at += used;
    let notes = d
        .get(at..)?
        .chunks_exact(NOTE_FIELDS)
        .map(|c| {
            let mut n = LiveNote::new(c[0], c[1], c[2] as f32, c[3] as f32);
            (n.vibrato, n.brightness, n.bend) = (c[4] as f32, c[5] as f32, c[6] as f32);
            n.plan = Plan { seed: c[7] as u32, glide_from: c[8] as f32, gap: c[9] as f32, cut: c[10] as f32, table: (c[11] as u32).min(n_tables.saturating_sub(1) as u32) };
            n
        })
        .collect();
    Some((id, LiveNotes::from_parts(patch, shaping, notes, len_beats, tables)))
}

/// Numbers → message to the mixer. Buffers come from `buffers`, note clips
/// from `notes`; clips with an unknown buffer or note clip are dropped.
/// `None` for kinds that do not go to the mixer (Mute, Free, Record, Notes,
/// FreeNotes) or for broken messages.
pub fn decode(kind: Kind, d: &[f64], buffers: &HashMap<u32, Arc<[f32]>>, notes: &HashMap<u32, Arc<LiveNotes>>) -> Option<RtMsg> {
    let n = |i: usize| d.get(i).copied();
    let clip = |c: &[f64]| {
        let data = buffers.get(&(c[0] as u32))?.clone();
        let side = if c[1] > 0.0 { buffers.get(&(c[1] as u32)).cloned() } else { None };
        Some(PlacedClip { data, side, start_frame: c[2].max(0.0) as u64, gain_l: c[3] as f32, gain_r: c[4] as f32, send: c[5] as f32 })
    };
    Some(match kind {
        Kind::Play => RtMsg::Play,
        Kind::Stop => RtMsg::Stop,
        Kind::Seek => RtMsg::Seek(n(0)?.max(0.0) as u64),
        Kind::Tempo => RtMsg::Tempo { bpm: n(0)?, beats_per_bar: n(1)?.max(1.0) as u32 },
        Kind::Metronome => RtMsg::Metronome(n(0)? != 0.0),
        Kind::Loop => RtMsg::Loop(range(d)),
        Kind::StopAt => RtMsg::StopAt(range(d)),
        Kind::Arrangement => {
            let head = d.get(..ARRANGEMENT_HEAD)?;
            let tempo = (head[0] > 0.0).then(|| (head[0], head[1].max(1.0) as u32));
            let count = n(ARRANGEMENT_HEAD).unwrap_or(0.0) as usize;
            let start = ARRANGEMENT_HEAD + 1;
            let clips_end = (start + count * CLIP_FIELDS).min(d.len());
            let clips = d.get(start..clips_end).unwrap_or(&[]).chunks_exact(CLIP_FIELDS).filter_map(clip).collect();
            let note_clips = d
                .get(clips_end..)
                .unwrap_or(&[])
                .chunks_exact(NOTE_CLIP_FIELDS)
                .filter_map(|c| {
                    Some(NoteClip { notes: notes.get(&(c[0] as u32))?.clone(), track: c[1] as u64, start_beats: c[2], gain_l: c[3] as f32, gain_r: c[4] as f32, send: c[5] as f32 })
                })
                .collect();
            RtMsg::Arrangement { arrangement: Arc::new(Arrangement { clips, notes: note_clips }), tempo }
        }
        Kind::Preview | Kind::PreviewMore => {
            let arr = Arc::new(Arrangement { clips: d.chunks_exact(CLIP_FIELDS).filter_map(clip).collect(), notes: Vec::new() });
            if kind == Kind::Preview {
                RtMsg::Preview(arr)
            } else {
                RtMsg::PreviewMore(arr)
            }
        }
        Kind::Mute | Kind::Free | Kind::Record | Kind::Notes | Kind::FreeNotes => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use klang_dsp::live::Patch;

    #[test]
    fn control_messages_survive_the_trip() {
        let none = HashMap::new();
        for msg in [RtMsg::Play, RtMsg::Seek(48_000), RtMsg::Tempo { bpm: 93.5, beats_per_bar: 3 }, RtMsg::Loop(Some((4.0, 8.0))), RtMsg::StopAt(None)] {
            let (k, d) = encode(&msg).unwrap();
            let back = decode(Kind::from_u32(k as u32).unwrap(), &d, &none, &HashMap::new()).unwrap();
            assert_eq!(encode(&back).unwrap(), (k, d));
        }
    }

    #[test]
    fn clips_refer_to_buffers_by_id() {
        let a: Arc<[f32]> = vec![0.25f32; 10].into();
        let s: Arc<[f32]> = vec![0.1f32; 10].into();
        let arr = Arrangement {
            clips: vec![
                PlacedClip { start_frame: 480, data: a.clone(), side: Some(s.clone()), gain_l: 0.5, gain_r: 0.75, send: 0.2 },
                PlacedClip { start_frame: 0, data: Arc::from(Vec::new()), side: None, gain_l: 1.0, gain_r: 1.0, send: 0.0 },
            ],
            notes: Vec::new(),
        };
        let d = encode_clips(&arr, |b| if Arc::ptr_eq(b, &a) { 7 } else { 9 });
        assert_eq!(d.len(), CLIP_FIELDS, "empty clips are dropped");
        let buffers = HashMap::from([(7, a.clone()), (9, s.clone())]);
        let Some(RtMsg::Preview(back)) = decode(Kind::Preview, &d, &buffers, &HashMap::new()) else { panic!("preview expected") };
        let c = &back.clips[0];
        assert!(Arc::ptr_eq(&c.data, &a) && Arc::ptr_eq(c.side.as_ref().unwrap(), &s));
        assert_eq!((c.start_frame, c.gain_l, c.gain_r), (480, 0.5, 0.75));
        assert!((c.send - 0.2).abs() < 1e-6);
        // unknown buffer: clip is dropped
        let msg = encode_arrangement(&arr, None, |b| if Arc::ptr_eq(b, &a) { 7 } else { 9 }, |_| 0);
        let Some(RtMsg::Arrangement { arrangement: empty, tempo: None }) = decode(Kind::Arrangement, &msg, &HashMap::new(), &HashMap::new()) else { panic!() };
        assert!(empty.clips.is_empty());
    }

    #[test]
    fn arrangement_carries_the_tempo_along() {
        let a: Arc<[f32]> = vec![0.5f32; 16].into();
        let arr = Arrangement { clips: vec![PlacedClip { start_frame: 96, data: a.clone(), side: None, gain_l: 1.0, gain_r: 0.25, send: 0.5 }], notes: Vec::new() };
        let buffers = HashMap::from([(3, a.clone())]);
        for tempo in [Some((132.5, 3)), None] {
            let msg = encode_arrangement(&arr, tempo, |_| 3, |_| 0);
            assert_eq!(msg.len(), ARRANGEMENT_HEAD + 1 + CLIP_FIELDS);
            let Some(RtMsg::Arrangement { arrangement, tempo: back }) = decode(Kind::from_u32(Kind::Arrangement as u32).unwrap(), &msg, &buffers, &HashMap::new()) else {
                panic!("arrangement expected")
            };
            assert_eq!(back, tempo, "the tempo survives the trip");
            let c = &arrangement.clips[0];
            assert!(Arc::ptr_eq(&c.data, &a));
            assert_eq!((c.start_frame, c.gain_l, c.gain_r, c.send), (96, 1.0, 0.25, 0.5));
            // back to numbers: the same message
            assert_eq!(encode_arrangement(&arrangement, back, |_| 3, |_| 0), msg);
        }
        // without a header: broken, no message
        assert!(decode(Kind::Arrangement, &[120.0], &buffers, &HashMap::new()).is_none());
        // without clips: empty arrangement, the tempo still takes effect
        let Some(RtMsg::Arrangement { arrangement, tempo }) = decode(Kind::Arrangement, &arrangement_head(Some((90.0, 4))), &buffers, &HashMap::new()) else { panic!() };
        assert!(arrangement.clips.is_empty() && tempo == Some((90.0, 4)));
    }

    #[test]
    fn note_clips_travel_once_and_are_referred_to_by_id() {
        let sr = 48_000.0;
        let notes: Vec<LiveNote> = [(0.0, 1.0, 60.0), (1.0, 0.5, 64.5), (1.5, 2.0, 60.0)]
            .iter()
            .map(|&(s, l, m)| LiveNote { bend: if m > 62.0 { -1.0 } else { 0.0 }, vibrato: 0.3, ..LiveNote::new(s, l, m, 0.7) })
            .collect();
        let shaping = Shaping { waerme: 0.3, breite: 0.6, farbe: 0.5, ..Default::default() };
        for patch in [
            Patch::Organ(klang_dsp::keys::OrganParams { mixtur: 0.4, ..Default::default() }),
            Patch::Synth(klang_dsp::pad::SynthParams::default(), klang_dsp::pad::Verlauf { puls: 2, ..Default::default() }),
            Patch::Drums(klang_dsp::drums::DrumKit::default()),
            Patch::Bass(klang_dsp::bass::BassParams { glide: 0.4, ..Default::default() }),
        ] {
            let live = Arc::new(LiveNotes::new(patch.clone(), shaping, &notes, 3.0, sr));
            let mut buffers = HashMap::new();
            let d = encode_notes(5, &live, |b| {
                let id = buffers.len() as u32 + 1;
                buffers.insert(id, b.clone());
                id
            });
            let (id, back) = decode_notes(&d, &buffers).expect("note clip");
            assert_eq!(id, 5);
            assert_eq!(back.patch, patch);
            assert_eq!(back.shaping, shaping);
            assert_eq!(back.notes, live.notes, "{patch:?}");
            assert_eq!(back.tables().len(), live.tables().len());
            // an arrangement refers to it by its ID
            let back = Arc::new(back);
            let arr = Arrangement { clips: Vec::new(), notes: vec![NoteClip { track: 3, start_beats: 8.0, notes: live.clone(), gain_l: 0.5, gain_r: 0.7, send: 0.1 }] };
            let msg = encode_arrangement(&arr, Some((100.0, 4)), |_| 0, |_| 5);
            let known = HashMap::from([(5, back.clone())]);
            let Some(RtMsg::Arrangement { arrangement, .. }) = decode(Kind::Arrangement, &msg, &HashMap::new(), &known) else { panic!() };
            let c = &arrangement.notes[0];
            assert!(Arc::ptr_eq(&c.notes, &back));
            assert_eq!((c.track, c.start_beats, c.gain_l, c.gain_r), (3, 8.0, 0.5, 0.7));
        }
    }
}
