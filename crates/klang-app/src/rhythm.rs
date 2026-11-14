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
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Sechzehntel je Takt (4/4).
pub const STEPS_PER_BAR: usize = 16;
const STEP_BEATS: f64 = 0.25;
pub const MAX_BARS: usize = 4;

/// Zeilen des Rasters: Kennung, Name, Taste; `main` = immer sichtbar.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    pub key: &'static str,
    pub label: &'static str,
    pub midi: u8,
    pub main: bool,
}

pub const ROWS: [Row; 13] = [
    Row { key: "kick", label: "Kick", midi: drums::KICK, main: true },
    Row { key: "snare", label: "Snare", midi: drums::SNARE, main: true },
    Row { key: "clap", label: "Clap", midi: drums::CLAP, main: true },
    Row { key: "rim", label: "Rim", midi: drums::RIM, main: true },
    Row { key: "hhc", label: "Hi-Hat zu", midi: drums::HAT_CLOSED, main: true },
    Row { key: "hho", label: "Hi-Hat offen", midi: drums::HAT_OPEN, main: true },
    Row { key: "crash", label: "Crash", midi: drums::CRASH, main: false },
    Row { key: "ride", label: "Ride", midi: drums::RIDE, main: false },
    Row { key: "cow", label: "Cowbell", midi: drums::COWBELL, main: false },
    Row { key: "tomh", label: "Tom hoch", midi: drums::TOM_HIGH, main: false },
    Row { key: "tomm", label: "Tom mittel", midi: drums::TOM_MID, main: false },
    Row { key: "toml", label: "Tom tief", midi: drums::TOM_LOW, main: false },
    Row { key: "shk", label: "Shaker", midi: drums::SHAKER, main: false },
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
    name: &'static str,
    bpm: u16,
    /// empfohlener Swing in Prozent
    swing: u8,
    tip: &'static str,
    /// Kürzel wie im Heft (BD, SN, CL, RS, CH, OH, CY, CB, HT, MT, LT, SH), Schritte 1–16
    rows: &'static [(&'static str, &'static [u8])],
}

struct GenreDef {
    name: &'static str,
    blurb: &'static str,
    /// Startpunkt im Charakter-Feld für eine neue Schlagzeug-Spur
    sound: &'static str,
    patterns: &'static [PatternDef],
}

const E8: &[u8] = &[1, 3, 5, 7, 9, 11, 13, 15];
const ALL16: &[u8] = &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];

const GENRES: &[GenreDef] = &[
    GenreDef {
        name: "Grundlagen",
        blurb: "Hier fängt alles an: Die Kick gibt den Puls, die Snare sitzt auf Zählzeit 2 und 4 (der Backbeat), die Hi-Hat läuft in Achteln durch.",
        sound: "Rock",
        patterns: &[
            PatternDef { name: "Boots ’n’ Cats", bpm: 118, swing: 0, tip: "Der Klassiker zum Üben – mitsprechen: „boots and cats and boots and cats“. Kick, Hi-Hat, Snare, Hi-Hat.", rows: &[("BD", &[1, 9]), ("SN", &[5, 13]), ("CH", E8)] },
            PatternDef { name: "Four on the Floor", bpm: 122, swing: 0, tip: "Kick auf jeder Viertel – das Fundament fast aller Tanzmusik. Offene Hi-Hat auf dem „und“.", rows: &[("BD", &[1, 5, 9, 13]), ("OH", &[3, 7, 11, 15])] },
            PatternDef { name: "Pop Backbeat", bpm: 110, swing: 0, tip: "Snare auf 2 und 4 – das Herz jedes Pop- und Rocksongs.", rows: &[("BD", &[1, 4, 7, 11]), ("SN", &[5, 13]), ("CH", E8)] },
        ],
    },
    GenreDef {
        name: "House",
        blurb: "Kick auf jeder Viertel, Clap auf 2 und 4 und die typischen offenen Hi-Hats auf dem Offbeat. 120–126 BPM.",
        sound: "House",
        patterns: &[
            PatternDef { name: "Classic House", bpm: 124, swing: 0, tip: "Das „tss“ der offenen Hi-Hat zwischen den Kicks macht den House-Groove.", rows: &[("BD", &[1, 5, 9, 13]), ("CL", &[5, 13]), ("OH", &[3, 7, 11, 15])] },
            PatternDef { name: "Deep House", bpm: 122, swing: 0, tip: "Weicher und rollender: zusätzliche geschlossene Hi-Hats bringen Bewegung.", rows: &[("BD", &[1, 5, 9, 13]), ("CL", &[5, 13]), ("CH", &[2, 8, 10]), ("OH", &[3, 7, 11, 15])] },
            PatternDef { name: "French House", bpm: 123, swing: 0, tip: "Durchgehende Hi-Hats plus Offbeat-Hats, der Shaker treibt zusätzlich.", rows: &[("BD", &[1, 5, 9, 13]), ("CL", &[5, 13]), ("CH", E8), ("OH", &[3, 7, 11, 15]), ("SH", &[1, 5, 9, 13])] },
        ],
    },
    GenreDef {
        name: "Techno",
        blurb: "Treibend und hypnotisch: Kick auf jeder Viertel, dazu ein Extra-Kick kurz vor Schluss. 130–138 BPM.",
        sound: "Techno",
        patterns: &[
            PatternDef { name: "Techno", bpm: 134, swing: 0, tip: "Der Kick auf Schritt 15 schiebt in den nächsten Takt.", rows: &[("BD", &[1, 5, 9, 13, 15]), ("OH", &[3, 7, 11, 15]), ("CH", &[10])] },
            PatternDef { name: "Peak-Time Techno", bpm: 136, swing: 0, tip: "Stampfend: Kick durch, Clap auf 2 und 4, Hi-Hats auf allen Zwischenschritten.", rows: &[("BD", &[1, 5, 9, 13]), ("CL", &[5, 13]), ("OH", &[3, 7, 11, 15]), ("CH", &[2, 4, 6, 8, 10, 12, 14, 16])] },
        ],
    },
    GenreDef {
        name: "Electro",
        blurb: "Synkopierte Kicks statt Four on the Floor, aber eine gerade Snare auf 2 und 4. Rund 128 BPM.",
        sound: "808",
        patterns: &[
            PatternDef { name: "Electro", bpm: 128, swing: 0, tip: "Die Kick springt (1, 7, 11, 14) – das gibt den nervösen Electro-Bounce.", rows: &[("BD", &[1, 7, 11, 14]), ("SN", &[5, 13]), ("CH", E8)] },
            PatternDef { name: "Miami Bass", bpm: 126, swing: 0, tip: "Kick und Snare wie beim Electro, Hi-Hats mit Sechzehntel-Betonungen.", rows: &[("BD", &[1, 7, 11, 14]), ("SN", &[5, 13]), ("CH", &[1, 3, 4, 5, 7, 8, 9, 11, 12, 13, 15, 16])] },
        ],
    },
    GenreDef {
        name: "Hip-Hop / Boom Bap",
        blurb: "Entspanntes Tempo (85–95 BPM), satte Kick-Snare-Grooves, oft leicht geswingt.",
        sound: "Lo-Fi",
        patterns: &[
            PatternDef { name: "Boom Bap", bpm: 88, swing: 20, tip: "Harte Kick auf 1, Snare auf 2 und 4 – mit etwas Swing sofort zum Mitnicken.", rows: &[("BD", &[1, 7, 11]), ("SN", &[5, 13]), ("CH", E8)] },
            PatternDef { name: "Hip Hop Groove", bpm: 90, swing: 20, tip: "Mehr Kicks (1, 3, 7, 8, 15) machen den Groove gesprächiger.", rows: &[("BD", &[1, 3, 7, 8, 15]), ("SN", &[5, 13]), ("CH", E8)] },
            PatternDef { name: "Lo-Fi", bpm: 82, swing: 35, tip: "Langsam und verschleppt – Swing hochdrehen.", rows: &[("BD", &[1, 8, 11]), ("SN", &[5, 13]), ("CH", &[1, 4, 5, 8, 9, 12, 13, 16]), ("OH", &[7])] },
        ],
    },
    GenreDef {
        name: "Trap",
        blurb: "Halftime: Snare bzw. Clap erst auf Zählzeit 3, schnelle Hi-Hats. Rund 140 BPM, fühlt sich halb so schnell an.",
        sound: "Trap",
        patterns: &[
            PatternDef { name: "Trap", bpm: 140, swing: 0, tip: "Clap erst in der Mitte = Halftime; Kick sparsam, Hi-Hats mit kleinen Rolls.", rows: &[("BD", &[1, 7, 13]), ("CL", &[9]), ("CH", &[1, 3, 5, 7, 9, 10, 11, 13, 15, 16])] },
            PatternDef { name: "Drill", bpm: 142, swing: 0, tip: "Rutschende Kicks (3, 5, 11) und der verschobene Clap.", rows: &[("BD", &[3, 5, 11]), ("CL", &[9]), ("CH", E8)] },
        ],
    },
    GenreDef {
        name: "Drum & Bass / Jungle",
        blurb: "Sehr schnell (160–176 BPM), durch die Snare auf 2 und 4 aber halb so schnell gefühlt. Der Amen-Break ist das Herzstück.",
        sound: "Rock",
        patterns: &[
            PatternDef { name: "Amen Break", bpm: 174, swing: 0, tip: "Die vielen Snares (5, 8, 10, 13, 16) machen das rollende Durcheinander.", rows: &[("BD", &[1, 3, 11, 12]), ("SN", &[5, 8, 10, 13, 16]), ("CH", E8)] },
            PatternDef { name: "Jungle", bpm: 162, swing: 0, tip: "Kantig und roh: verschobene Kicks, leise Zwischen-Snares, offene Hi-Hat auf 1.", rows: &[("BD", &[1, 3, 11]), ("SN", &[5, 8, 10, 15]), ("CH", E8), ("OH", &[1])] },
            PatternDef { name: "Liquid D&B", bpm: 174, swing: 0, tip: "Klarer Two-Step aus Kick und Snare – gut zum Hineinhören.", rows: &[("BD", &[1, 4, 8, 10, 11, 16]), ("SN", &[5, 13]), ("CH", E8)] },
        ],
    },
    GenreDef {
        name: "Funk / Breakbeat",
        blurb: "Der Groove lebt von leisen Zwischen-Snares (Ghost Notes) und synkopierten Kicks. 95–130 BPM, etwas Swing hilft.",
        sound: "Disco",
        patterns: &[
            PatternDef { name: "Funky Drummer", bpm: 100, swing: 15, tip: "Die vielen leisen Snares zwischen den Backbeats machen den Zauber.", rows: &[("BD", &[1, 3, 7, 11, 14]), ("SN", &[5, 8, 10, 12, 13, 16]), ("CH", &[1, 2, 3, 4, 5, 6, 7, 9, 10, 11, 12, 13, 15, 16]), ("OH", &[8, 14])] },
            PatternDef { name: "Impeach the President", bpm: 95, swing: 15, tip: "Schlicht und trocken, offene Hi-Hat auf 11 als Erkennungszeichen.", rows: &[("BD", &[1, 8, 9, 15]), ("SN", &[5, 13]), ("CH", &[1, 3, 5, 7, 8, 9, 13, 15]), ("OH", &[11])] },
            PatternDef { name: "Big Beat", bpm: 128, swing: 0, tip: "Fett und breit: Kick vorn, Hi-Hat nur auf 2 und 4.", rows: &[("BD", &[1, 4, 7, 9]), ("SN", &[5, 13]), ("CH", &[5, 13])] },
        ],
    },
    GenreDef {
        name: "Rock / Pop",
        blurb: "Gerader Backbeat, durchlaufende Achtel-Hi-Hats, Crash auf die 1. 110–130 BPM.",
        sound: "Rock",
        patterns: &[
            PatternDef { name: "Rock", bpm: 120, swing: 0, tip: "Der Standard: Kick auf 1, Snare auf 2 und 4, Crash zu Beginn.", rows: &[("BD", &[1, 8, 9, 11]), ("SN", &[5, 13]), ("CH", E8), ("CY", &[1])] },
            PatternDef { name: "Pop", bpm: 110, swing: 0, tip: "Etwas mehr Bewegung in der Kick – die Basis unzähliger Hits.", rows: &[("BD", &[1, 2, 4, 8, 11]), ("SN", &[5, 13]), ("CH", E8)] },
            PatternDef { name: "Disco", bpm: 120, swing: 0, tip: "Four on the Floor mit offenen Hi-Hats auf dem Offbeat – der Vorläufer von House.", rows: &[("BD", &[1, 5, 9, 13]), ("SN", &[5, 13]), ("OH", &[3, 7, 11, 15]), ("CH", ALL16)] },
        ],
    },
    GenreDef {
        name: "Reggaeton / Dancehall",
        blurb: "Der Dembow: Kick auf den Vierteln, Snare synkopiert auf 4, 7, 12, 15. 90–100 BPM.",
        sound: "808",
        patterns: &[
            PatternDef { name: "Reggaeton (Dembow)", bpm: 95, swing: 0, tip: "Das Snare-Muster 4·7 / 12·15 ist der Dembow-Rhythmus.", rows: &[("BD", &[1, 5, 9, 13]), ("SN", &[4, 7, 12, 15]), ("CH", E8)] },
            PatternDef { name: "Dancehall", bpm: 98, swing: 0, tip: "Verwandt, aber sparsamer in der Kick; der Rim gibt den holzigen Akzent.", rows: &[("BD", &[1, 9]), ("SN", &[4, 7, 12, 15]), ("RS", &[3, 11]), ("CH", E8)] },
        ],
    },
    GenreDef {
        name: "Reggae / Dub",
        blurb: "Entspannt, 70–80 BPM. Der One Drop lässt die 1 weg: Kick und Snare fallen zusammen auf Zählzeit 3.",
        sound: "Lo-Fi",
        patterns: &[
            PatternDef { name: "One Drop", bpm: 75, swing: 0, tip: "Keine Kick auf 1 – Kick und Snare gemeinsam auf der 3.", rows: &[("BD", &[9]), ("SN", &[9]), ("CH", E8)] },
            PatternDef { name: "Steppers", bpm: 78, swing: 0, tip: "Treibender: Kick auf jeder Viertel, Snare weiter auf der 3.", rows: &[("BD", &[1, 5, 9, 13]), ("SN", &[9]), ("CH", E8)] },
        ],
    },
    GenreDef {
        name: "Latin / Afro-Cuban",
        blurb: "Alles dreht sich um die Clave – ein 3-2-Muster auf dem Rim, darüber Ride und eine tanzende Kick. 100–130 BPM.",
        sound: "Disco",
        patterns: &[
            PatternDef { name: "Bossa Nova", bpm: 130, swing: 0, tip: "Die Rim-Clave (1, 4, 7, 11, 14) ist das Rückgrat, die Ride läuft durch.", rows: &[("RS", &[1, 4, 7, 11, 14]), ("BD", &[1, 4, 5, 8, 9, 12, 13, 16]), ("CY", ALL16)] },
            PatternDef { name: "Son Clave (3-2)", bpm: 100, swing: 0, tip: "Die klassische 3-2-Clave auf dem Rim: X··X··X···X·X···", rows: &[("RS", &[1, 4, 7, 11, 13]), ("BD", &[1, 9]), ("CH", E8)] },
            PatternDef { name: "Songo", bpm: 110, swing: 0, tip: "Kubanischer Groove: die Cowbell führt, Kick synkopiert.", rows: &[("BD", &[1, 5, 9, 13, 15]), ("RS", &[3, 4, 7, 8, 11, 12, 15, 16]), ("CB", &[1, 4, 7, 11, 15])] },
        ],
    },
    GenreDef {
        name: "UK Garage",
        blurb: "Synkopiert und mit viel Swing: verschobene Kicks, Clap auf 2 und 4, kantige Hi-Hats. 130–138 BPM.",
        sound: "House",
        patterns: &[
            PatternDef { name: "2-Step Garage", bpm: 135, swing: 40, tip: "Kick nur auf 1 und 11 – der „Skip“. Swing um 40 % für das echte Garage-Gefühl.", rows: &[("BD", &[1, 11]), ("CL", &[5, 13]), ("CH", &[3, 4, 7, 11, 15, 16]), ("RS", &[2, 8, 14]), ("MT", &[6, 12])] },
        ],
    },
    GenreDef {
        name: "Dubstep",
        blurb: "Halftime wie Trap, aber härter: Snare auf Zählzeit 3, sparsame Kicks, viel Platz für den Bass. Rund 140 BPM.",
        sound: "Trap",
        patterns: &[
            PatternDef { name: "Dubstep", bpm: 140, swing: 0, tip: "Kick auf 1, Snare auf der 3 – dazwischen ist Platz für den Bass.", rows: &[("BD", &[1, 11]), ("SN", &[9]), ("CH", &[2, 3, 7, 12, 15]), ("OH", &[5, 14])] },
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
            return Err(format!("Länge {} Schritte – erlaubt sind 16, 32, 48 oder 64", self.steps));
        }
        for (k, v) in self.rows.iter_mut() {
            if row(k).is_none() {
                return Err(format!("Unbekannte Trommel „{k}“ (erlaubt: {})", ROWS.iter().map(|r| r.key).collect::<Vec<_>>().join(", ")));
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
    PatternView { name: p.name.into(), bpm: p.bpm, swing: p.swing, tip: p.tip.into(), grid }
}

pub fn info() -> RhythmInfo {
    RhythmInfo {
        rows: ROWS.to_vec(),
        genres: GENRES
            .iter()
            .map(|g| GenreView { name: g.name.into(), blurb: g.blurb.into(), sound: g.sound.into(), patterns: g.patterns.iter().map(pattern_view).collect() })
            .collect(),
    }
}

/// Muster nach Genre und Name (ohne Groß-/Kleinschreibung; das Genre darf fehlen).
pub fn find_pattern(genre: Option<&str>, name: &str) -> Result<(GenreView, PatternView), String> {
    let all = info().genres;
    let eq = |a: &str, b: &str| a.to_lowercase() == b.trim().to_lowercase();
    let genres: Vec<&GenreView> = match genre {
        Some(g) => {
            let found: Vec<&GenreView> = all.iter().filter(|x| eq(&x.name, g) || x.name.to_lowercase().contains(&g.trim().to_lowercase())).collect();
            if found.is_empty() {
                return Err(format!("Kein Genre „{g}“ (vorhanden: {})", all.iter().map(|x| x.name.as_str()).collect::<Vec<_>>().join(", ")));
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
    Err(format!("Kein Muster „{name}“. Vorhanden: {}", names.join(", ")))
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
    let r = row(row_key).ok_or_else(|| format!("Unbekannte Trommel „{row_key}“"))?;
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
    let t = s.project.track(track_id).ok_or_else(|| format!("Spur {track_id} nicht gefunden"))?;
    if t.instrument.as_ref().map(|i| i.kind) != Some(InstrumentKind::Drums) {
        return Err(format!("„{}“ ist keine Schlagzeug-Spur", t.name));
    }
    let c = t.clips.iter().find(|c| c.id == clip_id).ok_or_else(|| format!("Clip {clip_id} nicht gefunden"))?;
    let ClipSource::Notes { length_beats, notes } = &c.source else { return Err("Kein Noten-Clip".into()) };
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
            return Err(format!("Schritt {} liegt hinter dem Clip ({steps} Schritte)", step + 1));
        }
        let notes = toggle(&notes, row_key, step, on, swing, steps)?;
        let what = row(row_key).map(|r| r.label).unwrap_or(row_key);
        let label = format!("Rhythmus „{}“: {what} {} {}", track.name, if on { "auf" } else { "weg bei" }, step + 1);
        self.execute(Command::Batch { label, commands: vec![Command::SetClipNotes { track_id, clip_id, notes, length_beats: None }] }, false)
    }

    /// Swing eines Schlagzeug-Clips ändern (`merge`: Teil einer Reglerbewegung).
    pub fn rhythm_swing(&self, track_id: Id, clip_id: Id, swing: f64, merge: bool) -> CmdResult<Snapshot> {
        let (track, _, notes, _) = drum_clip(self, track_id, clip_id)?;
        let label = format!("Rhythmus „{}“: Swing {} %", track.name, swing.round());
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
        let name = if name.trim().is_empty() { "Beat".to_string() } else { name.trim().to_string() };
        let mut commands = Vec::new();
        if let Some(bpm) = tempo {
            commands.push(Command::SetProject { patch: ProjectPatch { tempo_bpm: Some(bpm.clamp(20.0, 300.0)), ..Default::default() } });
        }
        let (tid, cid, label) = {
            let mut s = lock(&self.session)?;
            let p = &mut s.project;
            match (track_id, clip_id) {
                (Some(tid), Some(cid)) => {
                    let t = p.track(tid).ok_or_else(|| format!("Spur {tid} nicht gefunden"))?;
                    if t.instrument.as_ref().map(|i| i.kind) != Some(InstrumentKind::Drums) {
                        return Err(format!("„{}“ ist keine Schlagzeug-Spur", t.name));
                    }
                    if !t.clips.iter().any(|c| c.id == cid && matches!(c.source, ClipSource::Notes { .. })) {
                        return Err(format!("Clip {cid} nicht gefunden"));
                    }
                    commands.push(Command::SetClipNotes { track_id: tid, clip_id: cid, notes, length_beats: Some(len) });
                    commands.push(Command::SetClip { track_id: tid, clip_id: cid, patch: ClipPatch { name: Some(name.clone()), ..Default::default() } });
                    (tid, cid, format!("{label_prefix}Rhythmus „{name}“ in „{}“", t.name))
                }
                (Some(tid), None) => {
                    let t = p.track(tid).ok_or_else(|| format!("Spur {tid} nicht gefunden"))?;
                    if t.instrument.as_ref().map(|i| i.kind) != Some(InstrumentKind::Drums) {
                        return Err(format!("„{}“ ist keine Schlagzeug-Spur", t.name));
                    }
                    let tname = t.name.clone();
                    let clip = Clip { id: p.alloc_id(), name: name.clone(), start_beats: start_beats.max(0.0), source: ClipSource::Notes { length_beats: len, notes } };
                    let cid = clip.id;
                    commands.push(Command::AddClip { track_id: tid, clip });
                    (tid, cid, format!("{label_prefix}Rhythmus „{name}“ in „{tname}“"))
                }
                (None, _) => {
                    let count = p.tracks.iter().filter(|t| t.instrument.as_ref().map(|i| i.kind) == Some(InstrumentKind::Drums)).count();
                    let tname = match count {
                        0 => sound::kind_name(InstrumentKind::Drums).to_string(),
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
                    (tid, cid, format!("{label_prefix}Rhythmus „{name}“ in neuer Spur „{tname}“"))
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
            _ => return Err("Raster oder Clip angeben".into()),
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
                .map(|x| x.as_u64().filter(|&n| n >= 1).map(|n| n as usize - 1).ok_or_else(|| format!("„{k}“: Schritte ab 1")))
                .collect::<Result<_, _>>()?,
            _ => return Err(format!("„{k}“: Text wie \"x...x...\" oder Schritte wie [1, 9]")),
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
        assert!(find_pattern(Some("Polka"), "x").unwrap_err().contains("Kein Genre"));
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
