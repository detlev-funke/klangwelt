//! Rhythmus: Schlagzeug-Muster typischer Genres als Schnelleinstieg und ein
//! Schritt-Raster (16 Schritte je Takt) als Eingabehilfe für Schlagzeug-Clips.
//!
//! Die Muster folgen dem frei verfügbaren Heft „Pocket Operations“ (Schritte
//! 1–16 je Trommel). Das Raster ist ein Blick auf die Noten eines Clips: eine
//! Zeile je Trommel, ein Schritt = eine Sechzehntel. Aus dem Raster werden Noten
//! mit Betonungen (Backbeat laut, Zwischen-Snares leise) und optional Swing
//! (jede zweite Sechzehntel später); aus einem Clip wird das Raster gelesen.
//! Schalten einzelner Schritte ändert nur die Noten auf diesem Schritt.

use crate::sound;
use crate::state::{lock, AppState, CmdResult, Snapshot};
use klang_core::{Clip, ClipPatch, ClipSource, Command, Id, InstrumentKind, Note, ProjectPatch, Track, TrackKind};
use klang_dsp::drums;
use klang_dsp::util;
use klang_i18n::{tid, tk, tr};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Sechzehntel je Takt (4/4).
pub const STEPS_PER_BAR: usize = 16;
const STEP_BEATS: f64 = 0.25;
pub const MAX_BARS: usize = 4;

/// Rows of the grid: id, name (a key, translated on output), key; `main` = always visible.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    pub key: &'static str,
    #[serde(serialize_with = "crate::sound::ser_tr")]
    pub label: &'static str,
    pub midi: u8,
    pub main: bool,
}

pub const ROWS: [Row; 13] = [
    Row { key: "kick", label: tk!("Kick"), midi: drums::KICK, main: true },
    Row { key: "snare", label: tk!("Snare"), midi: drums::SNARE, main: true },
    Row { key: "clap", label: tk!("Clap"), midi: drums::CLAP, main: true },
    Row { key: "rim", label: tk!("Rim"), midi: drums::RIM, main: true },
    Row { key: "hhc", label: tk!("Closed hi-hat"), midi: drums::HAT_CLOSED, main: true },
    Row { key: "hho", label: tk!("Open hi-hat"), midi: drums::HAT_OPEN, main: true },
    Row { key: "crash", label: tk!("Crash"), midi: drums::CRASH, main: false },
    Row { key: "ride", label: tk!("Ride"), midi: drums::RIDE, main: false },
    Row { key: "cow", label: tk!("Cowbell"), midi: drums::COWBELL, main: false },
    Row { key: "tomh", label: tk!("High tom"), midi: drums::TOM_HIGH, main: false },
    Row { key: "tomm", label: tk!("Mid tom"), midi: drums::TOM_MID, main: false },
    Row { key: "toml", label: tk!("Low tom"), midi: drums::TOM_LOW, main: false },
    Row { key: "shk", label: tk!("Shaker"), midi: drums::SHAKER, main: false },
];

fn row(key: &str) -> Option<&'static Row> {
    ROWS.iter().find(|r| r.key == key)
}

/// Zeile zu einer Taste (auch verwandte Tasten: Toms 43/47/50, Crash 57, Ride 59, Maracas/Shaker 82).
fn row_of(midi: u8) -> Option<&'static Row> {
    let m = match midi {
        43 => drums::TOM_LOW,
        47 => drums::TOM_MID,
        50 => drums::TOM_HIGH,
        57 => drums::CRASH,
        59 => drums::RIDE,
        82 => drums::SHAKER,
        40 => drums::SNARE,
        35 => drums::KICK,
        44 => drums::HAT_CLOSED,
        m => m,
    };
    ROWS.iter().find(|r| r.midi == m)
}

// ---------------------------------------------------------------- Muster

struct PatternDef {
    /// Proper name (stays untranslated)
    name: &'static str,
    bpm: u16,
    /// empfohlener Swing in Prozent
    swing: u8,
    /// Key, translated on output
    tip: &'static str,
    /// Kürzel wie im Heft (BD, SN, CL, RS, CH, OH, CY, CB, HT, MT, LT, SH), Schritte 1–16
    rows: &'static [(&'static str, &'static [u8])],
}

struct GenreDef {
    /// Key, translated on output ([`find_pattern`] also knows both versions)
    /// – an identifier (`tid!`): find_pattern matches the agent's genre against this value.
    name: &'static str,
    blurb: &'static str,
    /// Starting point in the character field for a new drum track (an identifier, stays German)
    sound: &'static str,
    patterns: &'static [PatternDef],
}

const E8: &[u8] = &[1, 3, 5, 7, 9, 11, 13, 15];
const ALL16: &[u8] = &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];

const GENRES: &[GenreDef] = &[
    GenreDef {
        name: tid!("Grundlagen"),
        blurb: tk!("This is where it all starts: the kick sets the pulse, the snare sits on beats 2 and 4 (the backbeat), the hi-hat runs through in eighths."),
        sound: "Rock",
        patterns: &[
            PatternDef { name: "Boots ’n’ Cats", bpm: 118, swing: 0, tip: tk!("The classic for practicing – say it along: \"boots and cats and boots and cats\". Kick, hi-hat, snare, hi-hat."), rows: &[("BD", &[1, 9]), ("SN", &[5, 13]), ("CH", E8)] },
            PatternDef { name: "Four on the Floor", bpm: 122, swing: 0, tip: tk!("Kick on every quarter – the foundation of almost all dance music. Open hi-hat on the \"and\"."), rows: &[("BD", &[1, 5, 9, 13]), ("OH", &[3, 7, 11, 15])] },
            PatternDef { name: "Pop Backbeat", bpm: 110, swing: 0, tip: tk!("Snare on 2 and 4 – the heart of every pop and rock song."), rows: &[("BD", &[1, 4, 7, 11]), ("SN", &[5, 13]), ("CH", E8)] },
        ],
    },
    GenreDef {
        name: tid!("House"),
        blurb: tk!("Kick on every quarter, clap on 2 and 4 and the typical open hi-hats on the offbeat. 120–126 BPM."),
        sound: "House",
        patterns: &[
            PatternDef { name: "Classic House", bpm: 124, swing: 0, tip: tk!("The \"tss\" of the open hi-hat between the kicks makes the house groove."), rows: &[("BD", &[1, 5, 9, 13]), ("CL", &[5, 13]), ("OH", &[3, 7, 11, 15])] },
            PatternDef { name: "Deep House", bpm: 122, swing: 0, tip: tk!("Softer and rolling: extra closed hi-hats add movement."), rows: &[("BD", &[1, 5, 9, 13]), ("CL", &[5, 13]), ("CH", &[2, 8, 10]), ("OH", &[3, 7, 11, 15])] },
            PatternDef { name: "French House", bpm: 123, swing: 0, tip: tk!("Continuous hi-hats plus offbeat hats, with the shaker driving things further."), rows: &[("BD", &[1, 5, 9, 13]), ("CL", &[5, 13]), ("CH", E8), ("OH", &[3, 7, 11, 15]), ("SH", &[1, 5, 9, 13])] },
        ],
    },
    GenreDef {
        name: tid!("Techno"),
        blurb: tk!("Driving and hypnotic: kick on every quarter, plus an extra kick just before the end. 130–138 BPM."),
        sound: "Techno",
        patterns: &[
            PatternDef { name: "Techno", bpm: 134, swing: 0, tip: tk!("The kick on step 15 pushes into the next bar."), rows: &[("BD", &[1, 5, 9, 13, 15]), ("OH", &[3, 7, 11, 15]), ("CH", &[10])] },
            PatternDef { name: "Peak-Time Techno", bpm: 136, swing: 0, tip: tk!("Stomping: kick throughout, clap on 2 and 4, hi-hats on all the steps in between."), rows: &[("BD", &[1, 5, 9, 13]), ("CL", &[5, 13]), ("OH", &[3, 7, 11, 15]), ("CH", &[2, 4, 6, 8, 10, 12, 14, 16])] },
        ],
    },
    GenreDef {
        name: tid!("Electro"),
        blurb: tk!("Syncopated kicks instead of four on the floor, but a straight snare on 2 and 4. Around 128 BPM."),
        sound: "808",
        patterns: &[
            PatternDef { name: "Electro", bpm: 128, swing: 0, tip: tk!("The kick jumps around (1, 7, 11, 14) – that gives the nervous electro bounce."), rows: &[("BD", &[1, 7, 11, 14]), ("SN", &[5, 13]), ("CH", E8)] },
            PatternDef { name: "Miami Bass", bpm: 126, swing: 0, tip: tk!("Kick and snare as in electro, hi-hats with sixteenth-note accents."), rows: &[("BD", &[1, 7, 11, 14]), ("SN", &[5, 13]), ("CH", &[1, 3, 4, 5, 7, 8, 9, 11, 12, 13, 15, 16])] },
        ],
    },
    GenreDef {
        name: tid!("Hip-Hop / Boom Bap"),
        blurb: tk!("Relaxed tempo (85–95 BPM), punchy kick-snare grooves, often slightly swung."),
        sound: "Lo-Fi",
        patterns: &[
            PatternDef { name: "Boom Bap", bpm: 88, swing: 20, tip: tk!("Hard kick on 1, snare on 2 and 4 – add a little swing and heads start nodding."), rows: &[("BD", &[1, 7, 11]), ("SN", &[5, 13]), ("CH", E8)] },
            PatternDef { name: "Hip Hop Groove", bpm: 90, swing: 20, tip: tk!("More kicks (1, 3, 7, 8, 15) make the groove chattier."), rows: &[("BD", &[1, 3, 7, 8, 15]), ("SN", &[5, 13]), ("CH", E8)] },
            PatternDef { name: "Lo-Fi", bpm: 82, swing: 35, tip: tk!("Slow and dragging – turn up the swing."), rows: &[("BD", &[1, 8, 11]), ("SN", &[5, 13]), ("CH", &[1, 4, 5, 8, 9, 12, 13, 16]), ("OH", &[7])] },
        ],
    },
    GenreDef {
        name: tid!("Trap"),
        blurb: tk!("Halftime: snare or clap only on beat 3, fast hi-hats. Around 140 BPM, but feels half as fast."),
        sound: "Trap",
        patterns: &[
            PatternDef { name: "Trap", bpm: 140, swing: 0, tip: tk!("Clap only in the middle = halftime; sparse kick, hi-hats with little rolls."), rows: &[("BD", &[1, 7, 13]), ("CL", &[9]), ("CH", &[1, 3, 5, 7, 9, 10, 11, 13, 15, 16])] },
            PatternDef { name: "Drill", bpm: 142, swing: 0, tip: tk!("Sliding kicks (3, 5, 11) and the shifted clap."), rows: &[("BD", &[3, 5, 11]), ("CL", &[9]), ("CH", E8)] },
        ],
    },
    GenreDef {
        name: tid!("Drum & Bass / Jungle"),
        blurb: tk!("Very fast (160–176 BPM), but the snare on 2 and 4 makes it feel half as fast. The Amen break is the centerpiece."),
        sound: "Rock",
        patterns: &[
            PatternDef { name: "Amen Break", bpm: 174, swing: 0, tip: tk!("The many snares (5, 8, 10, 13, 16) create the rolling chaos."), rows: &[("BD", &[1, 3, 11, 12]), ("SN", &[5, 8, 10, 13, 16]), ("CH", E8)] },
            PatternDef { name: "Jungle", bpm: 162, swing: 0, tip: tk!("Edgy and raw: shifted kicks, quiet in-between snares, open hi-hat on 1."), rows: &[("BD", &[1, 3, 11]), ("SN", &[5, 8, 10, 15]), ("CH", E8), ("OH", &[1])] },
            PatternDef { name: "Liquid D&B", bpm: 174, swing: 0, tip: tk!("A clear two-step of kick and snare – good for getting into the style."), rows: &[("BD", &[1, 4, 8, 10, 11, 16]), ("SN", &[5, 13]), ("CH", E8)] },
        ],
    },
    GenreDef {
        name: tid!("Funk / Breakbeat"),
        blurb: tk!("The groove lives on quiet in-between snares (ghost notes) and syncopated kicks. 95–130 BPM; a little swing helps."),
        sound: "Disco",
        patterns: &[
            PatternDef { name: "Funky Drummer", bpm: 100, swing: 15, tip: tk!("The many quiet snares between the backbeats work the magic."), rows: &[("BD", &[1, 3, 7, 11, 14]), ("SN", &[5, 8, 10, 12, 13, 16]), ("CH", &[1, 2, 3, 4, 5, 6, 7, 9, 10, 11, 12, 13, 15, 16]), ("OH", &[8, 14])] },
            PatternDef { name: "Impeach the President", bpm: 95, swing: 15, tip: tk!("Simple and dry, with the open hi-hat on 11 as its signature."), rows: &[("BD", &[1, 8, 9, 15]), ("SN", &[5, 13]), ("CH", &[1, 3, 5, 7, 8, 9, 13, 15]), ("OH", &[11])] },
            PatternDef { name: "Big Beat", bpm: 128, swing: 0, tip: tk!("Fat and wide: kick up front, hi-hat only on 2 and 4."), rows: &[("BD", &[1, 4, 7, 9]), ("SN", &[5, 13]), ("CH", &[5, 13])] },
        ],
    },
    GenreDef {
        name: tid!("Rock / Pop"),
        blurb: tk!("Straight backbeat, steady eighth-note hi-hats, crash on the 1. 110–130 BPM."),
        sound: "Rock",
        patterns: &[
            PatternDef { name: "Rock", bpm: 120, swing: 0, tip: tk!("The standard: kick on 1, snare on 2 and 4, crash at the start."), rows: &[("BD", &[1, 8, 9, 11]), ("SN", &[5, 13]), ("CH", E8), ("CY", &[1])] },
            PatternDef { name: "Pop", bpm: 110, swing: 0, tip: tk!("A bit more movement in the kick – the basis of countless hits."), rows: &[("BD", &[1, 2, 4, 8, 11]), ("SN", &[5, 13]), ("CH", E8)] },
            PatternDef { name: "Disco", bpm: 120, swing: 0, tip: tk!("Four on the floor with open hi-hats on the offbeat – the forerunner of house."), rows: &[("BD", &[1, 5, 9, 13]), ("SN", &[5, 13]), ("OH", &[3, 7, 11, 15]), ("CH", ALL16)] },
        ],
    },
    GenreDef {
        name: tid!("Reggaeton / Dancehall"),
        blurb: tk!("The dembow: kick on the quarters, snare syncopated on 4, 7, 12, 15. 90–100 BPM."),
        sound: "808",
        patterns: &[
            PatternDef { name: "Reggaeton (Dembow)", bpm: 95, swing: 0, tip: tk!("The snare pattern 4·7 / 12·15 is the dembow rhythm."), rows: &[("BD", &[1, 5, 9, 13]), ("SN", &[4, 7, 12, 15]), ("CH", E8)] },
            PatternDef { name: "Dancehall", bpm: 98, swing: 0, tip: tk!("Related, but sparser in the kick; the rim adds the woody accent."), rows: &[("BD", &[1, 9]), ("SN", &[4, 7, 12, 15]), ("RS", &[3, 11]), ("CH", E8)] },
        ],
    },
    GenreDef {
        name: tid!("Reggae / Dub"),
        blurb: tk!("Relaxed, 70–80 BPM. The one drop leaves out the 1: kick and snare land together on beat 3."),
        sound: "Lo-Fi",
        patterns: &[
            PatternDef { name: "One Drop", bpm: 75, swing: 0, tip: tk!("No kick on 1 – kick and snare together on the 3."), rows: &[("BD", &[9]), ("SN", &[9]), ("CH", E8)] },
            PatternDef { name: "Steppers", bpm: 78, swing: 0, tip: tk!("More driving: kick on every quarter, snare still on the 3."), rows: &[("BD", &[1, 5, 9, 13]), ("SN", &[9]), ("CH", E8)] },
        ],
    },
    GenreDef {
        name: tid!("Latin / Afro-Cuban"),
        blurb: tk!("Everything revolves around the clave – a 3-2 pattern on the rim, with ride and a dancing kick on top. 100–130 BPM."),
        sound: "Disco",
        patterns: &[
            PatternDef { name: "Bossa Nova", bpm: 130, swing: 0, tip: tk!("The rim clave (1, 4, 7, 11, 14) is the backbone; the ride runs through."), rows: &[("RS", &[1, 4, 7, 11, 14]), ("BD", &[1, 4, 5, 8, 9, 12, 13, 16]), ("CY", ALL16)] },
            PatternDef { name: "Son Clave (3-2)", bpm: 100, swing: 0, tip: tk!("The classic 3-2 clave on the rim: X··X··X···X·X···"), rows: &[("RS", &[1, 4, 7, 11, 13]), ("BD", &[1, 9]), ("CH", E8)] },
            PatternDef { name: "Songo", bpm: 110, swing: 0, tip: tk!("Cuban groove: the cowbell leads, the kick is syncopated."), rows: &[("BD", &[1, 5, 9, 13, 15]), ("RS", &[3, 4, 7, 8, 11, 12, 15, 16]), ("CB", &[1, 4, 7, 11, 15])] },
        ],
    },
    GenreDef {
        name: tid!("UK Garage"),
        blurb: tk!("Syncopated and heavily swung: shifted kicks, clap on 2 and 4, edgy hi-hats. 130–138 BPM."),
        sound: "House",
        patterns: &[
            PatternDef { name: "2-Step Garage", bpm: 135, swing: 40, tip: tk!("Kick only on 1 and 11 – the \"skip\". Swing around 40% for that real garage feel."), rows: &[("BD", &[1, 11]), ("CL", &[5, 13]), ("CH", &[3, 4, 7, 11, 15, 16]), ("RS", &[2, 8, 14]), ("MT", &[6, 12])] },
        ],
    },
    GenreDef {
        name: tid!("Dubstep"),
        blurb: tk!("Halftime like trap, but harder: snare on beat 3, sparse kicks, lots of room for the bass. Around 140 BPM."),
        sound: "Trap",
        patterns: &[
            PatternDef { name: "Dubstep", bpm: 140, swing: 0, tip: tk!("Kick on 1, snare on the 3 – in between there's room for the bass."), rows: &[("BD", &[1, 11]), ("SN", &[9]), ("CH", &[2, 3, 7, 12, 15]), ("OH", &[5, 14])] },
        ],
    },
];

/// Kürzel aus dem Heft → Zeile; ein Becken auf höchstens zwei Schritten ist ein Crash, sonst eine Ride.
fn code_row(code: &str, steps: &[u8]) -> &'static str {
    match code {
        "BD" => "kick",
        "SN" => "snare",
        "CL" | "HC" => "clap",
        "RS" => "rim",
        "CH" => "hhc",
        "OH" => "hho",
        "CY" if steps.len() <= 2 => "crash",
        "CY" => "ride",
        "CB" => "cow",
        "HT" => "tomh",
        "MT" => "tomm",
        "LT" => "toml",
        _ => "shk",
    }
}

/// Raster: Länge in Schritten und je Zeile die gesetzten Schritte (ab 0).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Grid {
    pub steps: usize,
    pub rows: BTreeMap<String, Vec<usize>>,
}

impl Grid {
    /// Prüft Zeilen und Länge (1–4 Takte); Schritte sortiert, doppelte entfernt.
    pub fn checked(mut self) -> Result<Grid, String> {
        if self.steps == 0 || self.steps % STEPS_PER_BAR != 0 || self.steps > MAX_BARS * STEPS_PER_BAR {
            return Err(tr!("Length of {steps} steps – allowed: 16, 32, 48 or 64", steps = self.steps));
        }
        for (k, v) in self.rows.iter_mut() {
            if row(k).is_none() {
                return Err(tr!("Unknown drum \"{key}\" (allowed: {list})", key = k, list = ROWS.iter().map(|r| r.key).collect::<Vec<_>>().join(", ")));
            }
            v.sort_unstable();
            v.dedup();
            v.retain(|&s| s < self.steps);
        }
        self.rows.retain(|_, v| !v.is_empty());
        Ok(self)
    }

    /// Wiederholt bzw. kürzt auf `steps`.
    pub fn resized(&self, steps: usize) -> Grid {
        let src = self.steps.max(1);
        let rows = self
            .rows
            .iter()
            .map(|(k, v)| (k.clone(), (0..steps).filter(|s| v.contains(&(s % src))).collect::<Vec<_>>()))
            .filter(|(_, v)| !v.is_empty())
            .collect();
        Grid { steps, rows }
    }
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PatternView {
    pub name: String,
    pub bpm: u16,
    pub swing: u8,
    pub tip: String,
    /// ein Takt (16 Schritte)
    pub grid: Grid,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GenreView {
    pub name: String,
    pub blurb: String,
    pub sound: String,
    pub patterns: Vec<PatternView>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RhythmInfo {
    pub rows: Vec<Row>,
    pub genres: Vec<GenreView>,
}

fn pattern_view(p: &PatternDef) -> PatternView {
    let mut rows: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (code, steps) in p.rows {
        rows.entry(code_row(code, steps).to_string()).or_default().extend(steps.iter().map(|&s| s as usize - 1));
    }
    let grid = Grid { steps: STEPS_PER_BAR, rows }.checked().expect("Muster sind gültig");
    PatternView { name: p.name.into(), bpm: p.bpm, swing: p.swing, tip: klang_i18n::translate(p.tip), grid }
}

fn genre_view(g: &GenreDef) -> GenreView {
    GenreView { name: klang_i18n::translate(g.name), blurb: klang_i18n::translate(g.blurb), sound: g.sound.into(), patterns: g.patterns.iter().map(pattern_view).collect() }
}

pub fn info() -> RhythmInfo {
    RhythmInfo { rows: ROWS.to_vec(), genres: GENRES.iter().map(genre_view).collect() }
}

/// Pattern by genre and name (case-insensitive; the genre may be missing,
/// it counts in German – the identifier – or in the chosen language).
pub fn find_pattern(genre: Option<&str>, name: &str) -> Result<(GenreView, PatternView), String> {
    let all = info().genres;
    let eq = |a: &str, b: &str| a.to_lowercase() == b.trim().to_lowercase();
    let genres: Vec<&GenreView> = match genre {
        Some(g) => {
            let want = g.trim().to_lowercase();
            let matches = |n: &str| eq(n, g) || n.to_lowercase().contains(&want);
            let found: Vec<&GenreView> = all.iter().zip(GENRES).filter(|(x, d)| matches(&x.name) || matches(d.name)).map(|(x, _)| x).collect();
            if found.is_empty() {
                return Err(tr!("No genre \"{genre}\" (available: {list})", genre = g, list = all.iter().map(|x| x.name.as_str()).collect::<Vec<_>>().join(", ")));
            }
            found
        }
        None => all.iter().collect(),
    };
    for g in &genres {
        if let Some(p) = g.patterns.iter().find(|p| eq(&p.name, name)) {
            return Ok(((*g).clone(), p.clone()));
        }
    }
    // nur ein Genre und Name leer bzw. gleich dem Genre: dessen erstes Muster
    if let [g] = genres.as_slice() {
        if name.trim().is_empty() || eq(&g.name, name) {
            return Ok(((*g).clone(), g.patterns[0].clone()));
        }
    }
    let names: Vec<String> = genres.iter().flat_map(|g| g.patterns.iter().map(move |p| format!("{} ({})", p.name, g.name))).collect();
    Err(tr!("No pattern \"{name}\". Available: {list}", name = name, list = names.join(", ")))
}

// ---------------------------------------------------------------- Raster ↔ Noten

/// Verzögerung eines Schritts durch Swing (jede zweite Sechzehntel, bis 2/3 eines Schritts).
fn swing_offset(step: usize, swing: f64) -> f64 {
    if step % 2 == 1 { STEP_BEATS * (swing.clamp(0.0, 100.0) / 100.0) * 0.66 } else { 0.0 }
}

/// Anschlag eines Schritts: Backbeat und Viertel betont, Zwischen-Snares leise
/// (nur wenn der Takt eine Snare auf einer Viertel hat – sonst ist das Muster
/// selbst synkopiert, wie der Dembow).
fn velocity(row: &str, step: usize, snare_on_quarter: bool) -> u8 {
    let quarter = step % 4 == 0;
    let eighth = step % 2 == 0;
    match row {
        "kick" => if quarter { 112 } else { 98 },
        "snare" => if quarter || !snare_on_quarter { 108 } else { 66 },
        "clap" => 104,
        "rim" => 92,
        "hhc" => if eighth { 86 } else { 64 },
        "hho" => 90,
        "crash" => 104,
        "ride" => if quarter { 90 } else { 76 },
        "cow" => 90,
        "shk" => if eighth { 74 } else { 58 },
        _ => 96,
    }
}

fn snare_quarters(grid: &Grid) -> Vec<bool> {
    let bars = grid.steps.div_ceil(STEPS_PER_BAR).max(1);
    let snares = grid.rows.get("snare").cloned().unwrap_or_default();
    (0..bars).map(|b| snares.iter().any(|&s| s / STEPS_PER_BAR == b && s % 4 == 0)).collect()
}

fn step_note(row: &Row, step: usize, swing: f64, vel: u8) -> Note {
    Note {
        pitch: row.midi,
        start_beats: step as f64 * STEP_BEATS + swing_offset(step, swing),
        length_beats: STEP_BEATS * 0.9,
        velocity: vel,
        ..Default::default()
    }
}

/// Noten eines Rasters (ab Beat 0).
pub fn grid_notes(grid: &Grid, swing: f64) -> Vec<Note> {
    let sq = snare_quarters(grid);
    let mut out: Vec<Note> = grid
        .rows
        .iter()
        .filter_map(|(k, v)| row(k).map(|r| (r, v)))
        .flat_map(|(r, v)| v.iter().map(|&s| step_note(r, s, swing, velocity(r.key, s, sq[s / STEPS_PER_BAR]))).collect::<Vec<_>>())
        .collect();
    out.sort_by(|a, b| a.start_beats.total_cmp(&b.start_beats).then(a.pitch.cmp(&b.pitch)));
    out
}

/// Schritt einer Note. Swing verschiebt nur nach hinten (bis 2/3 eines Schritts):
/// bis dahin gehört eine Note hinter einer ungeraden Sechzehntel zu dieser, sonst zum nächsten Schritt.
fn step_of(start_beats: f64) -> Option<usize> {
    let x = start_beats / STEP_BEATS;
    if x < -0.5 {
        return None;
    }
    let f = x.floor();
    let s = if f >= 0.0 && (f as i64) % 2 == 1 && x - f <= 0.67 { f } else { x.round() };
    Some(s.max(0.0) as usize)
}

/// Raster aus Noten; zählt Noten, die keiner Trommel zugeordnet sind oder hinter dem Raster liegen.
pub fn grid_from_notes(notes: &[Note], steps: usize) -> (Grid, usize) {
    let mut rows: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut skipped = 0;
    for n in notes {
        match (row_of(n.pitch), step_of(n.start_beats)) {
            (Some(r), Some(s)) if s < steps => rows.entry(r.key.to_string()).or_default().push(s),
            _ => skipped += 1,
        }
    }
    for v in rows.values_mut() {
        v.sort_unstable();
        v.dedup();
    }
    (Grid { steps, rows }, skipped)
}

/// Setzt oder löscht einen Schritt; andere Noten bleiben, wie sie sind.
pub fn toggle(notes: &[Note], row_key: &str, step: usize, on: bool, swing: f64, steps: usize) -> Result<Vec<Note>, String> {
    let r = row(row_key).ok_or_else(|| tr!("Unknown drum \"{key}\"", key = row_key))?;
    let here = |n: &Note| row_of(n.pitch).map(|x| x.key) == Some(r.key) && step_of(n.start_beats) == Some(step);
    let mut out: Vec<Note> = notes.iter().filter(|n| !here(n)).cloned().collect();
    if on {
        let (grid, _) = grid_from_notes(&out, steps.max(step + 1));
        let sq = snare_quarters(&grid);
        out.push(step_note(r, step, swing, velocity(r.key, step, sq.get(step / STEPS_PER_BAR).copied().unwrap_or(true))));
        out.sort_by(|a, b| a.start_beats.total_cmp(&b.start_beats).then(a.pitch.cmp(&b.pitch)));
    }
    Ok(out)
}

/// Neuer Swing: Noten der Trommeln rücken auf ihren Schritt plus Swing; Anschlag und Ausdruck bleiben.
pub fn apply_swing(notes: &[Note], swing: f64) -> Vec<Note> {
    notes
        .iter()
        .map(|n| match (row_of(n.pitch), step_of(n.start_beats)) {
            (Some(_), Some(s)) => Note { start_beats: s as f64 * STEP_BEATS + swing_offset(s, swing), ..n.clone() },
            _ => n.clone(),
        })
        .collect()
}

/// Swing eines Clips, geschätzt aus seinen Noten auf ungeraden Schritten.
pub fn swing_of(notes: &[Note]) -> f64 {
    let offs: Vec<f64> = notes
        .iter()
        .filter(|n| row_of(n.pitch).is_some())
        .filter_map(|n| step_of(n.start_beats).filter(|s| s % 2 == 1).map(|s| n.start_beats - s as f64 * STEP_BEATS))
        .filter(|d| *d > 0.0)
        .collect();
    if offs.is_empty() {
        return 0.0;
    }
    let avg = offs.iter().sum::<f64>() / offs.len() as f64;
    ((avg / (STEP_BEATS * 0.66)) * 100.0).round().clamp(0.0, 100.0)
}

// ---------------------------------------------------------------- im Projekt

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ClipGrid {
    pub grid: Grid,
    pub swing: f64,
    /// Noten, die nicht im Raster erscheinen (andere Tasten, hinter dem Raster)
    pub skipped: usize,
    pub name: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Written {
    pub snapshot: Snapshot,
    pub track_id: Id,
    pub clip_id: Id,
}

fn drum_clip(st: &AppState, track_id: Id, clip_id: Id) -> CmdResult<(Track, Clip, Vec<Note>, f64)> {
    let s = lock(&st.session)?;
    let t = s.project.track(track_id).ok_or_else(|| tr!("Track {id} not found", id = track_id))?;
    if t.instrument.as_ref().map(|i| i.kind) != Some(InstrumentKind::Drums) {
        return Err(tr!("\"{name}\" is not a drum track", name = t.name));
    }
    let c = t.clips.iter().find(|c| c.id == clip_id).ok_or_else(|| tr!("Clip {id} not found", id = clip_id))?;
    let ClipSource::Notes { length_beats, notes } = &c.source else { return Err(tr!("Not a note clip")) };
    Ok((t.clone(), c.clone(), notes.clone(), *length_beats))
}

impl AppState {
    /// Raster eines Schlagzeug-Clips (höchstens vier Takte).
    pub fn rhythm_read(&self, track_id: Id, clip_id: Id) -> CmdResult<ClipGrid> {
        let (_, clip, notes, len) = drum_clip(self, track_id, clip_id)?;
        let bars = ((len / (STEPS_PER_BAR as f64 * STEP_BEATS)).ceil() as usize).clamp(1, MAX_BARS);
        let (grid, skipped) = grid_from_notes(&notes, bars * STEPS_PER_BAR);
        Ok(ClipGrid { grid, swing: swing_of(&notes), skipped, name: clip.name })
    }

    /// Einen Schritt in einem Schlagzeug-Clip setzen oder löschen (ein Rückgängig-Schritt).
    pub fn rhythm_toggle(&self, track_id: Id, clip_id: Id, row_key: &str, step: usize, on: bool, swing: f64) -> CmdResult<Snapshot> {
        let (track, _, notes, len) = drum_clip(self, track_id, clip_id)?;
        let steps = ((len / STEP_BEATS).round() as usize).max(STEPS_PER_BAR);
        if step >= steps {
            return Err(tr!("Step {step} is beyond the end of the clip ({steps} steps)", step = step + 1, steps = steps));
        }
        let notes = toggle(&notes, row_key, step, on, swing, steps)?;
        let what = row(row_key).map(|r| klang_i18n::translate(r.label)).unwrap_or_else(|| row_key.to_string());
        let label = if on {
            tr!("Rhythm \"{track}\": {drum} on {step}", track = track.name, drum = what, step = step + 1)
        } else {
            tr!("Rhythm \"{track}\": {drum} removed at {step}", track = track.name, drum = what, step = step + 1)
        };
        self.execute(Command::Batch { label, commands: vec![Command::SetClipNotes { track_id, clip_id, notes, length_beats: None }] }, false)
    }

    /// Swing eines Schlagzeug-Clips ändern (`merge`: Teil einer Reglerbewegung).
    pub fn rhythm_swing(&self, track_id: Id, clip_id: Id, swing: f64, merge: bool) -> CmdResult<Snapshot> {
        let (track, _, notes, _) = drum_clip(self, track_id, clip_id)?;
        let label = tr!("Rhythm \"{track}\": swing {swing}%", track = track.name, swing = swing.round());
        self.execute(
            Command::Batch { label, commands: vec![Command::SetClipNotes { track_id, clip_id, notes: apply_swing(&notes, swing), length_beats: None }] },
            merge,
        )
    }

    /// Schreibt ein Raster: in einen bestimmten Clip (ersetzt seine Noten und Länge),
    /// sonst als neuen Clip ab `start_beats` in die Spur – ohne Spur in eine neue
    /// Schlagzeug-Spur mit dem Klang `sound` (Startpunkt im Charakter-Feld).
    #[allow(clippy::too_many_arguments)]
    pub fn rhythm_write(
        &self,
        grid: Grid,
        swing: f64,
        track_id: Option<Id>,
        clip_id: Option<Id>,
        start_beats: f64,
        name: &str,
        sound_preset: Option<&str>,
        tempo: Option<f64>,
        label_prefix: &str,
    ) -> CmdResult<Written> {
        let grid = grid.checked()?;
        let len = grid.steps as f64 * STEP_BEATS;
        let notes = grid_notes(&grid, swing);
        let name = if name.trim().is_empty() { tr!("Beat") } else { name.trim().to_string() };
        let mut commands = Vec::new();
        if let Some(bpm) = tempo {
            commands.push(Command::SetProject { patch: ProjectPatch { tempo_bpm: Some(bpm.clamp(20.0, 300.0)), ..Default::default() } });
        }
        let (tid, cid, label) = {
            let mut s = lock(&self.session)?;
            let p = &mut s.project;
            match (track_id, clip_id) {
                (Some(tid), Some(cid)) => {
                    let t = p.track(tid).ok_or_else(|| tr!("Track {id} not found", id = tid))?;
                    if t.instrument.as_ref().map(|i| i.kind) != Some(InstrumentKind::Drums) {
                        return Err(tr!("\"{name}\" is not a drum track", name = t.name));
                    }
                    if !t.clips.iter().any(|c| c.id == cid && matches!(c.source, ClipSource::Notes { .. })) {
                        return Err(tr!("Clip {id} not found", id = cid));
                    }
                    commands.push(Command::SetClipNotes { track_id: tid, clip_id: cid, notes, length_beats: Some(len) });
                    commands.push(Command::SetClip { track_id: tid, clip_id: cid, patch: ClipPatch { name: Some(name.clone()), ..Default::default() } });
                    (tid, cid, tr!("{prefix}Rhythm \"{name}\" in \"{track}\"", prefix = label_prefix, name = name, track = t.name))
                }
                (Some(tid), None) => {
                    let t = p.track(tid).ok_or_else(|| tr!("Track {id} not found", id = tid))?;
                    if t.instrument.as_ref().map(|i| i.kind) != Some(InstrumentKind::Drums) {
                        return Err(tr!("\"{name}\" is not a drum track", name = t.name));
                    }
                    let tname = t.name.clone();
                    let clip = Clip { id: p.alloc_id(), name: name.clone(), start_beats: start_beats.max(0.0), source: ClipSource::Notes { length_beats: len, notes } };
                    let cid = clip.id;
                    commands.push(Command::AddClip { track_id: tid, clip });
                    (tid, cid, tr!("{prefix}Rhythm \"{name}\" in \"{track}\"", prefix = label_prefix, name = name, track = tname))
                }
                (None, _) => {
                    let count = p.tracks.iter().filter(|t| t.instrument.as_ref().map(|i| i.kind) == Some(InstrumentKind::Drums)).count();
                    let tname = match count {
                        0 => sound::kind_name(InstrumentKind::Drums),
                        n => format!("{} {}", sound::kind_name(InstrumentKind::Drums), n + 1),
                    };
                    let tid = p.alloc_id();
                    let mut track = Track::new(tid, tname.clone(), TrackKind::Instrument);
                    let mut inst = sound::default_instrument(InstrumentKind::Drums);
                    if let Some(preset) = sound_preset {
                        if let Ok((x, y)) = sound::field_preset(InstrumentKind::Drums, "charakter", preset) {
                            sound::apply_field(&mut inst, "charakter", x, y)?;
                            inst.preset = preset.to_string();
                        }
                    }
                    track.instrument = Some(inst);
                    let cid = p.alloc_id();
                    track.clips.push(Clip { id: cid, name: name.clone(), start_beats: start_beats.max(0.0), source: ClipSource::Notes { length_beats: len, notes } });
                    commands.push(Command::AddTrack { track, index: None });
                    (tid, cid, tr!("{prefix}Rhythm \"{name}\" in new track \"{track}\"", prefix = label_prefix, name = name, track = tname))
                }
            }
        };
        let snapshot = self.execute(Command::Batch { label, commands }, false)?;
        Ok(Written { snapshot, track_id: tid, clip_id: cid })
    }

    /// Spielt ein Raster (oder einen Schlagzeug-Clip) im Projekttempo vor – mit dem
    /// Klang der Spur, sonst mit dem Standard-Schlagzeug bzw. dem Startpunkt `sound`.
    pub fn rhythm_preview(&self, track_id: Option<Id>, clip_id: Option<Id>, grid: Option<Grid>, swing: f64, sound_preset: Option<&str>) -> CmdResult<f32> {
        let engine = self.engine()?;
        let sr = engine.info().sample_rate as f32;
        let (inst, gain_db, notes, len) = match (track_id, clip_id, grid) {
            (Some(tid), Some(cid), None) => {
                let (t, _, notes, len) = drum_clip(self, tid, cid)?;
                (t.instrument.clone(), t.gain_db, notes, len)
            }
            (tid, _, Some(grid)) => {
                let grid = grid.checked()?;
                let track = tid.and_then(|id| lock(&self.session).ok()?.project.track(id).cloned());
                let inst = match &track {
                    Some(t) if t.instrument.as_ref().map(|i| i.kind) == Some(InstrumentKind::Drums) => t.instrument.clone(),
                    _ => {
                        let mut inst = sound::default_instrument(InstrumentKind::Drums);
                        if let Some((x, y)) = sound_preset.and_then(|p| sound::field_preset(InstrumentKind::Drums, "charakter", p).ok()) {
                            sound::apply_field(&mut inst, "charakter", x, y)?;
                        }
                        Some(inst)
                    }
                };
                (inst, track.map(|t| t.gain_db).unwrap_or(0.0), grid_notes(&grid, swing), grid.steps as f64 * STEP_BEATS)
            }
            _ => return Err(tr!("Specify a grid or a clip")),
        };
        // kurze Muster zweimal, damit man den Kreis hört
        let reps = if len <= 4.0 + 1e-9 { 2 } else { 1 };
        let spb = 60.0 / self.tempo();
        let specs: Vec<_> = (0..reps)
            .flat_map(|r| notes.iter().map(move |n| (r, n)))
            .map(|(r, n)| {
                let mut n = n.clone();
                n.start_beats += r as f64 * len;
                sound::note_spec(&n, spb as f32, 0.0)
            })
            .collect();
        let secs = (reps as f64 * len * spb) as f32;
        let r = sound::render_track(inst.as_ref(), &specs, secs, sr, spb as f32);
        engine.preview_stereo(r.mid.into(), r.side.map(Into::into), util::db_to_gain(gain_db), sound::reverb_send(inst.as_ref()));
        Ok(secs)
    }
}

/// Erster freier Taktanfang hinter dem letzten Clip einer Spur (ohne Spur: 0).
pub fn free_start(st: &AppState, track_id: Option<Id>) -> f64 {
    let Ok(s) = lock(&st.session) else { return 0.0 };
    let bpb = s.project.beats_per_bar.max(1) as f64;
    let end = track_id
        .and_then(|id| s.project.track(id))
        .map(|t| {
            t.clips
                .iter()
                .map(|c| match &c.source {
                    ClipSource::Notes { length_beats, .. } => c.start_beats + length_beats,
                    _ => c.start_beats,
                })
                .fold(0.0f64, f64::max)
        })
        .unwrap_or(0.0);
    (end / bpb - 1e-9).ceil().max(0.0) * bpb
}

/// Zeile als Text („x...x...“) – kompakt für den Agenten.
pub fn row_text(steps: &[usize], len: usize) -> String {
    (0..len).map(|s| if steps.contains(&s) { 'x' } else { '.' }).collect()
}

/// Zeilen als Text oder Schrittlisten (1–16 …) einlesen.
pub fn grid_from_json(rows: &serde_json::Value, steps: usize) -> Result<Grid, String> {
    let obj = rows.as_object().ok_or("rows: { kick: \"x...x...\", snare: [5, 13], … }")?;
    let mut out = BTreeMap::new();
    for (k, v) in obj {
        let list: Vec<usize> = match v {
            serde_json::Value::String(s) => s.chars().filter(|c| !c.is_whitespace() && *c != '|').enumerate().filter(|(_, c)| matches!(c, 'x' | 'X' | '1' | '*')).map(|(i, _)| i).collect(),
            serde_json::Value::Array(a) => a
                .iter()
                .map(|x| x.as_u64().filter(|&n| n >= 1).map(|n| n as usize - 1).ok_or_else(|| tr!("\"{key}\": steps start at 1", key = k)))
                .collect::<Result<_, _>>()?,
            _ => return Err(tr!("\"{key}\": text like \"x...x...\" or steps like [1, 9]", key = k)),
        };
        out.insert(k.clone(), list);
    }
    Grid { steps, rows: out }.checked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pattern_is_valid_and_named() {
        let i = info();
        assert_eq!(i.genres.len(), 14);
        let n: usize = i.genres.iter().map(|g| g.patterns.len()).sum();
        assert_eq!(n, 33);
        for g in &i.genres {
            assert!(sound::field_preset(InstrumentKind::Drums, "charakter", &g.sound).is_ok(), "Klang {} für {}", g.sound, g.name);
            for p in &g.patterns {
                assert!(!p.grid.rows.is_empty() && p.bpm >= 60, "{}", p.name);
            }
        }
        // Becken: einmal = Crash (Rock), durchgehend = Ride (Bossa Nova)
        let (_, rock) = find_pattern(Some("Rock"), "Rock").unwrap();
        assert_eq!(rock.grid.rows["crash"], vec![0]);
        let (_, bossa) = find_pattern(None, "bossa nova").unwrap();
        assert_eq!(bossa.grid.rows["ride"].len(), 16);
        let (g, p) = find_pattern(Some("house"), "").unwrap();
        assert_eq!((g.name.as_str(), p.name.as_str()), ("House", "Classic House"));
        assert!(find_pattern(Some("Polka"), "x").unwrap_err().contains("No genre"));
    }

    #[test]
    fn grid_to_notes_and_back() {
        let (_, p) = find_pattern(None, "Funky Drummer").unwrap();
        let grid = p.grid.resized(32);
        let notes = grid_notes(&grid, 0.0);
        let (back, skipped) = grid_from_notes(&notes, 32);
        assert_eq!(back, grid);
        assert_eq!(skipped, 0);
        // Backbeat laut, Zwischen-Snares leise
        let snare = |s: usize| notes.iter().find(|n| n.pitch == drums::SNARE && step_of(n.start_beats) == Some(s)).unwrap().velocity;
        assert!(snare(4) > 100 && snare(7) < 80);
        // Dembow: keine Snare auf einer Viertel – dann sind alle laut
        let (_, dembow) = find_pattern(None, "Reggaeton (Dembow)").unwrap();
        assert!(grid_notes(&dembow.grid, 0.0).iter().filter(|n| n.pitch == drums::SNARE).all(|n| n.velocity > 100));
    }

    #[test]
    fn swing_moves_every_second_sixteenth_and_survives_reading() {
        let (_, p) = find_pattern(None, "Hip Hop Groove").unwrap();
        let notes = grid_notes(&p.grid, 40.0);
        let off: Vec<&Note> = notes.iter().filter(|n| step_of(n.start_beats).map(|s| s % 2 == 1).unwrap_or(false)).collect();
        let on: Vec<&Note> = notes.iter().filter(|n| step_of(n.start_beats).map(|s| s % 2 == 0).unwrap_or(false)).collect();
        assert!(on.iter().all(|n| (n.start_beats / STEP_BEATS).fract().abs() < 1e-9));
        assert!(!off.is_empty() && off.iter().all(|n| (n.start_beats / STEP_BEATS).fract() > 0.2));
        // gelesen wie geschrieben, Swing erkannt, neu geswingt ohne Verlust
        assert_eq!(grid_from_notes(&notes, 16).0, p.grid);
        assert_eq!(swing_of(&notes), 40.0);
        let straight = apply_swing(&notes, 0.0);
        assert_eq!(straight, grid_notes(&p.grid, 0.0));
        // auch maximaler Swing bleibt näher am eigenen Schritt
        assert_eq!(grid_from_notes(&grid_notes(&p.grid, 100.0), 16).0, p.grid);
    }

    #[test]
    fn toggling_touches_only_that_step() {
        let mut notes = grid_notes(&find_pattern(None, "Boots ’n’ Cats").unwrap().1.grid, 0.0);
        // fremde Taste und Ausdruck bleiben erhalten
        notes.push(Note { pitch: 60, start_beats: 1.0, length_beats: 0.5, velocity: 50, ..Default::default() });
        notes[0].brightness = 0.5;
        let on = toggle(&notes, "rim", 3, true, 0.0, 16).unwrap();
        assert_eq!(on.len(), notes.len() + 1);
        let off = toggle(&on, "rim", 3, false, 0.0, 16).unwrap();
        assert_eq!(off.len(), notes.len());
        assert!(off.iter().any(|n| n.pitch == 60) && off.iter().any(|n| n.brightness == 0.5));
        let (g, skipped) = grid_from_notes(&off, 16);
        assert_eq!(skipped, 1, "Taste 60 ist keine Trommel");
        assert!(!g.rows.contains_key("rim"));
        assert!(toggle(&notes, "gong", 0, true, 0.0, 16).is_err());
    }

    #[test]
    fn rows_from_text_or_lists() {
        let g = grid_from_json(&serde_json::json!({ "kick": "x...x...x...x...", "snare": [5, 13] }), 16).unwrap();
        assert_eq!(g.rows["kick"], vec![0, 4, 8, 12]);
        assert_eq!(g.rows["snare"], vec![4, 12]);
        assert_eq!(row_text(&g.rows["kick"], 16), "x...x...x...x...");
        assert!(grid_from_json(&serde_json::json!({ "gong": "x" }), 16).is_err());
        assert!(Grid { steps: 20, rows: BTreeMap::new() }.checked().is_err());
    }
}
