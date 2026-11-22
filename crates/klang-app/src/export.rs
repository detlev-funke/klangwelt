//! Export: the project – or the selected range – as a file. Rendering is
//! offline with the same mix as playback (48 kHz, muted tracks are left
//! out, solo applies). WAV, FLAC, the Ogg container and MIDI are our own
//! code; Opus encoding is done by libopus (klang-opus).
//!
//! Location: Musik/Klangwelt/Exporte (with `KLANGWELT_BIBLIOTHEK=<Ordner>`:
//! `<Ordner>/Exporte`); a name with a path ends up exactly there.

use crate::flac;
use crate::state::{AppState, CmdResult};
use klang_core::{ClipSource, Id, InstrumentKind, Project, Track};
use klang_i18n::tr;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Opus via libopus (C, feature "opus") – on the desktop and in the browser
/// (built there with clang for WebAssembly). Without the feature the list
/// of bitrates stays empty and the dialog does not offer Opus.
#[cfg(feature = "opus")]
mod opus {
    pub use klang_opus::{encode_ogg_opus as encode, BITRATES, DEFAULT_BITRATE};
}
#[cfg(not(feature = "opus"))]
mod opus {
    pub const BITRATES: [u32; 0] = [];
    pub const DEFAULT_BITRATE: u32 = 160;
    pub fn encode(_left: &[f32], _right: &[f32], _kbps: u32, _title: &str) -> Result<Vec<u8>, String> {
        Err(klang_i18n::tr!("Opus isn't available in this version (built without libopus) – please choose WAV or FLAC."))
    }
}

pub const SAMPLE_RATE: u32 = 48_000;
/// Tail (reverb, decay) lasts at most this long after the last bar.
const TAIL_SECS: f64 = 6.0;
/// Below −80 dB the tail counts as decayed.
const SILENT: f32 = 1e-4;

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ExportOptions {
    /// wav, flac, opus, stems or midi
    pub format: String,
    /// File name (without extension) or full path; empty = project name
    #[serde(default)]
    pub name: Option<String>,
    /// Opus: kbit/s (default 160)
    #[serde(default)]
    pub bitrate: Option<u32>,
    /// WAV, FLAC, stems: 16 or 24 bit (default 24)
    #[serde(default)]
    pub bits: Option<u32>,
    /// Only the selected range
    #[serde(default)]
    pub range: Option<bool>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    pub format: String,
    /// File – for stems, the folder
    pub path: String,
    pub files: Vec<String>,
    pub bytes: u64,
    pub seconds: f64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ExportInfo {
    pub dir: String,
    /// Suggestion for the file name
    pub name: String,
    /// Selected range in beats
    pub range: Option<[f64; 2]>,
    pub range_label: Option<String>,
    /// Length of the piece in seconds (0 = empty)
    pub seconds: f64,
    pub has_notes: bool,
    pub bitrates: Vec<u32>,
    pub default_bitrate: u32,
    /// Files and folders that already exist in the folder
    pub existing: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Format {
    Wav,
    Flac,
    Opus,
    Stems,
    Midi,
}

impl Format {
    fn parse(s: &str) -> CmdResult<Self> {
        Ok(match s.trim().to_ascii_lowercase().as_str() {
            "wav" | "wave" => Format::Wav,
            "flac" => Format::Flac,
            "opus" | "ogg" => Format::Opus,
            "stems" | "spuren" => Format::Stems,
            "midi" | "mid" => Format::Midi,
            other => return Err(tr!("Unknown export format \"{format}\" (wav, flac, opus, stems, midi)", format = other)),
        })
    }

    fn key(self) -> &'static str {
        match self {
            Format::Wav => "wav",
            Format::Flac => "flac",
            Format::Opus => "opus",
            Format::Stems => "stems",
            Format::Midi => "midi",
        }
    }

    fn ext(self) -> Option<&'static str> {
        match self {
            Format::Wav => Some("wav"),
            Format::Flac => Some("flac"),
            Format::Opus => Some("opus"),
            Format::Stems => None,
            Format::Midi => Some("mid"),
        }
    }
}

/// Default folder for exports.
pub fn export_dir(st: &AppState) -> PathBuf {
    crate::host::beside_library(st.host(), "Exporte")
}

/// Characters that Windows does not allow in file names become "_".
fn clean(name: &str) -> String {
    let s: String = name.chars().map(|c| if "<>:\"/\\|?*".contains(c) || c.is_control() { '_' } else { c }).collect();
    let s = s.trim().trim_end_matches(['.', ' ']).to_string();
    if s.is_empty() { "Export".into() } else { s }
}

fn target(dir: &Path, name: &str, fmt: Format) -> PathBuf {
    let name = name.trim();
    let is_path = name.contains(['\\', '/']) || name.contains(':');
    let mut p = if is_path { PathBuf::from(name) } else { dir.join(clean(name)) };
    match fmt.ext() {
        Some(ext) => {
            let has = p.extension().is_some_and(|e| e.to_string_lossy().eq_ignore_ascii_case(ext));
            if !has {
                let mut s = p.into_os_string();
                s.push(format!(".{ext}"));
                p = PathBuf::from(s);
            }
            p
        }
        None if is_path => p,
        None => dir.join(format!("{} – Stems", clean(name))),
    }
}

fn default_name(project: &Project, file: Option<&Path>) -> String {
    if let Some(stem) = file.and_then(Path::file_stem) {
        return stem.to_string_lossy().into_owned();
    }
    if project.name.trim().is_empty() || project.name == "Neues Projekt" { "Export".into() } else { clean(&project.name) }
}

fn audible(project: &Project) -> impl Iterator<Item = &Track> {
    project.audible_tracks()
}

fn song_end_beats(project: &Project) -> f64 {
    let bpm = project.tempo_bpm;
    project.tracks.iter().flat_map(|t| t.clips.iter()).map(|c| c.start_beats + c.length_beats(bpm)).fold(0.0, f64::max)
}

fn range_label(project: &Project, (a, b): (f64, f64)) -> String {
    let bpb = project.beats_per_bar.max(1) as f64;
    let from = (a / bpb).floor() as i64 + 1;
    let to = ((b / bpb) - 1e-9).ceil().max(1.0) as i64;
    if from >= to { tr!("Bar {from}", from = from) } else { tr!("bars {from}–{to}", from = from, to = to) }
}

pub fn info(dir: &Path, st: &AppState) -> CmdResult<ExportInfo> {
    let src = st.export_source()?;
    let file = st.project_file.lock().ok().and_then(|f| f.clone());
    let p = &src.project;
    let existing = crate::vfs::list(dir).map(|l| l.into_iter().map(|e| e.name).collect()).unwrap_or_default();
    let has_notes = audible(p).any(|t| t.clips.iter().any(|c| matches!(&c.source, ClipSource::Notes { notes, .. } if !notes.is_empty())));
    Ok(ExportInfo {
        dir: dir.display().to_string(),
        name: default_name(p, file.as_deref()),
        range: src.range.map(|(a, b)| [a, b]),
        range_label: src.range.map(|r| range_label(p, r)),
        seconds: song_end_beats(p) * 60.0 / p.tempo_bpm,
        has_notes,
        bitrates: opus::BITRATES.to_vec(),
        default_bitrate: opus::DEFAULT_BITRATE,
        existing,
    })
}

/// Time span to render in frames: (start, shortest end, render length).
/// Without a range the tail may extend past the end.
fn span(project: &Project, range: Option<(f64, f64)>) -> CmdResult<(usize, usize, usize)> {
    let spb = 60.0 / project.tempo_bpm;
    let frames = |beats: f64| (beats.max(0.0) * spb * SAMPLE_RATE as f64).round() as usize;
    match range {
        Some((a, b)) => Ok((frames(a), frames(b), frames(b))),
        None => {
            let end = song_end_beats(project);
            if end <= 0.0 {
                return Err(tr!("The project is empty – there's nothing to export."));
            }
            let bar = project.beats_per_bar.max(1) as f64;
            Ok((0, frames(end), frames(end + bar) + (TAIL_SECS * SAMPLE_RATE as f64) as usize))
        }
    }
}

/// Length after trimming the decayed tail.
fn trimmed_len(left: &[f32], right: &[f32], min: usize) -> usize {
    let last = (0..left.len()).rev().find(|&i| left[i].abs() > SILENT || right[i].abs() > SILENT).map_or(0, |i| i + 1);
    (last + SAMPLE_RATE as usize / 20).max(min).min(left.len())
}

/// Float → integer with `bits` bits (16 bit with triangular dither), interleaved.
fn quantize(left: &[f32], right: &[f32], bits: u32) -> Vec<i32> {
    let max = ((1i64 << (bits - 1)) - 1) as f32;
    let mut seed = 0x9E37_79B9u32;
    let mut rand = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as f32 / u32::MAX as f32
    };
    let dither = bits <= 16;
    let mut out = Vec::with_capacity(left.len() * 2);
    for (&l, &r) in left.iter().zip(right) {
        for v in [l, r] {
            let d = if dither { rand() - rand() } else { 0.0 };
            out.push((v.clamp(-1.0, 1.0) * max + d).round().clamp(-max - 1.0, max) as i32);
        }
    }
    out
}

/// PCM WAV (stereo, 16 or 24 bit).
pub fn wav(left: &[f32], right: &[f32], bits: u32, sample_rate: u32) -> Vec<u8> {
    let pcm = quantize(left, right, bits);
    let bytes = (bits / 8) as usize;
    let data_len = pcm.len() * bytes;
    let mut out = Vec::with_capacity(44 + data_len);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2 * bytes as u32).to_le_bytes());
    out.extend_from_slice(&((2 * bytes) as u16).to_le_bytes());
    out.extend_from_slice(&(bits as u16).to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_len as u32).to_le_bytes());
    for v in pcm {
        out.extend_from_slice(&v.to_le_bytes()[..bytes]);
    }
    out
}

// ---------------------------------------------------------------- MIDI

fn vlq(out: &mut Vec<u8>, mut v: u32) {
    let mut stack = vec![(v & 0x7F) as u8];
    v >>= 7;
    while v > 0 {
        stack.push(0x80 | (v & 0x7F) as u8);
        v >>= 7;
    }
    out.extend(stack.iter().rev());
}

fn chunk(out: &mut Vec<u8>, id: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(id);
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(body);
}

/// Events (tick, order, bytes) → track data with delta times.
fn track_body(mut events: Vec<(u64, u8, Vec<u8>)>, end: u64) -> Vec<u8> {
    events.sort_by_key(|e| (e.0, e.1));
    let mut body = Vec::new();
    let mut now = 0u64;
    for (tick, _, bytes) in events {
        vlq(&mut body, (tick - now) as u32);
        body.extend_from_slice(&bytes);
        now = tick;
    }
    vlq(&mut body, end.saturating_sub(now) as u32);
    body.extend_from_slice(&[0xFF, 0x2F, 0x00]);
    body
}

fn meta(kind: u8, data: &[u8]) -> Vec<u8> {
    let mut v = vec![0xFF, kind];
    vlq(&mut v, data.len() as u32);
    v.extend_from_slice(data);
    v
}

/// General MIDI program matching the instrument (drums: channel 10).
fn gm_program(kind: InstrumentKind) -> Option<u8> {
    match kind {
        InstrumentKind::Pluck => Some(25),       // Steel Guitar
        InstrumentKind::Bass => Some(38),        // Synth Bass 1
        InstrumentKind::Organ => Some(19),       // Church Organ
        InstrumentKind::Harpsichord => Some(6),  // Harpsichord
        InstrumentKind::Synth => Some(89),       // Pad 2 (warm)
        InstrumentKind::Drums => None,
    }
}

/// Standard MIDI file (type 1, 480 ticks per quarter): one track per
/// instrument track with notes, plus tempo and time signature in the first.
pub fn midi(project: &Project, range: Option<(f64, f64)>) -> CmdResult<Vec<u8>> {
    const PPQ: f64 = 480.0;
    // Beats count in the unit of the time signature (4 = quarter, 8 = eighth)
    let quarter_per_beat = 4.0 / project.beat_unit.max(1) as f64;
    let tick = |beats: f64| (beats * quarter_per_beat * PPQ).round().max(0.0) as u64;
    let (from, to) = range.unwrap_or((0.0, f64::INFINITY));
    let mut tracks: Vec<Vec<(u64, u8, Vec<u8>)>> = Vec::new();
    let mut end = 0u64;
    let mut channel = 0u8;
    for t in audible(project) {
        let Some(inst) = t.instrument.as_ref() else { continue };
        let drums = inst.kind == InstrumentKind::Drums;
        let ch = if drums {
            9
        } else {
            let c = channel;
            channel = (channel + 1) % 16;
            if channel == 9 {
                channel = 10;
            }
            c
        };
        let shift = t.pitch_semitones.round() as i32;
        let mut events: Vec<(u64, u8, Vec<u8>)> = Vec::new();
        for clip in &t.clips {
            let ClipSource::Notes { length_beats, notes } = &clip.source else { continue };
            for n in notes.iter().filter(|n| n.start_beats < *length_beats) {
                let a = clip.start_beats + n.start_beats;
                let b = clip.start_beats + n.start_beats + n.length_beats.min(length_beats - n.start_beats);
                if a < from || a >= to {
                    continue;
                }
                let pitch = (n.pitch as i32 + if drums { 0 } else { shift }).clamp(0, 127) as u8;
                let (on, off) = (tick(a - from), tick(b.min(to) - from).max(tick(a - from) + 1));
                events.push((on, 1, vec![0x90 | ch, pitch, n.velocity.clamp(1, 127)]));
                events.push((off, 0, vec![0x80 | ch, pitch, 64]));
                end = end.max(off);
            }
        }
        if events.is_empty() {
            continue;
        }
        let mut head = vec![(0, 0, meta(0x03, t.name.as_bytes()))];
        if let Some(p) = gm_program(inst.kind) {
            head.push((0, 0, vec![0xC0 | ch, p]));
        }
        events.splice(0..0, head);
        tracks.push(events);
    }
    if tracks.is_empty() {
        return Err(tr!("No notes to export – MIDI only contains instrument tracks (audio isn't possible)."));
    }

    let us_per_quarter = (60_000_000.0 / (project.tempo_bpm * quarter_per_beat)).round() as u32;
    let denom = (project.beat_unit.max(1) as f64).log2().round() as u8;
    let conductor = vec![
        (0, 0, meta(0x03, project.name.as_bytes())),
        (0, 0, meta(0x51, &us_per_quarter.to_be_bytes()[1..])),
        (0, 0, meta(0x58, &[project.beats_per_bar.clamp(1, 255) as u8, denom, 24, 8])),
    ];
    let mut out = Vec::new();
    let mut head = Vec::new();
    head.extend_from_slice(&1u16.to_be_bytes());
    head.extend_from_slice(&((tracks.len() + 1) as u16).to_be_bytes());
    head.extend_from_slice(&(PPQ as u16).to_be_bytes());
    chunk(&mut out, b"MThd", &head);
    chunk(&mut out, b"MTrk", &track_body(conductor, end));
    for t in tracks {
        chunk(&mut out, b"MTrk", &track_body(t, end));
    }
    Ok(out)
}

// ---------------------------------------------------------------- Flow

fn write(path: &Path, data: &[u8]) -> CmdResult<()> {
    if let Some(parent) = path.parent() {
        crate::vfs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    crate::vfs::write(path, data).map_err(|e| format!("{}: {e}", path.display()))
}

/// Runs the export.
pub fn run(dir: &Path, st: &AppState, opts: &ExportOptions) -> CmdResult<ExportResult> {
    let fmt = Format::parse(&opts.format)?;
    let mut src = st.export_source()?;
    let project = src.project.clone();
    let range = if opts.range.unwrap_or(false) {
        Some(src.range.ok_or_else(|| tr!("No time range selected – drag out a range in the timeline or export the whole piece."))?)
    } else {
        None
    };
    let file = st.project_file.lock().ok().and_then(|f| f.clone());
    let name = opts.name.as_deref().map(str::trim).filter(|n| !n.is_empty()).map(str::to_string).unwrap_or_else(|| default_name(&project, file.as_deref()));
    let path = target(dir, &name, fmt);
    let title = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| name.clone());
    let bits = match opts.bits.unwrap_or(24) {
        16 => 16,
        24 => 24,
        b => return Err(tr!("{bits}-bit isn't supported – use 16 or 24", bits = b)),
    };

    if fmt == Format::Midi {
        let data = midi(&project, range)?;
        write(&path, &data)?;
        let end = range.map_or_else(|| song_end_beats(&project), |(a, b)| b - a);
        return Ok(ExportResult {
            format: fmt.key().into(),
            path: path.display().to_string(),
            files: vec![path.display().to_string()],
            bytes: data.len() as u64,
            seconds: end * 60.0 / project.tempo_bpm,
        });
    }

    let (start, min_end, frames) = span(&project, range)?;
    let mix = src.arrangement(&project, SAMPLE_RATE);
    let (mut left, mut right) = klang_engine::render_offline(mix, SAMPLE_RATE, frames as u64);
    left.drain(..start.min(left.len()));
    right.drain(..start.min(right.len()));
    let len = if range.is_some() { left.len() } else { trimmed_len(&left, &right, min_end - start) };
    left.truncate(len);
    right.truncate(len);
    let seconds = len as f64 / SAMPLE_RATE as f64;

    let (files, bytes) = match fmt {
        Format::Wav => {
            let data = wav(&left, &right, bits, SAMPLE_RATE);
            write(&path, &data)?;
            (vec![path.clone()], data.len() as u64)
        }
        Format::Flac => {
            let data = flac::encode(&quantize(&left, &right, bits), bits, SAMPLE_RATE, &title);
            write(&path, &data)?;
            (vec![path.clone()], data.len() as u64)
        }
        Format::Opus => {
            let kbps = opts.bitrate.unwrap_or(opus::DEFAULT_BITRATE);
            if !(32..=510).contains(&kbps) {
                return Err(tr!("{kbps} kbit/s isn't supported – use 32 to 510", kbps = kbps));
            }
            let data = opus::encode(&left, &right, kbps, &title)?;
            write(&path, &data)?;
            (vec![path.clone()], data.len() as u64)
        }
        Format::Stems => {
            crate::vfs::create_dir_all(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let mut files = Vec::new();
            let mut total = 0u64;
            for (i, t) in audible(&project).enumerate() {
                let mut solo = t.clone();
                solo.mute = false;
                solo.solo = false;
                // the group's level stays in the stem, its mute and solo don't apply alone
                solo.gain_db = project.track_gain_db(t);
                solo.group = Id::NEW;
                let single = Project { tracks: vec![solo], groups: Vec::new(), ..project.clone() };
                let arr = src.arrangement(&single, SAMPLE_RATE);
                let (mut l, mut r) = klang_engine::render_offline(arr, SAMPLE_RATE, (start + len) as u64);
                l.drain(..start.min(l.len()));
                r.drain(..start.min(r.len()));
                let data = wav(&l, &r, bits, SAMPLE_RATE);
                let file = path.join(format!("{:02} {}.wav", i + 1, clean(&t.name)));
                write(&file, &data)?;
                total += data.len() as u64;
                files.push(file);
            }
            if files.is_empty() {
                return Err(tr!("No audible track – are all of them muted?"));
            }
            (files, total)
        }
        Format::Midi => unreachable!("oben erledigt"),
    };
    Ok(ExportResult {
        format: fmt.key().into(),
        path: path.display().to_string(),
        files: files.iter().map(|f| f.display().to_string()).collect(),
        bytes,
        seconds,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use klang_core::{Clip, Id, Instrument, Note, TrackKind};

    fn project() -> Project {
        let mut p = Project { name: "Probe".into(), tempo_bpm: 120.0, ..Default::default() };
        let mut t = Track::new(Id(1), "Melodie", TrackKind::Instrument);
        t.clips.push(Clip {
            id: Id(2),
            name: "c".into(),
            start_beats: 4.0,
            source: ClipSource::Notes { length_beats: 4.0, notes: vec![Note::new(60, 0.0, 1.0, 100), Note::new(64, 1.0, 1.0, 90), Note::new(67, 3.5, 2.0, 80)] },
        });
        let mut d = Track::new(Id(3), "Beat", TrackKind::Instrument);
        d.instrument = Some(Instrument { kind: InstrumentKind::Drums, ..Instrument::pluck() });
        d.clips.push(Clip { id: Id(4), name: "d".into(), start_beats: 0.0, source: ClipSource::Notes { length_beats: 2.0, notes: vec![Note::new(36, 0.0, 0.5, 110)] } });
        p.tracks = vec![t, d];
        p
    }

    #[test]
    fn wav_header_and_length() {
        let l = vec![0.5f32; 1000];
        let w = wav(&l, &l, 24, 48_000);
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(&w[8..16], b"WAVEfmt ");
        assert_eq!(u16::from_le_bytes([w[34], w[35]]), 24);
        assert_eq!(w.len(), 44 + 1000 * 2 * 3);
        // 0.5 → half level
        let s = i32::from_le_bytes([0, w[44], w[45], w[46]]) >> 8;
        assert_eq!(s, (0.5f32 * 8_388_607.0).round() as i32);
        let w16 = wav(&l, &l, 16, 48_000);
        assert_eq!(w16.len(), 44 + 1000 * 4);
    }

    #[test]
    fn names_and_paths() {
        let dir = Path::new("C:/Exporte");
        assert_eq!(target(dir, "Mein Lied", Format::Wav), dir.join("Mein Lied.wav"));
        assert_eq!(target(dir, "Lied 1.2", Format::Flac), dir.join("Lied 1.2.flac"));
        assert_eq!(target(dir, "a.OPUS", Format::Opus), dir.join("a.OPUS"));
        assert_eq!(target(dir, "Wo?Was*", Format::Midi), dir.join("Wo_Was_.mid"));
        assert_eq!(target(dir, "Lied", Format::Stems), dir.join("Lied – Stems"));
        assert_eq!(target(dir, "D:/x/y", Format::Wav), PathBuf::from("D:/x/y.wav"));
        assert_eq!(clean("  ..  "), "Export");
    }

    #[test]
    fn tail_is_trimmed_after_it_fades() {
        let mut l = vec![0.0f32; 48_000];
        l[1000] = 0.5;
        let r = vec![0.0f32; 48_000];
        // shortest end wins if it is already silent earlier
        assert_eq!(trimmed_len(&l, &r, 10_000), 10_000);
        // otherwise until shortly after the last audible sample
        assert_eq!(trimmed_len(&l, &r, 0), 1001 + 2400);
    }

    #[test]
    fn midi_has_tempo_tracks_and_notes() {
        let p = project();
        let m = midi(&p, None).unwrap();
        assert_eq!(&m[..4], b"MThd");
        assert_eq!(u16::from_be_bytes([m[8], m[9]]), 1, "Typ 1");
        assert_eq!(u16::from_be_bytes([m[10], m[11]]), 3, "Dirigent + 2 Spuren");
        assert_eq!(u16::from_be_bytes([m[12], m[13]]), 480);
        // Tempo 120 = 500 000 µs per quarter
        let tempo = m.windows(3).position(|w| w == [0xFF, 0x51, 0x03]).unwrap();
        assert_eq!(&m[tempo + 3..tempo + 6], &[0x07, 0xA1, 0x20]);
        // Drums on channel 10, the melody with a program
        assert!(m.windows(3).any(|w| w == [0x99, 36, 110]));
        assert!(m.windows(2).any(|w| w == [0xC0, 25]));
        assert!(m.windows(3).any(|w| w == [0x90, 60, 100]));
        // A note extending past the clip end is trimmed: G ends at beat 8
        assert_eq!(m.windows(3).filter(|w| w[0] == 0x90).count(), 3);
    }

    #[test]
    fn midi_range_shifts_and_filters() {
        let p = project();
        let m = midi(&p, Some((4.0, 6.0))).unwrap();
        // only the two melody notes in the range, the drums are missing entirely
        assert_eq!(u16::from_be_bytes([m[10], m[11]]), 2);
        assert_eq!(m.windows(3).filter(|w| w[0] == 0x90).count(), 2);
        // first note starts at tick 0: delta 0 right before the note-on
        let on = m.windows(3).position(|w| w == [0x90, 60, 100]).unwrap();
        assert_eq!(m[on - 1], 0);
        assert!(midi(&Project::default(), None).is_err());
    }

    #[test]
    fn variable_length_numbers() {
        for (v, want) in [(0u32, vec![0u8]), (0x7F, vec![0x7F]), (0x80, vec![0x81, 0x00]), (0x3FFF, vec![0xFF, 0x7F]), (0x0F_FFFF, vec![0xBF, 0xFF, 0x7F])] {
            let mut out = Vec::new();
            vlq(&mut out, v);
            assert_eq!(out, want, "{v:#x}");
        }
    }
}
