//! Song templates: a whole piece made of sections (intro, build-up, break,
//! climax, outro …) and roles (pad, arpeggio, bass, lead, kick, hi-hats,
//! clap, percussion, transitions). One level per section and role: 0 off,
//! 1 sparse, 2 full. This produces tracks with one clip per section –
//! each melodic role one track, all drums together one drums track.
//!
//! The kind of a section shapes it: A build-up gets louder and brighter over
//! its length (brightness of the notes opens the filter) and ends with a
//! snare roll, an outro fades out. Everything is deterministic – the same
//! template produces the same notes.

use crate::rhythm;
use crate::sound;
use crate::state::{lock, AppState, CmdResult, Snapshot};
use klang_core::{Clip, ClipSource, Command, Id, Instrument, InstrumentKind, Note, ProjectPatch, Track, TrackKind, TrackView};
use klang_dsp::drums;
use klang_i18n::tr;
use klang_music::harmony as kh;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const BEATS_PER_BAR: f64 = 4.0;
pub const MAX_BARS: u32 = 400;

/// Role of a row in the song structure.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    Pad,
    Arp,
    Bass,
    Lead,
    Kick,
    Hats,
    Clap,
    Perc,
    Fx,
}

impl Role {
    pub fn is_drum(self) -> bool {
        matches!(self, Role::Kick | Role::Hats | Role::Clap | Role::Perc | Role::Fx)
    }
}

/// Kind of a section.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum SectionKind {
    Intro,
    #[default]
    Teil,
    Aufbau,
    Break,
    Hoehepunkt,
    Outro,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Layer {
    pub key: String,
    pub name: String,
    pub role: Role,
    /// Instrument (default: synth for pad/arpeggio/lead, bass, drums)
    #[serde(default)]
    pub kind: Option<InstrumentKind>,
    /// Starting point in the character field
    #[serde(default)]
    pub sound: Option<String>,
    /// Envelope template (synth)
    #[serde(default)]
    pub verlauf: Option<String>,
    /// Starting point in the space field
    #[serde(default)]
    pub raum: Option<String>,
    /// Volume of the track in dB
    #[serde(default)]
    pub gain_db: f32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Section {
    pub name: String,
    pub bars: u32,
    #[serde(default)]
    pub kind: SectionKind,
    /// Level per role (key of the row): 0 off, 1 sparse, 2 full
    #[serde(default)]
    pub levels: BTreeMap<String, u8>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SongPlan {
    /// Identifier of the template the plan comes from
    #[serde(default)]
    pub template: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub tempo: f64,
    /// Key, e.g. "a-Moll"
    pub key: String,
    /// Chords (symbols with German note names, or scale degrees), in rotation
    pub chords: Vec<String>,
    /// Bars per chord
    pub chord_bars: u32,
    /// Sound of the drums (starting point in the character field)
    #[serde(default)]
    pub drums: Option<String>,
    /// Swing of the drums in percent
    #[serde(default)]
    pub swing: f64,
    pub layers: Vec<Layer>,
    pub sections: Vec<Section>,
}

impl SongPlan {
    pub fn total_bars(&self) -> u32 {
        self.sections.iter().map(|s| s.bars).sum()
    }

    /// Validates the plan; returns key and chords.
    pub fn check(&self) -> Result<(klang_music::theory::Key, Vec<kh::Chord>), String> {
        let key = kh::parse_key(&self.key).ok_or_else(|| tr!("Key \"{key}\" not understood – e.g. \"A minor\", \"C major\"", key = self.key))?;
        if self.chords.is_empty() {
            return Err(tr!("At least one chord"));
        }
        let chords = self
            .chords
            .iter()
            .map(|c| kh::parse_chord(c, &key).ok_or_else(|| tr!("Chord \"{chord}\" not understood – e.g. Am, F, C, G7 or degrees like i, VI, III, VII", chord = c)))
            .collect::<Result<Vec<_>, _>>()?;
        if !(40.0..=220.0).contains(&self.tempo) {
            return Err(tr!("Tempo {tempo} – allowed: 40–220 BPM", tempo = self.tempo));
        }
        if self.sections.is_empty() {
            return Err(tr!("At least one section"));
        }
        if self.sections.iter().any(|s| s.bars == 0 || s.bars > 64) {
            return Err(tr!("Sections have 1–64 bars"));
        }
        if self.total_bars() > MAX_BARS {
            return Err(tr!("At most {max} bars", max = MAX_BARS));
        }
        let mut seen = std::collections::BTreeSet::new();
        for l in &self.layers {
            if !seen.insert(l.key.as_str()) {
                return Err(tr!("Role \"{role}\" appears twice", role = l.key));
            }
        }
        for s in &self.sections {
            for k in s.levels.keys() {
                if !self.layers.iter().any(|l| &l.key == k) {
                    return Err(tr!("Section \"{section}\": unknown role \"{role}\"", section = s.name, role = k));
                }
            }
        }
        Ok((key, chords))
    }
}

// ---------------------------------------------------------------- Templates

#[allow(clippy::too_many_arguments)]
fn layer(key: &str, name: &str, role: Role, kind: Option<InstrumentKind>, sound: Option<&str>, verlauf: Option<&str>, raum: Option<&str>, gain_db: f32) -> Layer {
    Layer {
        key: key.into(),
        name: name.into(),
        role,
        kind,
        sound: sound.map(Into::into),
        verlauf: verlauf.map(Into::into),
        raum: raum.map(Into::into),
        gain_db,
    }
}

fn section(name: &str, bars: u32, kind: SectionKind, levels: &[(&str, u8)]) -> Section {
    Section { name: name.into(), bars, kind, levels: levels.iter().map(|(k, v)| (k.to_string(), *v)).collect() }
}

fn drum_layers() -> Vec<Layer> {
    vec![
        layer("kick", &tr!("Kick"), Role::Kick, None, None, None, None, 0.0),
        layer("hats", &tr!("Hi-hats"), Role::Hats, None, None, None, None, 0.0),
        layer("clap", &tr!("Clap"), Role::Clap, None, None, None, None, 0.0),
        layer("perc", &tr!("Percussion"), Role::Perc, None, None, None, None, 0.0),
        layer("fx", &tr!("Transitions"), Role::Fx, None, None, None, None, 0.0),
    ]
}

use SectionKind::*;

/// The built-in templates. Description, role and section names in
/// the chosen language; identifiers (template, role keys, sounds,
/// envelopes, rooms), key, chords and the templates' proper names stay.
pub fn templates() -> Vec<SongPlan> {
    let synth = Some(InstrumentKind::Synth);
    let bass = Some(InstrumentKind::Bass);
    let pluck = Some(InstrumentKind::Pluck);
    vec![
        SongPlan {
            template: "trance".into(),
            name: "Trance".into(),
            description: tr!("Only pads at first, then arpeggio and kick, step by step bass, hi-hats and clap – a break with lead, a build-up with snare roll, two peaks, outro."),
            tempo: 138.0,
            key: "a-Moll".into(),
            chords: vec!["a".into(), "F".into(), "C".into(), "G".into()],
            chord_bars: 2,
            drums: Some("Techno".into()),
            swing: 0.0,
            layers: [
                vec![
                    layer("pad", &tr!("Pad"), Role::Pad, synth, Some("Ozean"), Some("Anschwellen"), Some("Saal"), -9.0),
                    layer("arp", &tr!("Arpeggio"), Role::Arp, synth, Some("Glas"), Some("Stich"), Some("Bühne"), -11.0),
                    layer("bass", &tr!("Bass"), Role::Bass, bass, Some("Pluck-Bass"), None, None, -5.0),
                    layer("lead", &tr!("Lead"), Role::Lead, synth, Some("Streicher"), Some("Aufblühen"), Some("Saal"), -9.0),
                ],
                drum_layers(),
            ]
            .concat(),
            sections: vec![
                section(&tr!("Intro"), 16, Intro, &[("pad", 1)]),
                section(&tr!("Subject"), 16, Teil, &[("pad", 2), ("arp", 1), ("kick", 1)]),
                section(&tr!("Build-up"), 16, Aufbau, &[("pad", 2), ("arp", 2), ("bass", 1), ("kick", 1), ("hats", 1), ("fx", 1)]),
                section(&tr!("Break"), 16, Break, &[("pad", 2), ("lead", 1)]),
                section(&tr!("Rise"), 8, Aufbau, &[("arp", 2), ("lead", 1), ("kick", 1), ("hats", 1), ("clap", 1), ("fx", 2)]),
                section(&tr!("Peak"), 16, Hoehepunkt, &[("pad", 2), ("arp", 2), ("bass", 2), ("lead", 2), ("kick", 2), ("hats", 2), ("clap", 2), ("fx", 1)]),
                section(&tr!("Peak 2"), 16, Hoehepunkt, &[("pad", 2), ("arp", 2), ("bass", 2), ("lead", 2), ("kick", 2), ("hats", 2), ("clap", 2), ("perc", 1)]),
                section(&tr!("Outro"), 16, Outro, &[("pad", 1), ("arp", 1), ("kick", 1), ("hats", 1)]),
            ],
        },
        SongPlan {
            template: "ambient".into(),
            name: "Ambient".into(),
            description: tr!("Wide pads, a floating arpeggio, later a soft pulse and a gentle beat – with quiet moments in between."),
            tempo: 84.0,
            key: "e-Moll".into(),
            chords: vec!["e".into(), "C".into(), "G".into(), "D".into()],
            chord_bars: 4,
            drums: Some("Lo-Fi".into()),
            swing: 0.0,
            layers: [
                vec![
                    layer("pad", &tr!("Pad"), Role::Pad, synth, Some("Nebel"), Some("Atmen"), Some("Kirche"), -8.0),
                    layer("arp", &tr!("Sparkle"), Role::Arp, pluck, Some("Kristall"), None, Some("Saal"), -12.0),
                    layer("bass", &tr!("Bass"), Role::Bass, bass, Some("Sub-Bass"), None, None, -7.0),
                    layer("lead", &tr!("Choir"), Role::Lead, synth, Some("Chor"), Some("Aufblühen"), Some("Kirche"), -10.0),
                ],
                drum_layers(),
            ]
            .concat(),
            sections: vec![
                section(&tr!("Morning"), 8, Intro, &[("pad", 1)]),
                section(&tr!("Expanse"), 16, Teil, &[("pad", 2), ("arp", 1)]),
                section(&tr!("Pulse"), 16, Aufbau, &[("pad", 2), ("arp", 1), ("bass", 1), ("perc", 1)]),
                section(&tr!("Heart"), 16, Hoehepunkt, &[("pad", 2), ("arp", 2), ("bass", 1), ("lead", 1), ("kick", 1), ("hats", 1), ("perc", 1)]),
                section(&tr!("Stillness"), 8, Break, &[("pad", 1), ("lead", 1)]),
                section(&tr!("Heart 2"), 16, Hoehepunkt, &[("pad", 2), ("arp", 2), ("bass", 2), ("lead", 2), ("kick", 1), ("hats", 2), ("perc", 1)]),
                section(&tr!("Release"), 16, Outro, &[("pad", 1), ("arp", 1)]),
            ],
        },
        SongPlan {
            template: "deep-house".into(),
            name: "Deep House".into(),
            description: tr!("Starts with kick and hi-hats, the groove grows with clap and bass, then the chords come in – break, drop, outro."),
            tempo: 122.0,
            key: "c-Moll".into(),
            chords: vec!["c".into(), "As".into(), "Es".into(), "B".into()],
            chord_bars: 2,
            drums: Some("House".into()),
            swing: 0.0,
            layers: [
                vec![
                    layer("pad", &tr!("Chords"), Role::Pad, synth, Some("Wärme"), Some("Pulsieren"), Some("Bühne"), -10.0),
                    layer("arp", &tr!("Stabs"), Role::Arp, pluck, Some("Warm"), None, Some("Studio"), -12.0),
                    layer("bass", &tr!("Bass"), Role::Bass, bass, Some("Finger"), None, None, -4.0),
                    layer("lead", &tr!("Melody"), Role::Lead, synth, Some("Glas"), Some("Glocke"), Some("Saal"), -11.0),
                ],
                drum_layers(),
            ]
            .concat(),
            sections: vec![
                section(&tr!("Intro"), 16, Intro, &[("kick", 1), ("hats", 1)]),
                section(&tr!("Groove"), 16, Teil, &[("kick", 2), ("hats", 2), ("clap", 1), ("bass", 1), ("perc", 1)]),
                section(&tr!("Chords"), 16, Aufbau, &[("pad", 1), ("bass", 1), ("kick", 2), ("hats", 2), ("clap", 1), ("perc", 1), ("fx", 1)]),
                section(&tr!("Break"), 8, Break, &[("pad", 2), ("lead", 1)]),
                section(&tr!("Drop"), 16, Hoehepunkt, &[("pad", 2), ("arp", 1), ("bass", 2), ("lead", 2), ("kick", 2), ("hats", 2), ("clap", 2), ("perc", 2), ("fx", 1)]),
                section(&tr!("Outro"), 16, Outro, &[("kick", 1), ("hats", 1), ("perc", 1)]),
            ],
        },
        SongPlan {
            template: "synthwave".into(),
            name: "Synthwave".into(),
            description: tr!("Eighties pads and a pulsing arpeggio, verse and chorus alternating with a punchy snare."),
            tempo: 104.0,
            key: "f-Moll".into(),
            chords: vec!["f".into(), "Des".into(), "As".into(), "Es".into()],
            chord_bars: 2,
            drums: Some("Rock".into()),
            swing: 0.0,
            layers: [
                vec![
                    layer("pad", &tr!("Pad"), Role::Pad, synth, Some("Wärme"), Some("Anschwellen"), Some("Saal"), -9.0),
                    layer("arp", &tr!("Arpeggio"), Role::Arp, synth, Some("Puls"), Some("Gate"), Some("Bühne"), -12.0),
                    layer("bass", &tr!("Bass"), Role::Bass, bass, Some("Pluck-Bass"), None, None, -5.0),
                    layer("lead", &tr!("Lead"), Role::Lead, synth, Some("Streicher"), Some("Aufblühen"), Some("Saal"), -9.0),
                ],
                drum_layers(),
            ]
            .concat(),
            sections: vec![
                section(&tr!("Intro"), 8, Intro, &[("pad", 1), ("arp", 1)]),
                section(&tr!("Verse"), 16, Teil, &[("pad", 1), ("arp", 2), ("bass", 1), ("kick", 1), ("hats", 1), ("clap", 1)]),
                section(&tr!("Chorus"), 16, Hoehepunkt, &[("pad", 2), ("arp", 2), ("bass", 2), ("lead", 2), ("kick", 2), ("hats", 2), ("clap", 2), ("fx", 1)]),
                section(&tr!("Verse 2"), 16, Teil, &[("pad", 1), ("arp", 2), ("bass", 1), ("kick", 1), ("hats", 1), ("clap", 1), ("perc", 1)]),
                section(&tr!("Bridge"), 8, Aufbau, &[("pad", 2), ("arp", 2), ("lead", 1), ("kick", 1), ("fx", 2)]),
                section(&tr!("Chorus 2"), 16, Hoehepunkt, &[("pad", 2), ("arp", 2), ("bass", 2), ("lead", 2), ("kick", 2), ("hats", 2), ("clap", 2), ("perc", 1), ("fx", 1)]),
                section(&tr!("Outro"), 8, Outro, &[("pad", 1), ("arp", 1)]),
            ],
        },
        SongPlan {
            template: "lofi".into(),
            name: "Lo-Fi".into(),
            description: tr!("Warm chords and muted plucks, a laid-back swung beat – relaxed and ready to build on."),
            tempo: 82.0,
            key: "F-Dur".into(),
            chords: vec!["F".into(), "e".into(), "d".into(), "C".into()],
            chord_bars: 1,
            drums: Some("Lo-Fi".into()),
            swing: 35.0,
            layers: [
                vec![
                    layer("pad", &tr!("Chords"), Role::Pad, synth, Some("Wärme"), Some("Atmen"), Some("Studio"), -9.0),
                    layer("arp", &tr!("Pluck"), Role::Arp, pluck, Some("Gedämpft"), None, Some("Studio"), -11.0),
                    layer("bass", &tr!("Bass"), Role::Bass, bass, Some("Finger"), None, None, -5.0),
                    layer("lead", &tr!("Melody"), Role::Lead, pluck, Some("Warm"), None, Some("Bühne"), -10.0),
                ],
                drum_layers(),
            ]
            .concat(),
            sections: vec![
                section(&tr!("Intro"), 4, Intro, &[("pad", 1), ("arp", 1)]),
                section(&tr!("Part A"), 8, Teil, &[("pad", 2), ("arp", 1), ("bass", 1), ("kick", 1), ("hats", 1), ("clap", 1)]),
                section(&tr!("Part B"), 8, Hoehepunkt, &[("pad", 2), ("arp", 2), ("bass", 2), ("lead", 1), ("kick", 2), ("hats", 2), ("clap", 2), ("perc", 1)]),
                section(&tr!("Pause"), 4, Break, &[("pad", 1), ("lead", 1)]),
                section(&tr!("Part B 2"), 8, Hoehepunkt, &[("pad", 2), ("arp", 2), ("bass", 2), ("lead", 2), ("kick", 2), ("hats", 2), ("clap", 2), ("perc", 1)]),
                section(&tr!("Outro"), 4, Outro, &[("pad", 1), ("arp", 1)]),
            ],
        },
    ]
}

pub fn template(id: &str) -> Result<SongPlan, String> {
    let all = templates();
    let want = id.trim().to_lowercase();
    all.iter()
        .find(|t| t.template == want || t.name.to_lowercase() == want)
        .cloned()
        .ok_or_else(|| tr!("No song template \"{id}\" (available: {list})", id = id, list = all.iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join(", ")))
}

// ---------------------------------------------------------------- Notes

/// Volume and brightness curve of a section at position `t` (0…1).
fn shape(kind: SectionKind, t: f64) -> (f64, f32) {
    match kind {
        Aufbau => (0.55 + 0.45 * t, (-0.7 + 1.1 * t) as f32),
        Outro => (1.0 - 0.45 * t, (0.2 - 0.9 * t) as f32),
        Intro => (0.75 + 0.2 * t, (-0.4 + 0.4 * t) as f32),
        Break => (0.85, -0.2),
        Hoehepunkt => (1.0, 0.25),
        Teil => (0.92, 0.0),
    }
}

fn vel(v: f64) -> u8 {
    v.round().clamp(1.0, 127.0) as u8
}

/// Chord tones near `center`, ascending (root, third, fifth …).
fn voiced(chord: &kh::Chord, center: i32) -> Vec<u8> {
    let mut out: Vec<u8> = chord
        .pcs()
        .iter()
        .map(|&pc| {
            let mut m = center - 6 + ((pc as i32 - (center - 6)).rem_euclid(12));
            if m < center - 6 {
                m += 12;
            }
            m.clamp(0, 127) as u8
        })
        .collect();
    out.sort_unstable();
    out
}

struct Ctx<'a> {
    chords: &'a [kh::Chord],
    chord_bars: u32,
}

impl Ctx<'_> {
    fn chord_at(&self, bar: u32) -> &kh::Chord {
        &self.chords[((bar / self.chord_bars.max(1)) as usize) % self.chords.len()]
    }
}

fn note(pitch: u8, start: f64, len: f64, velocity: u8, brightness: f32) -> Note {
    Note { pitch, start_beats: start, length_beats: len, velocity, brightness: brightness.clamp(-1.0, 1.0), ..Default::default() }
}

/// Notes of a melodic role in a section (relative to the start of the section).
fn melodic(role: Role, level: u8, sec: &Section, first_bar: u32, ctx: &Ctx) -> Vec<Note> {
    let mut out = Vec::new();
    let bars = sec.bars;
    let total = bars as f64 * BEATS_PER_BAR;
    for b in 0..bars {
        let chord = ctx.chord_at(first_bar + b);
        let bar0 = b as f64 * BEATS_PER_BAR;
        let at = |beat: f64| shape(sec.kind, (bar0 + beat) / total);
        match role {
            Role::Pad => {
                // one chord per chord change, held; full adds a low root
                let span = ctx.chord_bars.max(1);
                if (first_bar + b) % span != 0 && b != 0 {
                    continue;
                }
                let left = span - ((first_bar + b) % span);
                let len = (left.min(bars - b)) as f64 * BEATS_PER_BAR - 0.05;
                let (d, br) = at(0.0);
                let base = if level >= 2 { 82.0 } else { 64.0 };
                for p in voiced(chord, 62) {
                    out.push(note(p, bar0, len, vel(base * d), br));
                }
                if level >= 2 {
                    out.push(note(voiced(chord, 50)[0], bar0, len, vel(base * 0.85 * d), br));
                }
            }
            Role::Arp => {
                let tones = voiced(chord, 62);
                let mut seq: Vec<u8> = tones.clone();
                seq.push(tones[0] + 12);
                if level >= 2 {
                    seq.extend(tones.iter().skip(1).map(|t| t + 12));
                    let down: Vec<u8> = seq.iter().rev().skip(1).take(seq.len().saturating_sub(2)).copied().collect();
                    seq.extend(down);
                }
                let step = if level >= 2 { 0.25 } else { 0.5 };
                let n = (BEATS_PER_BAR / step) as usize;
                for i in 0..n {
                    let beat = i as f64 * step;
                    let (d, br) = at(beat);
                    let accent = if (beat.fract()).abs() < 1e-9 { 1.0 } else { 0.85 };
                    out.push(note(seq[i % seq.len()], bar0 + beat, step * 0.9, vel(82.0 * d * accent), br));
                }
            }
            Role::Bass => {
                let root = {
                    let pc = chord.bass_pc() as i32;
                    let mut m = 36 + pc;
                    if m > 43 {
                        m -= 12;
                    }
                    m as u8
                };
                // sparse: eighths on the "and"; full: rolling sixteenths (the beat stays the kick)
                let hits: Vec<(f64, f64)> = if level >= 2 {
                    (0..4).flat_map(|q| [0.25, 0.5, 0.75].map(move |o| (q as f64 + o, 0.2))).collect()
                } else {
                    (0..4).map(|q| (q as f64 + 0.5, 0.42)).collect()
                };
                for (beat, len) in hits {
                    let (d, br) = at(beat);
                    out.push(note(root, bar0 + beat, len, vel(100.0 * d), br * 0.5));
                }
            }
            Role::Lead => {
                // motif over two bars from chord tones; sparse only every other bar
                const A: &[(f64, f64, usize)] = &[(0.0, 0.5, 2), (0.75, 0.25, 1), (1.5, 0.5, 2), (2.5, 0.25, 3), (3.0, 0.75, 1)];
                const B: &[(f64, f64, usize)] = &[(0.0, 0.75, 3), (1.0, 0.25, 2), (1.5, 0.5, 1), (2.0, 1.0, 0), (3.5, 0.5, 1)];
                let phrase = if (first_bar + b) % 2 == 0 { A } else { B };
                if level < 2 && (first_bar + b) % 2 == 1 {
                    continue;
                }
                let tones = voiced(chord, 74);
                let pick = |i: usize| -> u8 {
                    let t = &tones;
                    if i < t.len() { t[i] } else { t[i % t.len()] + 12 }
                };
                for &(beat, len, idx) in phrase {
                    let (d, br) = at(beat);
                    out.push(note(pick(idx), bar0 + beat, len * 0.95, vel(92.0 * d), br + 0.2));
                }
            }
            _ => {}
        }
    }
    out
}

/// Beats of a drum role as grid rows per bar (steps 0–15).
fn drum_steps(role: Role, level: u8) -> Vec<(&'static str, Vec<usize>)> {
    let q = vec![0, 4, 8, 12];
    let off = vec![2, 6, 10, 14];
    match role {
        Role::Kick => vec![("kick", q)],
        Role::Hats if level >= 2 => vec![("hho", off), ("hhc", (0..16).filter(|s| s % 4 != 2).collect())],
        Role::Hats => vec![("hho", off)],
        Role::Clap if level >= 2 => vec![("clap", vec![4, 12]), ("snare", vec![4, 12, 15])],
        Role::Clap => vec![("clap", vec![4, 12])],
        Role::Perc if level >= 2 => vec![("shk", (0..16).collect()), ("rim", vec![3, 10])],
        Role::Perc => vec![("shk", (0..16).step_by(2).collect())],
        _ => vec![],
    }
}

/// Drums of a section (all drum roles together).
fn drum_notes(plan: &SongPlan, sec: &Section) -> Vec<Note> {
    let bars = sec.bars;
    let total = bars as f64 * BEATS_PER_BAR;
    let mut grid = rhythm::Grid { steps: (bars as usize) * rhythm::STEPS_PER_BAR, rows: BTreeMap::new() };
    let mut fx = 0u8;
    let mut kick_level = 0u8;
    for l in plan.layers.iter().filter(|l| l.role.is_drum()) {
        let level = sec.levels.get(&l.key).copied().unwrap_or(0);
        if level == 0 {
            continue;
        }
        if l.role == Role::Fx {
            fx = level;
            continue;
        }
        if l.role == Role::Kick {
            kick_level = level;
        }
        for (row, steps) in drum_steps(l.role, level) {
            let e = grid.rows.entry(row.to_string()).or_default();
            for b in 0..bars as usize {
                e.extend(steps.iter().map(|s| b * rhythm::STEPS_PER_BAR + s));
            }
        }
    }
    // Snare roll at the end of a build-up: eighths in the second-to-last bar, sixteenths in the last
    let roll_bars = if fx >= 2 { 4 } else { 2 };
    if fx > 0 && sec.kind == Aufbau && bars >= 2 {
        let e = grid.rows.entry("snare".to_string()).or_default();
        let from = bars.saturating_sub(roll_bars) as usize;
        for b in from..bars as usize {
            let left = bars as usize - b;
            let step = match left {
                1 => 1,
                2 => 2,
                _ => 4,
            };
            e.extend((0..16).step_by(step).map(|s| b * rhythm::STEPS_PER_BAR + s));
        }
    }
    for v in grid.rows.values_mut() {
        v.sort_unstable();
        v.dedup();
    }
    let mut notes = rhythm::grid_notes(&grid, plan.swing);
    // Cymbal at the start of a section (except in the intro)
    if fx > 0 && sec.kind != Intro {
        notes.push(note(drums::CRASH, 0.0, 0.5, 104, 0.0));
    }
    // Progression of the section: build-up gets louder (the roll especially), outro quieter
    for n in notes.iter_mut() {
        let (d, _) = shape(sec.kind, n.start_beats / total);
        let roll = n.pitch == drums::SNARE && sec.kind == Aufbau && n.start_beats >= total - roll_bars as f64 * BEATS_PER_BAR;
        if roll && fx > 0 {
            // Roll: from quiet to strong, evenly over its length
            let r = (n.start_beats - (total - roll_bars as f64 * BEATS_PER_BAR)) / (roll_bars as f64 * BEATS_PER_BAR);
            n.velocity = vel(40.0 + 80.0 * r);
        } else {
            n.velocity = vel(n.velocity as f64 * d.max(0.3));
        }
    }
    // full kick: a bit more punch
    if kick_level >= 2 {
        for n in notes.iter_mut().filter(|n| n.pitch == drums::KICK) {
            n.velocity = n.velocity.saturating_add(8).min(127);
        }
    }
    notes.sort_by(|a, b| a.start_beats.total_cmp(&b.start_beats).then(a.pitch.cmp(&b.pitch)));
    notes
}

/// A generated track.
pub struct GenTrack {
    pub name: String,
    pub instrument: Instrument,
    pub gain_db: f32,
    /// (name, start in beats, length, notes)
    pub clips: Vec<(String, f64, f64, Vec<Note>)>,
}

fn layer_instrument(l: &Layer) -> Instrument {
    let kind = l.kind.unwrap_or(match l.role {
        Role::Bass => InstrumentKind::Bass,
        r if r.is_drum() => InstrumentKind::Drums,
        _ => InstrumentKind::Synth,
    });
    let mut inst = sound::default_instrument(kind);
    if let Some(p) = &l.sound {
        if let Ok((x, y)) = sound::field_preset(kind, "charakter", p) {
            let _ = sound::apply_field(&mut inst, "charakter", x, y);
            inst.preset = p.clone();
        }
    }
    if let Some(r) = &l.raum {
        if let Ok((x, y)) = sound::field_preset(kind, "raum", r) {
            let _ = sound::apply_field(&mut inst, "raum", x, y);
        }
    }
    if let Some(v) = &l.verlauf {
        let _ = sound::apply_verlauf(&mut inst, v);
    }
    inst
}

/// All tracks of a plan.
pub fn generate(plan: &SongPlan) -> Result<Vec<GenTrack>, String> {
    let (_, chords) = plan.check()?;
    let ctx = Ctx { chords: &chords, chord_bars: plan.chord_bars.max(1) };
    let starts: Vec<u32> = plan.sections.iter().scan(0u32, |acc, s| {
        let at = *acc;
        *acc += s.bars;
        Some(at)
    }).collect();
    let mut out = Vec::new();
    for l in plan.layers.iter().filter(|l| !l.role.is_drum()) {
        let mut clips = Vec::new();
        for (sec, &first) in plan.sections.iter().zip(&starts) {
            let level = sec.levels.get(&l.key).copied().unwrap_or(0);
            if level == 0 {
                continue;
            }
            let notes = melodic(l.role, level, sec, first, &ctx);
            clips.push((sec.name.clone(), first as f64 * BEATS_PER_BAR, sec.bars as f64 * BEATS_PER_BAR, notes));
        }
        if !clips.is_empty() {
            out.push(GenTrack { name: l.name.clone(), instrument: layer_instrument(l), gain_db: l.gain_db, clips });
        }
    }
    // all drums into one track
    let mut drum_clips = Vec::new();
    for (sec, &first) in plan.sections.iter().zip(&starts) {
        let any = plan.layers.iter().any(|l| l.role.is_drum() && sec.levels.get(&l.key).copied().unwrap_or(0) > 0);
        if !any {
            continue;
        }
        drum_clips.push((sec.name.clone(), first as f64 * BEATS_PER_BAR, sec.bars as f64 * BEATS_PER_BAR, drum_notes(plan, sec)));
    }
    if !drum_clips.is_empty() {
        let mut inst = sound::default_instrument(InstrumentKind::Drums);
        if let Some(p) = &plan.drums {
            if let Ok((x, y)) = sound::field_preset(InstrumentKind::Drums, "charakter", p) {
                let _ = sound::apply_field(&mut inst, "charakter", x, y);
                inst.preset = p.clone();
            }
        }
        out.push(GenTrack { name: tr!("Beat"), instrument: inst, gain_db: -2.0, clips: drum_clips });
    }
    Ok(out)
}

// ---------------------------------------------------------------- in the project

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SongWritten {
    pub snapshot: Snapshot,
    pub track_ids: Vec<Id>,
    /// replaced (most recently generated) tracks
    pub replaced: usize,
    pub bars: u32,
    pub seconds: f64,
}

impl AppState {
    /// Writes the song: sets tempo and 4/4, replaces the most recently generated
    /// song tracks (if they still exist) and creates the new ones – one undo step.
    pub fn song_write(&self, plan: &SongPlan, replace: bool, label_prefix: &str) -> CmdResult<SongWritten> {
        let tracks = generate(plan)?;
        let previous: Vec<Id> = if replace { lock(&self.song_tracks)?.clone() } else { Vec::new() };
        let mut commands = vec![Command::SetProject { patch: ProjectPatch { tempo_bpm: Some(plan.tempo), beats_per_bar: Some(4), beat_unit: Some(4), ..Default::default() } }];
        let mut ids = Vec::new();
        let replaced;
        {
            let mut s = lock(&self.session)?;
            let p = &mut s.project;
            let present: Vec<Id> = previous.iter().copied().filter(|id| p.track(*id).is_some()).collect();
            replaced = present.len();
            commands.extend(present.into_iter().map(|track_id| Command::RemoveTrack { track_id }));
            for g in tracks {
                let id = p.alloc_id();
                let mut t = Track::new(id, g.name, TrackKind::Instrument);
                t.instrument = Some(g.instrument);
                t.gain_db = g.gain_db;
                // flat rows: the whole song at a glance
                t.view = TrackView::Compact;
                for (name, start, len, notes) in g.clips {
                    t.clips.push(Clip { id: p.alloc_id(), name, start_beats: start, source: ClipSource::Notes { length_beats: len, notes } });
                }
                ids.push(id);
                commands.push(Command::AddTrack { track: t, index: None });
            }
        }
        let label = if replaced > 0 {
            tr!("{prefix}Regenerate song \"{name}\"", prefix = label_prefix, name = plan.name)
        } else {
            tr!("{prefix}Generate song \"{name}\"", prefix = label_prefix, name = plan.name)
        };
        let snapshot = self.execute(Command::Batch { label, commands }, false)?;
        *lock(&self.song_tracks)? = ids.clone();
        let bars = plan.total_bars();
        Ok(SongWritten { snapshot, track_ids: ids, replaced, bars, seconds: bars as f64 * BEATS_PER_BAR * 60.0 / plan.tempo })
    }
}


/// Levels of a section as text for the agent, e.g. "pad 2 · arp 1 · kick 1".
pub fn levels_text(sec: &Section, plan: &SongPlan) -> String {
    plan.layers
        .iter()
        .filter_map(|l| sec.levels.get(&l.key).filter(|v| **v > 0).map(|v| format!("{} {v}", l.key)))
        .collect::<Vec<_>>()
        .join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_template_is_valid_and_generates() {
        for t in templates() {
            t.check().unwrap_or_else(|e| panic!("{}: {e}", t.name));
            let tracks = generate(&t).unwrap();
            assert!(tracks.iter().any(|g| g.name == "Beat"), "{}: Schlagzeug", t.name);
            // each track: clips on the grid of the sections, all notes within their clip
            for g in &tracks {
                for (name, start, len, notes) in &g.clips {
                    assert!(start % 4.0 == 0.0 && *len > 0.0, "{} {name}", g.name);
                    assert!(notes.iter().all(|n| n.start_beats >= 0.0 && n.start_beats < *len && n.velocity > 0), "{} {name}", g.name);
                }
            }
            for l in &t.layers {
                if let Some(s) = &l.sound {
                    let kind = layer_instrument(l).kind;
                    assert!(sound::field_preset(kind, "charakter", s).is_ok(), "{}: Klang {s}", t.name);
                }
            }
        }
    }

    #[test]
    fn trance_grows_from_pads_to_full() {
        let t = template("Trance").unwrap();
        let tracks = generate(&t).unwrap();
        let names: Vec<&str> = tracks.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, ["Pad", "Arpeggio", "Bass", "Lead", "Beat"]);
        // Intro: only the pad, no beat
        let beat = tracks.iter().find(|g| g.name == "Beat").unwrap();
        assert!(beat.clips.iter().all(|c| c.1 >= 16.0 * 4.0), "Beat erst nach dem Intro");
        assert_eq!(tracks[0].clips[0].0, "Intro");
        // Build-up ends with a snare roll that gets louder
        let aufbau = beat.clips.iter().find(|c| c.0 == "Build-up").unwrap();
        let roll: Vec<&Note> = aufbau.3.iter().filter(|n| n.pitch == drums::SNARE).collect();
        assert!(roll.len() >= 16 + 8, "Wirbel: {}", roll.len());
        assert!(roll.last().unwrap().velocity > roll.first().unwrap().velocity + 30);
        // Climax: crash at the start, more drums than in the theme
        let hp = beat.clips.iter().find(|c| c.0 == "Peak").unwrap();
        assert!(hp.3.iter().any(|n| n.pitch == drums::CRASH && n.start_beats == 0.0));
        let thema = beat.clips.iter().find(|c| c.0 == "Subject").unwrap();
        assert!(hp.3.len() > 3 * thema.3.len());
        // Build-up: arpeggio gets brighter
        let arp = tracks.iter().find(|g| g.name == "Arpeggio").unwrap();
        let a = &arp.clips.iter().find(|c| c.0 == "Build-up").unwrap().3;
        assert!(a.last().unwrap().brightness > a.first().unwrap().brightness + 0.6);
        // Chords follow a – F – C – G with two bars per chord
        let pad = &tracks[0].clips[1].3; // Theme
        let roots: Vec<u8> = pad.iter().filter(|n| n.start_beats % 8.0 == 0.0).map(|n| n.pitch % 12).collect();
        assert!(roots.contains(&9) && roots.contains(&5), "{roots:?}");
    }

    #[test]
    fn levels_change_density() {
        let t = template("trance").unwrap();
        let (_, chords) = t.check().unwrap();
        let ctx = Ctx { chords: &chords, chord_bars: 2 };
        let sec = section("x", 4, Teil, &[]);
        let n1 = melodic(Role::Arp, 1, &sec, 0, &ctx).len();
        let n2 = melodic(Role::Arp, 2, &sec, 0, &ctx).len();
        assert_eq!((n1, n2), (32, 64));
        assert!(melodic(Role::Lead, 1, &sec, 0, &ctx).len() < melodic(Role::Lead, 2, &sec, 0, &ctx).len());
        assert!(drum_steps(Role::Hats, 2).iter().map(|r| r.1.len()).sum::<usize>() > drum_steps(Role::Hats, 1)[0].1.len());
    }

    #[test]
    fn bad_plans_are_explained() {
        let mut t = template("Ambient").unwrap();
        t.key = "Lila".into();
        assert!(t.check().unwrap_err().contains("Key"));
        let mut t = template("Ambient").unwrap();
        t.chords = vec!["Xyz".into()];
        assert!(t.check().unwrap_err().contains("Chord"));
        let mut t = template("Ambient").unwrap();
        t.sections[0].levels.insert("gong".into(), 1);
        assert!(t.check().unwrap_err().contains("gong"));
        assert!(template("Polka").unwrap_err().contains("Trance"));
    }
}
