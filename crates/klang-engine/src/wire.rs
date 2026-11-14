//! Messages to the mixer as a sequence of numbers – for the path between two
//! WebAssembly instances in the browser (web core in the worker → mixer in the
//! AudioWorklet, via `postMessage`). On the desktop the same
//! [`RtMsg`]s go directly through the ring buffer.
//!
//! Sound buffers travel separately (once per buffer, with an ID); a
//! message only refers to their IDs. A message is a kind
//! ([`Kind`]) and a list of `f64`.

use crate::{Arrangement, PlacedClip, RtMsg};
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
    /// [Tempo (BPM, 0 = unchanged), beats per bar, clips …] – the tempo
    /// travels along so that position and sound change in the same block.
    /// Clips: each [buffer, side (0 = none), start frame, level left, level right, reverb]
    Arrangement = 8,
    /// [clips …] as in `Arrangement`, without tempo
    Preview = 9,
    PreviewMore = 10,
    /// [on (0/1)] – not for the mixer itself, but for the worklet
    Mute = 11,
    /// Free buffers: [ID, …]
    Free = 12,
    /// Recording: [on (0/1)]
    Record = 13,
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
            _ => return None,
        })
    }
}

/// Numbers per clip in an arrangement message.
pub const CLIP_FIELDS: usize = 6;

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

/// Control message → numbers (arrangements: see [`encode_clips`]).
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

/// Numbers → message to the mixer. Buffers come from `buffers`; clips with
/// an unknown buffer are dropped. `None` for kinds that do not go to the
/// mixer (Mute, Free, Record) or for broken messages.
pub fn decode(kind: Kind, d: &[f64], buffers: &HashMap<u32, Arc<[f32]>>) -> Option<RtMsg> {
    let n = |i: usize| d.get(i).copied();
    Some(match kind {
        Kind::Play => RtMsg::Play,
        Kind::Stop => RtMsg::Stop,
        Kind::Seek => RtMsg::Seek(n(0)?.max(0.0) as u64),
        Kind::Tempo => RtMsg::Tempo { bpm: n(0)?, beats_per_bar: n(1)?.max(1.0) as u32 },
        Kind::Metronome => RtMsg::Metronome(n(0)? != 0.0),
        Kind::Loop => RtMsg::Loop(range(d)),
        Kind::StopAt => RtMsg::StopAt(range(d)),
        Kind::Arrangement | Kind::Preview | Kind::PreviewMore => {
            let (tempo, d) = if kind == Kind::Arrangement {
                let head = d.get(..ARRANGEMENT_HEAD)?;
                let tempo = (head[0] > 0.0).then(|| (head[0], head[1].max(1.0) as u32));
                (tempo, &d[ARRANGEMENT_HEAD..])
            } else {
                (None, d)
            };
            let clips = d
                .chunks_exact(CLIP_FIELDS)
                .filter_map(|c| {
                    let data = buffers.get(&(c[0] as u32))?.clone();
                    let side = if c[1] > 0.0 { buffers.get(&(c[1] as u32)).cloned() } else { None };
                    Some(PlacedClip { data, side, start_frame: c[2].max(0.0) as u64, gain_l: c[3] as f32, gain_r: c[4] as f32, send: c[5] as f32 })
                })
                .collect();
            let arr = Arc::new(Arrangement { clips });
            match kind {
                Kind::Arrangement => RtMsg::Arrangement { arrangement: arr, tempo },
                Kind::Preview => RtMsg::Preview(arr),
                _ => RtMsg::PreviewMore(arr),
            }
        }
        Kind::Mute | Kind::Free | Kind::Record => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_messages_survive_the_trip() {
        let none = HashMap::new();
        for msg in [RtMsg::Play, RtMsg::Seek(48_000), RtMsg::Tempo { bpm: 93.5, beats_per_bar: 3 }, RtMsg::Loop(Some((4.0, 8.0))), RtMsg::StopAt(None)] {
            let (k, d) = encode(&msg).unwrap();
            let back = decode(Kind::from_u32(k as u32).unwrap(), &d, &none).unwrap();
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
        };
        let d = encode_clips(&arr, |b| if Arc::ptr_eq(b, &a) { 7 } else { 9 });
        assert_eq!(d.len(), CLIP_FIELDS, "leere Clips fallen weg");
        let buffers = HashMap::from([(7, a.clone()), (9, s.clone())]);
        let Some(RtMsg::Preview(back)) = decode(Kind::Preview, &d, &buffers) else { panic!("Vorhören erwartet") };
        let c = &back.clips[0];
        assert!(Arc::ptr_eq(&c.data, &a) && Arc::ptr_eq(c.side.as_ref().unwrap(), &s));
        assert_eq!((c.start_frame, c.gain_l, c.gain_r), (480, 0.5, 0.75));
        assert!((c.send - 0.2).abs() < 1e-6);
        // unknown buffer: clip is dropped
        let msg = [&arrangement_head(None)[..], &d].concat();
        let Some(RtMsg::Arrangement { arrangement: empty, tempo: None }) = decode(Kind::Arrangement, &msg, &HashMap::new()) else { panic!() };
        assert!(empty.clips.is_empty());
    }

    #[test]
    fn arrangement_carries_the_tempo_along() {
        let a: Arc<[f32]> = vec![0.5f32; 16].into();
        let arr = Arrangement { clips: vec![PlacedClip { start_frame: 96, data: a.clone(), side: None, gain_l: 1.0, gain_r: 0.25, send: 0.5 }] };
        let buffers = HashMap::from([(3, a.clone())]);
        for tempo in [Some((132.5, 3)), None] {
            let msg = [&arrangement_head(tempo)[..], &encode_clips(&arr, |_| 3)].concat();
            assert_eq!(msg.len(), ARRANGEMENT_HEAD + CLIP_FIELDS);
            let Some(RtMsg::Arrangement { arrangement, tempo: back }) = decode(Kind::from_u32(Kind::Arrangement as u32).unwrap(), &msg, &buffers) else {
                panic!("Arrangement erwartet")
            };
            assert_eq!(back, tempo, "Tempo übersteht den Weg");
            let c = &arrangement.clips[0];
            assert!(Arc::ptr_eq(&c.data, &a));
            assert_eq!((c.start_frame, c.gain_l, c.gain_r, c.send), (96, 1.0, 0.25, 0.5));
            // back to numbers: the same message
            assert_eq!([&arrangement_head(back)[..], &encode_clips(&arrangement, |_| 3)].concat(), msg);
        }
        // without a header: broken, no message
        assert!(decode(Kind::Arrangement, &[120.0], &buffers).is_none());
        // without clips: empty arrangement, the tempo still takes effect
        let Some(RtMsg::Arrangement { arrangement, tempo }) = decode(Kind::Arrangement, &arrangement_head(Some((90.0, 4))), &buffers) else { panic!() };
        assert!(arrangement.clips.is_empty() && tempo == Some((90.0, 4)));
    }
}
