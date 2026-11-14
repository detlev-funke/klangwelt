//! Soundwerkstatt: Instrumente (Zupfer, Orgel, Cembalo, Bass, Synth, Schlagzeug) mit ihren
//! Reglern, Feldern (Charakter, Klangfarbe, Raum), Verläufen, Klangerzeugung,
//! Vorhör-Mustern und Messwerten.
//!
//! Die Beschreibung der Instrumente ([`specs`]) ist die gemeinsame Quelle für
//! Oberfläche (Regler, Presets) und Agent (MCP-Werkzeuge).

use klang_core::{Instrument, InstrumentKind, Note};
use klang_dsp::bass::{render_bass, BassParams};
use klang_dsp::drums::{self, ClapParams, DrumKit, HatParams, KickParams, SnareParams, TomParams};
use klang_dsp::features::analyze_sound;
use klang_dsp::fx;
use klang_dsp::keys::{render_harpsichord, render_organ, HarpsichordParams, OrganParams};
use klang_dsp::pad::{render_synth, SynthParams, Verlauf, VERLAUF_PRESETS};
use klang_dsp::pitch::{detect_pitch, hit_pitches, YinConfig};
use klang_dsp::synth::{render_notes, NoteSpec, PluckParams};
use klang_dsp::util::median;
use serde::Serialize;
use std::collections::BTreeMap;

// ---------------------------------------------------------------- Beschreibung

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParamSpec {
    pub key: &'static str,
    pub label: &'static str,
    /// Gruppe im Klang-Tab (z. B. „Bassdrum“).
    pub group: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub left: &'static str,
    pub right: &'static str,
    /// Wert ist ein MIDI-Ton (Anzeige als Notenname).
    pub note: bool,
    pub meaning: &'static str,
    /// Kein eigener Regler (Feld-Stellung, Verlauf).
    pub hidden: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct PresetSpec {
    pub name: &'static str,
    pub params: BTreeMap<&'static str, f32>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PreviewSpec {
    pub key: &'static str,
    pub label: &'static str,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeySpec {
    pub midi: u8,
    pub short: &'static str,
    pub name: &'static str,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstrumentSpec {
    pub kind: InstrumentKind,
    pub name: &'static str,
    pub description: &'static str,
    pub params: Vec<ParamSpec>,
    pub presets: Vec<PresetSpec>,
    pub previews: Vec<PreviewSpec>,
    /// Tastenbelegung (nur Schlagzeug).
    pub keys: Vec<KeySpec>,
    /// Felder zum Gestalten: Charakter, Klangfarbe, Raum.
    pub fields: Vec<FieldSpec>,
    /// Vorlagen für den zeitlichen Verlauf (nur Synth).
    pub verlauf: Vec<VerlaufSpec>,
}

#[allow(clippy::too_many_arguments)]
fn p(key: &'static str, label: &'static str, group: &'static str, default: f32, left: &'static str, right: &'static str, meaning: &'static str) -> ParamSpec {
    ParamSpec { key, label, group, min: 0.0, max: 1.0, default, left, right, note: false, meaning, hidden: false }
}

#[allow(clippy::too_many_arguments)]
fn pn(key: &'static str, label: &'static str, group: &'static str, min: f32, max: f32, default: f32, meaning: &'static str) -> ParamSpec {
    ParamSpec { key, label, group, min, max, default, left: "tief", right: "hoch", note: true, meaning, hidden: false }
}

/// Standardwert eines Parameters laut Beschreibung des Instruments.
fn spec_default(kind: InstrumentKind, key: &str) -> f32 {
    spec(kind).params.iter().find(|p| p.key == key).map(|p| p.default).unwrap_or(0.0)
}

fn pv(key: &'static str, label: &'static str) -> PreviewSpec {
    PreviewSpec { key, label }
}

// ---------------------------------------------------------------- Felder
//
// Drei Felder zum Gestalten, jedes mit zwei Achsen und eigenen Startpunkten:
// * **Charakter** (je Instrument, z. B. Gewicht × Komplexität) – setzt die
//   Regler des Instruments nach einer festen Zuordnung ([`charakter_params`]).
// * **Klangfarbe** (dunkel ↔ hell × glatt ↔ rau) = Regler `farbe`, `rauheit`.
// * **Raum** (schmal ↔ breit × nah ↔ fern) = Regler `breite`, `raum`.
// Jedes Feld setzt nur seine eigenen Regler – ein Startpunkt in einem Feld
// ändert nichts an den anderen.

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AxisSpec {
    /// Regler, den die Achse setzt (beim Charakter `charakter.x`/`charakter.y`).
    pub param: &'static str,
    pub label: &'static str,
    /// Vier Wörter von links nach rechts bzw. unten nach oben.
    pub words: [&'static str; 4],
}

#[derive(Clone, Debug, Serialize)]
pub struct FieldPreset {
    pub name: &'static str,
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldSpec {
    /// „charakter“, „klangfarbe“ oder „raum“
    pub key: &'static str,
    pub name: &'static str,
    pub x: AxisSpec,
    pub y: AxisSpec,
    pub presets: Vec<FieldPreset>,
    /// Regler, die das Feld setzt (zur Erklärung für Mensch und Agent).
    pub sets: Vec<&'static str>,
}

/// Vorlage für den zeitlichen Verlauf (Synth).
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerlaufSpec {
    pub name: &'static str,
    pub beschreibung: &'static str,
    pub laut: Vec<[f32; 2]>,
    pub hell: Vec<[f32; 2]>,
    pub ausklang: f32,
    pub puls: u8,
    pub pulstiefe: f32,
}

const CHARAKTER_X: &str = "charakter.x";
const CHARAKTER_Y: &str = "charakter.y";

fn fp(name: &'static str, x: f32, y: f32) -> FieldPreset {
    FieldPreset { name, x, y }
}

fn axis(param: &'static str, label: &'static str, words: [&'static str; 4]) -> AxisSpec {
    AxisSpec { param, label, words }
}

/// Achsen und Startpunkte des Charakter-Felds je Instrument.
fn charakter_field(kind: InstrumentKind) -> (AxisSpec, AxisSpec, Vec<FieldPreset>) {
    let (x, y) = (CHARAKTER_X, CHARAKTER_Y);
    match kind {
        InstrumentKind::Drums => (
            axis(x, "Gewicht", ["knackig", "saftig", "voll", "mächtig"]),
            axis(y, "Komplexität", ["schlicht", "griffig", "vielschichtig", "komplex"]),
            vec![fp("Techno", 0.22, 0.42), fp("House", 0.42, 0.28), fp("Lo-Fi", 0.33, 0.08), fp("Rock", 0.58, 0.62), fp("Disco", 0.48, 0.45), fp("808", 0.84, 0.18), fp("Trap", 0.95, 0.55), fp("Hardstyle", 1.0, 0.92)],
        ),
        InstrumentKind::Synth => (
            axis(x, "Fülle", ["zart", "weich", "breit", "mächtig"]),
            axis(y, "Komplexität", ["rein", "lebendig", "schimmernd", "schillernd"]),
            vec![fp("Glas", 0.15, 0.75), fp("Wärme", 0.5, 0.2), fp("Puls", 0.3, 0.62), fp("Streicher", 0.6, 0.45), fp("Chor", 0.62, 0.68), fp("Ozean", 0.82, 0.58), fp("Kathedrale", 0.92, 0.4), fp("Nebel", 0.78, 0.92)],
        ),
        InstrumentKind::Pluck => (
            axis(x, "Fülle", ["kurz", "zart", "voll", "tragend"]),
            axis(y, "Glanz", ["weich", "rund", "hell", "brillant"]),
            vec![fp("Gedämpft", 0.12, 0.15), fp("Warm", 0.72, 0.32), fp("Harfe", 0.85, 0.55), fp("Hell", 0.35, 0.82), fp("Kristall", 0.6, 0.97)],
        ),
        InstrumentKind::Organ => (
            axis(x, "Gewicht", ["zart", "still", "kräftig", "Plenum"]),
            axis(y, "Obertöne", ["grundtönig", "rund", "obertönig", "glänzend"]),
            vec![fp("Flöten", 0.25, 0.3), fp("Prinzipal", 0.5, 0.12), fp("Cornet", 0.6, 0.65), fp("Pedal", 0.92, 0.1), fp("Plenum", 0.95, 0.9)],
        ),
        InstrumentKind::Harpsichord => (
            axis(x, "Fülle", ["gedämpft", "schlank", "voll", "prächtig"]),
            axis(y, "Brillanz", ["weich", "rund", "spitz", "brillant"]),
            vec![fp("Laute", 0.08, 0.25), fp("8′", 0.43, 0.5), fp("8′+4′", 0.8, 0.6), fp("Brillant", 0.75, 0.95)],
        ),
        InstrumentKind::Bass => (
            axis(x, "Gewicht", ["knackig", "straff", "satt", "mächtig"]),
            axis(y, "Biss", ["sauber", "rund", "bissig", "aggressiv"]),
            vec![fp("Finger", 0.4, 0.3), fp("Pluck-Bass", 0.22, 0.6), fp("Acid", 0.3, 0.92), fp("Growl", 0.82, 0.8), fp("Sub-Bass", 0.95, 0.06)],
        ),
    }
}

/// Grundstellung des Charakter-Punkts je Instrument (daraus die Standardwerte).
fn charakter_default(kind: InstrumentKind) -> (f32, f32) {
    match kind {
        InstrumentKind::Drums => (0.45, 0.4),
        InstrumentKind::Synth => (0.5, 0.4),
        InstrumentKind::Pluck => (0.45, 0.5),
        InstrumentKind::Organ => (0.55, 0.3),
        InstrumentKind::Harpsichord => (0.43, 0.5),
        InstrumentKind::Bass => (0.42, 0.5),
    }
}

fn c01(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}

/// Regler des Instruments für einen Punkt im Charakter-Feld (x, y je 0…1).
pub fn charakter_params(kind: InstrumentKind, x: f32, y: f32) -> Vec<(&'static str, f32)> {
    let (x, y) = (c01(x), c01(y));
    match kind {
        InstrumentKind::Drums => vec![
            ("kick.grundton", 36.0 - 7.0 * x),
            ("kick.punch", c01(0.85 - 0.45 * x + 0.1 * y)),
            ("kick.laenge", c01(0.2 + 0.75 * x)),
            ("kick.klick", c01(0.75 - 0.6 * x + 0.15 * y)),
            ("kick.waerme", c01(0.1 + 0.5 * x + 0.3 * y)),
            ("snare.kessel", 56.7 - 6.0 * x),
            ("snare.teppich", c01(0.35 + 0.5 * y)),
            ("snare.laenge", c01(0.2 + 0.5 * x)),
            ("snare.schaerfe", c01(0.4 + 0.45 * y - 0.15 * x)),
            ("clap.helligkeit", c01(0.35 + 0.4 * y)),
            ("clap.laenge", c01(0.25 + 0.5 * x)),
            ("hat.helligkeit", c01(0.45 + 0.4 * y)),
            ("hat.laenge", c01(0.18 + 0.25 * x)),
            ("hat.offen", c01(0.35 + 0.4 * x)),
            ("tom.grundton", 47.0 - 9.0 * x),
            ("tom.laenge", c01(0.3 + 0.55 * x)),
        ],
        InstrumentKind::Synth => vec![
            ("stimmen", c01(0.15 + 0.85 * x)),
            ("sub", c01(0.75 * x * x)),
            ("verstimmung", c01(0.1 + 0.2 * x + 0.6 * y)),
            ("wellenform", c01(0.2 + 0.6 * y)),
            ("schimmer", c01(0.55 * y * (1.0 - 0.4 * x))),
            ("filter", c01(0.38 + 0.15 * y + 0.07 * x)),
            ("resonanz", c01(0.05 + 0.45 * y)),
            ("bewegung", c01(0.05 + 0.75 * y)),
            ("tempo", c01(0.15 + 0.45 * y)),
        ],
        InstrumentKind::Pluck => vec![
            ("helligkeit", c01(0.15 + 0.8 * y)),
            ("anschlag", c01(0.2 + 0.7 * y)),
            ("abklingen", c01(0.15 + 0.8 * x)),
            ("koerper", c01(0.15 + 0.7 * x - 0.15 * y)),
        ],
        InstrumentKind::Organ => vec![
            ("subbass", c01((x - 0.55) / 0.45) * 0.9),
            ("prinzipal", c01(0.25 + 0.75 * x)),
            ("floete", c01(0.7 - 0.35 * x + 0.1 * y)),
            ("oktave", c01(0.05 + 0.5 * y + 0.35 * x * y)),
            ("quinte", c01((y - 0.45) * 1.6) * (0.4 + 0.6 * x)),
            ("mixtur", c01((y - 0.55) * 2.2) * (0.3 + 0.7 * x)),
            ("chiff", c01(0.25 + 0.35 * y)),
        ],
        InstrumentKind::Harpsichord => vec![
            ("achtfuss", 1.0),
            ("laute", c01(1.0 - 3.0 * x)),
            ("vierfuss", c01((x - 0.5) * 2.0)),
            ("nachklang", c01(0.2 + 0.7 * x)),
            ("kiel", c01(0.2 + 0.8 * y)),
        ],
        InstrumentKind::Bass => vec![
            ("sub", c01(0.05 + 0.85 * x)),
            ("abklingen", c01(0.2 + 0.7 * x)),
            ("filter", c01(0.4 - 0.15 * x + 0.15 * y)),
            ("wellenform", c01(0.1 + 0.7 * y)),
            ("resonanz", c01(0.05 + 0.85 * y * y)),
            ("biss", c01(0.8 * y * y + 0.1 * x * y)),
            ("anschlag", c01(0.35 + 0.55 * y - 0.1 * x)),
        ],
    }
}

/// Die drei Felder eines Instruments.
pub fn fields(kind: InstrumentKind) -> Vec<FieldSpec> {
    let (cx, cy, cp) = charakter_field(kind);
    let sets = charakter_params(kind, 0.5, 0.5).into_iter().map(|(k, _)| k).collect();
    vec![
        FieldSpec { key: "charakter", name: "Charakter", x: cx, y: cy, presets: cp, sets },
        FieldSpec {
            key: "klangfarbe",
            name: "Klangfarbe",
            x: axis("farbe", "Farbe", ["dunkel", "warm", "klar", "hell"]),
            y: axis("rauheit", "Rauheit", ["glatt", "samtig", "körnig", "rau"]),
            presets: vec![fp("Samt", 0.25, 0.12), fp("Neutral", 0.5, 0.04), fp("Glanz", 0.85, 0.1), fp("Röhre", 0.4, 0.45), fp("Staub", 0.18, 0.72), fp("Kante", 0.75, 0.62), fp("Fuzz", 0.55, 0.95)],
            sets: vec!["farbe", "rauheit"],
        },
        FieldSpec {
            key: "raum",
            name: "Raum",
            x: axis("breite", "Breite", ["schmal", "mittig", "breit", "weit"]),
            y: axis("raum", "Entfernung", ["nah", "direkt", "räumlich", "fern"]),
            presets: vec![fp("Mono", 0.0, 0.05), fp("Studio", 0.3, 0.12), fp("Bühne", 0.55, 0.35), fp("Saal", 0.75, 0.6), fp("Kirche", 0.85, 0.9), fp("Weit", 1.0, 0.45)],
            sets: vec!["breite", "raum"],
        },
    ]
}

/// Setzt einen Punkt in einem Feld: nur die Regler dieses Felds ändern sich.
pub fn apply_field(inst: &mut Instrument, field: &str, x: f32, y: f32) -> Result<(), String> {
    let (x, y) = (c01(x), c01(y));
    match field {
        "charakter" => {
            inst.params.insert(CHARAKTER_X.into(), x);
            inst.params.insert(CHARAKTER_Y.into(), y);
            for (k, v) in charakter_params(inst.kind, x, y) {
                inst.params.insert(k.into(), v);
            }
        }
        "klangfarbe" => {
            inst.params.insert("farbe".into(), x);
            inst.params.insert("rauheit".into(), y);
        }
        "raum" => {
            inst.params.insert("breite".into(), x);
            inst.params.insert("raum".into(), y);
        }
        _ => return Err(format!("Unbekanntes Feld „{field}“ (charakter, klangfarbe, raum)")),
    }
    Ok(())
}

/// Startpunkt eines Felds nach Namen (ohne Groß-/Kleinschreibung).
pub fn field_preset(kind: InstrumentKind, field: &str, name: &str) -> Result<(f32, f32), String> {
    let f = fields(kind).into_iter().find(|f| f.key == field).ok_or_else(|| format!("Unbekanntes Feld „{field}“"))?;
    f.presets
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(name.trim()))
        .map(|p| (p.x, p.y))
        .ok_or_else(|| format!("Kein Startpunkt „{name}“ im Feld {} (vorhanden: {})", f.name, f.presets.iter().map(|p| p.name).collect::<Vec<_>>().join(", ")))
}

fn verlauf_specs() -> Vec<VerlaufSpec> {
    VERLAUF_PRESETS
        .iter()
        .map(|p| VerlaufSpec { name: p.name, beschreibung: p.beschreibung, laut: p.laut.to_vec(), hell: p.hell.to_vec(), ausklang: p.ausklang, puls: p.puls, pulstiefe: p.pulstiefe })
        .collect()
}

/// Setzt eine Verlaufsvorlage (nur Synth): Kurven und Verlaufs-Regler.
pub fn apply_verlauf(inst: &mut Instrument, name: &str) -> Result<(), String> {
    if inst.kind != InstrumentKind::Synth {
        return Err("Verläufe gibt es nur beim Synth.".into());
    }
    let p = VERLAUF_PRESETS
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(name.trim()))
        .ok_or_else(|| format!("Kein Verlauf „{name}“ (vorhanden: {})", VERLAUF_PRESETS.iter().map(|p| p.name).collect::<Vec<_>>().join(", ")))?;
    inst.curves.insert("laut".into(), p.laut.to_vec());
    inst.curves.insert("hell".into(), p.hell.to_vec());
    inst.params.insert("verlauf.ausklang".into(), p.ausklang);
    inst.params.insert("verlauf.puls".into(), p.puls as f32);
    inst.params.insert("verlauf.pulstiefe".into(), p.pulstiefe);
    Ok(())
}

/// Prüft gezeichnete Kurven: „laut“/„hell“, 2–24 Punkte, Zeit 0–12 s aufsteigend, Werte 0…1.
pub fn validate_curves(given: &BTreeMap<String, Vec<[f32; 2]>>) -> Result<BTreeMap<String, Vec<[f32; 2]>>, String> {
    let mut out = BTreeMap::new();
    for (k, pts) in given {
        if k != "laut" && k != "hell" {
            return Err(format!("Unbekannte Kurve „{k}“ (laut, hell)"));
        }
        if pts.len() < 2 || pts.len() > 24 {
            return Err(format!("Kurve „{k}“: 2 bis 24 Punkte"));
        }
        let mut last = -1.0f32;
        let mut clean = Vec::with_capacity(pts.len());
        for [t, v] in pts {
            if !t.is_finite() || !v.is_finite() {
                return Err(format!("Kurve „{k}“: nur Zahlen"));
            }
            let t = t.clamp(0.0, 12.0).max(last);
            last = t;
            clean.push([t, v.clamp(0.0, 1.0)]);
        }
        out.insert(k.clone(), clean);
    }
    Ok(out)
}

// ---------------------------------------------------------------- Instrumente

/// Wärme und Schwebung (Klangformung), Farbe und Rauheit (Klangfarbe), Breite
/// und Entfernung (Raum) – dieselben Regler überall, damit man sie sich merkt.
fn shaping(waerme: f32, schwebung: f32, raum: f32, breite: f32) -> Vec<ParamSpec> {
    let mut v = vec![
        p("waerme", "Wärme", "Klangformung", waerme, "klar", "warm", "Fülle in den unteren Mitten, weichere Höhen und sanfte Röhren-Sättigung"),
        p("schwebung", "Schwebung", "Klangformung", schwebung, "rein", "schwebend", "Leicht verstimmte Kopien (Chorus): breiter und lebendiger, wie mehrere Saiten"),
    ];
    v.extend(colour_and_space(raum, breite));
    v
}

/// Klangfarbe und Raum (auch beim Schlagzeug).
fn colour_and_space(raum: f32, breite: f32) -> Vec<ParamSpec> {
    vec![
        p("farbe", "Farbe", "Klangfarbe", 0.5, "dunkel", "hell", "Kippfilter: dunkel senkt die Höhen, hell hebt sie (Mitte = neutral)"),
        p("rauheit", "Rauheit", "Klangfarbe", 0.0, "glatt", "rau", "Schnelles Flattern der Lautstärke, Sättigung und Körnung – sauber ↔ dreckig"),
        p("breite", "Breite", "Raum", breite, "schmal", "breit", "Stereo-Breite; die Tiefen bleiben in der Mitte"),
        p("raum", "Entfernung", "Raum", raum, "nah", "fern", "Anteil im Hall, aus der Ferne auch leiser und dumpfer – vom Studio bis zur Kirche"),
    ]
}

/// Unsichtbare Regler: Punkt im Charakter-Feld.
fn karte(kind: InstrumentKind) -> Vec<ParamSpec> {
    let (x, y) = charakter_default(kind);
    let mut a = p(CHARAKTER_X, "Charakter waagrecht", "Karte", x, "", "", "Waagrechte Stellung im Charakter-Feld");
    let mut b = p(CHARAKTER_Y, "Charakter senkrecht", "Karte", y, "", "", "Senkrechte Stellung im Charakter-Feld");
    a.hidden = true;
    b.hidden = true;
    vec![a, b]
}

fn hidden(key: &'static str, label: &'static str, min: f32, max: f32, default: f32, meaning: &'static str) -> ParamSpec {
    ParamSpec { key, label, group: "Verlauf", min, max, default, left: "", right: "", note: false, meaning, hidden: true }
}

/// Instrument-Beschreibung ohne Felder (Regler, Vorhören, Tasten).
fn base_spec(kind: InstrumentKind) -> InstrumentSpec {
    let keyboard_previews = || vec![pv("akkord", "Akkord"), pv("tonleiter", "Tonleiter"), pv("einzelton", "Einzelton")];
    let (name, description, params, previews, keys): (&'static str, &'static str, Vec<ParamSpec>, Vec<PreviewSpec>, Vec<KeySpec>) = match kind {
        InstrumentKind::Pluck => (
            "Zupfer",
            "Gezupfte Saite (Karplus-Strong): Gitarre, Harfe, Fingerbass",
            [
                p("helligkeit", "Helligkeit", "Klang", 0.55, "dunkel", "hell", "Dämpfung der hohen Teiltöne in der Saite"),
                p("abklingen", "Ausklingen", "Klang", 0.5, "kurz", "lang", "Wie lange der Ton nachklingt"),
                p("koerper", "Körper", "Klang", 0.4, "dünn", "warm", "Stärke der Korpusresonanzen (Luft, Holz, Glanz)"),
                p("anschlag", "Anschlag", "Klang", 0.5, "weich", "hart", "Anregung: Finger/Filz zur Saitenmitte ↔ Plektrum nah am Steg"),
            ]
            .into_iter()
            .chain(shaping(0.35, 0.25, 0.3, 0.35))
            .collect(),
            keyboard_previews(),
            vec![],
        ),
        InstrumentKind::Drums => (
            "Schlagzeug",
            "Synthetisches Schlagzeug – jede Taste eine Trommel (C1 Bassdrum, D1 Snare, D♯1 Clap, F♯1/A♯1 Hi-Hat, F1/A1/C2 Toms)",
            [
                pn("kick.grundton", "Grundton", "Bassdrum", 24.0, 52.0, 33.0, "Ton, auf den die Bassdrum ausklingt – passend zur Tonart des Basses wählen"),
                p("kick.punch", "Punch", "Bassdrum", 0.6, "weich", "punchig", "Stärke und Tempo des Tonhöhenabfalls"),
                p("kick.laenge", "Länge", "Bassdrum", 0.5, "kurz", "lang", "Ausklingzeit"),
                p("kick.klick", "Klick", "Bassdrum", 0.4, "rund", "klickig", "Anschlaggeräusch des Schlägels"),
                p("kick.waerme", "Wärme", "Bassdrum", 0.3, "sauber", "angezerrt", "Sättigung"),
                pn("snare.kessel", "Kesselton", "Snare", 45.0, 67.0, 54.0, "Stimmung des Snare-Kessels"),
                p("snare.teppich", "Teppich", "Snare", 0.6, "trocken", "rasselnd", "Anteil des Rasselns der Snare-Saiten"),
                p("snare.laenge", "Länge", "Snare", 0.4, "kurz", "lang", "Ausklingzeit"),
                p("snare.schaerfe", "Schärfe", "Snare", 0.5, "dumpf", "scharf", "Frequenzlage des Rasselns"),
                p("clap.helligkeit", "Helligkeit", "Clap", 0.5, "dunkel", "hell", "Klangfarbe des Klatschens"),
                p("clap.laenge", "Länge", "Clap", 0.4, "kurz", "lang", "Ausklang/Raum"),
                p("hat.helligkeit", "Helligkeit", "Hi-Hat", 0.6, "dunkel", "hell", "Klangfarbe der Becken"),
                p("hat.laenge", "Länge zu", "Hi-Hat", 0.3, "kurz", "lang", "Geschlossene Hi-Hat"),
                p("hat.offen", "Länge offen", "Hi-Hat", 0.5, "kurz", "lang", "Offene Hi-Hat (wird von der nächsten Hi-Hat abgewürgt)"),
                pn("tom.grundton", "Grundton", "Toms", 36.0, 55.0, 43.0, "Tiefe Tom; mittlere +5, hohe +10 Halbtöne"),
                p("tom.laenge", "Länge", "Toms", 0.5, "kurz", "lang", "Ausklingzeit der Toms"),
            ]
            .into_iter()
            .chain(colour_and_space(0.12, 0.15))
            .collect(),
            vec![
                pv("beat", "Beat"), pv("kick", "Bassdrum"), pv("snare", "Snare"), pv("clap", "Clap"),
                pv("hihat", "Hi-Hat"), pv("hihat-offen", "Hi-Hat offen"), pv("toms", "Toms"),
            ],
            [
                drums::KICK, drums::RIM, drums::SNARE, drums::CLAP, drums::TOM_LOW, drums::HAT_CLOSED, drums::TOM_MID, drums::HAT_OPEN, drums::TOM_HIGH,
                drums::CRASH, drums::RIDE, drums::COWBELL, drums::SHAKER,
            ]
                .iter()
                .filter_map(|&m| drums::piece_name(m).map(|(short, name)| KeySpec { midi: m, short, name }))
                .collect(),
        ),
        InstrumentKind::Bass => (
            "Bass",
            "Bass-Synthesizer: Oszillator, Sub-Oszillator, resonanter Tiefpass mit Hüllkurve, Sättigung, Glide",
            [
                p("wellenform", "Wellenform", "Oszillator", 0.5, "rund", "scharf", "Sinus ↔ Sägezahn ↔ Rechteck"),
                p("sub", "Sub", "Oszillator", 0.4, "dünn", "tief", "Sinus eine Oktave tiefer"),
                p("filter", "Filter", "Filter", 0.35, "dunkel", "hell", "Grenzfrequenz des Tiefpasses"),
                p("resonanz", "Resonanz", "Filter", 0.3, "weich", "quäkig", "Betonung an der Grenzfrequenz"),
                p("anschlag", "Anschlag", "Hüllkurve", 0.5, "weich", "knackig", "Filter-Hüllkurve und Einschwingen"),
                p("abklingen", "Ausklingen", "Hüllkurve", 0.5, "kurz", "lang", "Abklingzeit und Haltepegel"),
                p("biss", "Biss", "Charakter", 0.2, "sauber", "verzerrt", "Sättigung/Verzerrung"),
                p("glide", "Glide", "Charakter", 0.0, "aus", "gleitend", "Gleiten zwischen gebundenen Noten"),
            ]
            .into_iter()
            .chain(shaping(0.3, 0.0, 0.05, 0.0))
            .collect(),
            vec![pv("basslinie", "Basslinie"), pv("einzelton", "Einzelton")],
            vec![],
        ),
        InstrumentKind::Organ => (
            "Orgel",
            "Pfeifenorgel (additiv): Register 16′, 8′, 4′, 2⅔′, 2′ und Mixtur, Pfeifenansprache, Tremulant – für Choral und Fuge",
            [
                p("subbass", "Subbass 16′", "Register", 0.0, "aus", "voll", "Register eine Oktave tiefer – das Fundament im Pedal"),
                p("prinzipal", "Prinzipal 8′", "Register", 0.8, "aus", "voll", "Grundregister der Orgel: kräftiger Ton mit Obertönen"),
                p("floete", "Flöte 4′", "Register", 0.5, "aus", "voll", "Weiche Flöte eine Oktave höher"),
                p("quinte", "Quinte 2⅔′", "Register", 0.0, "aus", "voll", "Dritter Teilton – färbt hornartig"),
                p("oktave", "Oktave 2′", "Register", 0.3, "aus", "voll", "Zwei Oktaven höher – Glanz"),
                p("mixtur", "Mixtur", "Register", 0.0, "aus", "glänzend", "Hohe Obertonreihe – die Krone des Plenums"),
                p("chiff", "Anblasen", "Ansprache", 0.4, "weich", "spuckend", "Anblasgeräusch beim Ansprechen der Pfeifen"),
                p("tremulant", "Tremulant", "Ansprache", 0.0, "ruhig", "bebend", "Schwankung des Winds (leichtes Beben)"),
            ]
            .into_iter()
            .chain(shaping(0.2, 0.15, 0.5, 0.45))
            .collect(),
            keyboard_previews(),
            vec![],
        ),
        InstrumentKind::Harpsichord => (
            "Cembalo",
            "Kielinstrument: gezupfte Saiten mit 8′- und 4′-Chor und Lautenzug – der Klang des Wohltemperierten Klaviers",
            [
                p("achtfuss", "8′-Chor", "Register", 1.0, "aus", "voll", "Saitenchor in Normallage"),
                p("vierfuss", "4′-Chor", "Register", 0.0, "aus", "voll", "Saitenchor eine Oktave höher – silbriger, voller"),
                p("laute", "Lautenzug", "Register", 0.0, "aus", "gedämpft", "Filz an den Saiten: kurz und weich wie eine Laute"),
                p("kiel", "Kiel", "Klang", 0.6, "weich", "spitz", "Anriss des Kiels: weich ↔ brillant"),
                p("nachklang", "Nachklang", "Klang", 0.5, "kurz", "lang", "Wie lange die Saite nachklingt"),
            ]
            .into_iter()
            .chain(shaping(0.15, 0.15, 0.35, 0.4))
            .collect(),
            keyboard_previews(),
            vec![],
        ),
        InstrumentKind::Synth => (
            "Synth",
            "Synthesizer für Flächen und lange Klänge: verstimmte Oszillatoren, Sub und Schimmer, Filter – mit gezeichnetem Verlauf (Lautstärke, Helligkeit, Puls im Takt)",
            [
                p("stimmen", "Stimmen", "Oszillatoren", 0.5, "eine", "sieben", "Leicht verstimmte Stimmen je Ton (Unisono)"),
                p("verstimmung", "Verstimmung", "Oszillatoren", 0.35, "rein", "schwebend", "Abstand der Stimmen, bis 30 Cent"),
                p("wellenform", "Wellenform", "Oszillatoren", 0.45, "rund", "scharf", "Sinus ↔ Sägezahn ↔ Rechteck"),
                p("sub", "Sub", "Oszillatoren", 0.25, "dünn", "tief", "Sinus eine Oktave tiefer"),
                p("schimmer", "Schimmer", "Oszillatoren", 0.15, "matt", "glänzend", "Sägezahn eine Oktave höher"),
                p("filter", "Filter", "Filter", 0.45, "dunkel", "hell", "Grundstellung des Tiefpasses – die Kurve „Helligkeit“ wirkt darauf"),
                p("resonanz", "Resonanz", "Filter", 0.2, "weich", "singend", "Betonung an der Grenzfrequenz"),
                p("bewegung", "Bewegung", "Bewegung", 0.25, "ruhig", "wogend", "Langsames Auf und Ab im Filter"),
                p("tempo", "Tempo", "Bewegung", 0.3, "langsam", "schnell", "Tempo der Bewegung (0,05–6 Hz)"),
                hidden("verlauf.ausklang", "Ausklang", 0.05, 6.0, 1.6, "Sekunden bis zur Stille nach dem Loslassen"),
                hidden("verlauf.puls", "Puls", 0.0, 3.0, 0.0, "Puls im Takt: 0 aus, 1 Viertel, 2 Achtel, 3 Sechzehntel"),
                hidden("verlauf.pulstiefe", "Pulstiefe", 0.0, 1.0, 0.0, "Wie tief der Puls die Lautstärke absenkt"),
            ]
            .into_iter()
            .chain(shaping(0.2, 0.1, 0.45, 0.6))
            .collect(),
            vec![pv("akkord", "Akkord"), pv("folge", "Akkordfolge"), pv("einzelton", "Einzelton")],
            vec![],
        ),
    };
    InstrumentSpec { kind, name, description, params, presets: vec![], previews, keys, fields: vec![], verlauf: vec![] }
}

pub fn spec(kind: InstrumentKind) -> InstrumentSpec {
    let mut s = base_spec(kind);
    s.params.extend(karte(kind));
    // Standardwerte der Charakter-Regler = Zuordnung an der Grundstellung
    let (dx, dy) = charakter_default(kind);
    for (k, v) in charakter_params(kind, dx, dy) {
        if let Some(ps) = s.params.iter_mut().find(|p| p.key == k) {
            ps.default = (v * 100.0).round() / 100.0;
        }
    }
    s.fields = fields(kind);
    // Voreinstellungen = Startpunkte des Charakter-Felds (mit den Werten, die sie setzen)
    s.presets = s.fields[0]
        .presets
        .iter()
        .map(|fp| PresetSpec {
            name: fp.name,
            params: charakter_params(kind, fp.x, fp.y).into_iter().chain([(CHARAKTER_X, fp.x), (CHARAKTER_Y, fp.y)]).collect(),
        })
        .collect();
    if kind == InstrumentKind::Synth {
        s.verlauf = verlauf_specs();
    }
    s
}

pub fn specs() -> Vec<InstrumentSpec> {
    [InstrumentKind::Pluck, InstrumentKind::Organ, InstrumentKind::Harpsichord, InstrumentKind::Bass, InstrumentKind::Synth, InstrumentKind::Drums]
        .into_iter()
        .map(spec)
        .collect()
}

pub fn kind_name(kind: InstrumentKind) -> &'static str {
    spec(kind).name
}

/// Instrument mit Standardwerten.
pub fn default_instrument(kind: InstrumentKind) -> Instrument {
    let s = spec(kind);
    let mut inst = Instrument { kind, preset: "standard".into(), params: s.params.iter().map(|p| (p.key.to_string(), p.default)).collect(), curves: BTreeMap::new() };
    if kind == InstrumentKind::Synth {
        let v = Verlauf::default();
        inst.curves.insert("laut".into(), v.laut);
        inst.curves.insert("hell".into(), v.hell);
    }
    inst
}

/// Prüft und begrenzt Werte für ein Instrument; unbekannte Namen sind ein Fehler.
pub fn validate_params(kind: InstrumentKind, given: &BTreeMap<String, f32>) -> Result<BTreeMap<String, f32>, String> {
    let s = spec(kind);
    let mut out = BTreeMap::new();
    for (k, v) in given {
        let Some(ps) = s.params.iter().find(|p| p.key == k) else {
            let allowed: Vec<&str> = s.params.iter().map(|p| p.key).collect();
            return Err(format!("Unbekannter Parameter „{k}“ für {}. Erlaubt: {}", s.name, allowed.join(", ")));
        };
        if !v.is_finite() {
            return Err(format!("„{k}“ muss eine Zahl sein"));
        }
        out.insert(k.clone(), v.clamp(ps.min, ps.max));
    }
    Ok(out)
}

fn param(inst: Option<&Instrument>, key: &str, default: f32) -> f32 {
    inst.and_then(|i| i.params.get(key).copied()).unwrap_or(default)
}

pub fn pluck_params(inst: Option<&Instrument>) -> PluckParams {
    let d = PluckParams::default();
    PluckParams {
        brightness: param(inst, "helligkeit", d.brightness),
        decay: param(inst, "abklingen", d.decay),
        body: param(inst, "koerper", d.body),
        attack: param(inst, "anschlag", d.attack),
    }
}

pub fn drum_kit(inst: Option<&Instrument>) -> DrumKit {
    let d = DrumKit::default();
    let v = |k: &str, def: f32| param(inst, k, def);
    DrumKit {
        kick: KickParams {
            grundton: v("kick.grundton", d.kick.grundton),
            punch: v("kick.punch", d.kick.punch),
            laenge: v("kick.laenge", d.kick.laenge),
            klick: v("kick.klick", d.kick.klick),
            waerme: v("kick.waerme", d.kick.waerme),
        },
        snare: SnareParams {
            kessel: v("snare.kessel", d.snare.kessel),
            teppich: v("snare.teppich", d.snare.teppich),
            laenge: v("snare.laenge", d.snare.laenge),
            schaerfe: v("snare.schaerfe", d.snare.schaerfe),
        },
        clap: ClapParams { helligkeit: v("clap.helligkeit", d.clap.helligkeit), laenge: v("clap.laenge", d.clap.laenge) },
        hat: HatParams { helligkeit: v("hat.helligkeit", d.hat.helligkeit), laenge: v("hat.laenge", d.hat.laenge), offen: v("hat.offen", d.hat.offen) },
        tom: TomParams { grundton: v("tom.grundton", d.tom.grundton), laenge: v("tom.laenge", d.tom.laenge) },
    }
}

pub fn bass_params(inst: Option<&Instrument>) -> BassParams {
    let d = BassParams::default();
    let v = |k: &str, def: f32| param(inst, k, def);
    BassParams {
        wellenform: v("wellenform", d.wellenform),
        filter: v("filter", d.filter),
        resonanz: v("resonanz", d.resonanz),
        anschlag: v("anschlag", d.anschlag),
        abklingen: v("abklingen", d.abklingen),
        sub: v("sub", d.sub),
        biss: v("biss", d.biss),
        glide: v("glide", d.glide),
    }
}

pub fn organ_params(inst: Option<&Instrument>) -> OrganParams {
    let d = OrganParams::default();
    let v = |k: &str, def: f32| param(inst, k, def);
    OrganParams {
        subbass: v("subbass", d.subbass),
        prinzipal: v("prinzipal", d.prinzipal),
        floete: v("floete", d.floete),
        quinte: v("quinte", d.quinte),
        oktave: v("oktave", d.oktave),
        mixtur: v("mixtur", d.mixtur),
        chiff: v("chiff", d.chiff),
        tremulant: v("tremulant", d.tremulant),
    }
}

pub fn harpsichord_params(inst: Option<&Instrument>) -> HarpsichordParams {
    let d = HarpsichordParams::default();
    let v = |k: &str, def: f32| param(inst, k, def);
    HarpsichordParams {
        achtfuss: v("achtfuss", d.achtfuss),
        vierfuss: v("vierfuss", d.vierfuss),
        laute: v("laute", d.laute),
        kiel: v("kiel", d.kiel),
        nachklang: v("nachklang", d.nachklang),
    }
}

pub fn synth_params(inst: Option<&Instrument>) -> SynthParams {
    let d = SynthParams::default();
    let v = |k: &str, def: f32| param(inst, k, def);
    SynthParams {
        stimmen: v("stimmen", d.stimmen),
        verstimmung: v("verstimmung", d.verstimmung),
        wellenform: v("wellenform", d.wellenform),
        sub: v("sub", d.sub),
        schimmer: v("schimmer", d.schimmer),
        filter: v("filter", d.filter),
        resonanz: v("resonanz", d.resonanz),
        bewegung: v("bewegung", d.bewegung),
        tempo: v("tempo", d.tempo),
    }
}

/// Gezeichneter Verlauf des Synths (fehlt etwas, gilt „Anschwellen“).
pub fn verlauf(inst: Option<&Instrument>) -> Verlauf {
    let d = Verlauf::default();
    let curve = |k: &str, def: Vec<[f32; 2]>| inst.and_then(|i| i.curves.get(k)).filter(|c| !c.is_empty()).cloned().unwrap_or(def);
    Verlauf {
        laut: curve("laut", d.laut),
        hell: curve("hell", d.hell),
        ausklang: param(inst, "verlauf.ausklang", d.ausklang).clamp(0.05, 6.0),
        puls: param(inst, "verlauf.puls", 0.0).round().clamp(0.0, 3.0) as u8,
        pulstiefe: param(inst, "verlauf.pulstiefe", 0.0).clamp(0.0, 1.0),
    }
}

pub fn kind_of(inst: Option<&Instrument>) -> InstrumentKind {
    inst.map(|i| i.kind).unwrap_or_default()
}

/// Ob die Spur-Transposition die Tonhöhe ändert (beim Schlagzeug wählt die Taste die Trommel).
pub fn transposes(inst: Option<&Instrument>) -> bool {
    kind_of(inst) != InstrumentKind::Drums
}

/// Wert eines Reglers der Klangformung (fehlt er, gilt der Standard des Instruments).
fn shaping_value(inst: Option<&Instrument>, key: &str) -> f32 {
    let kind = kind_of(inst);
    let has = spec(kind).params.iter().any(|p| p.key == key);
    if has { param(inst, key, spec_default(kind, key)).clamp(0.0, 1.0) } else { 0.0 }
}

/// Anteil, den die Engine in den Hall schickt (Regler „Raum“).
pub fn reverb_send(inst: Option<&Instrument>) -> f32 {
    shaping_value(inst, "raum") * 0.6
}

/// Gerenderte Spur: Mitte und – bei Breite – Seite (Stereo, siehe `PlacedClip`).
pub struct Rendered {
    pub mid: Vec<f32>,
    pub side: Option<Vec<f32>>,
}

/// Rendert Noten mit dem Klangerzeuger der Spur – samt Wärme, Schwebung,
/// Rauheit, Farbe, Entfernung und Breite (den Hall fügt die Engine hinzu, siehe
/// [`reverb_send`]). `spb` = Sekunden je Schlag (Puls des Synths).
pub fn render_track(inst: Option<&Instrument>, notes: &[NoteSpec], total_secs: f32, sr: f32, spb: f32) -> Rendered {
    let mut out = match kind_of(inst) {
        InstrumentKind::Pluck => render_notes(notes, total_secs, sr, &pluck_params(inst)),
        InstrumentKind::Drums => drums::render_drums(notes, total_secs, sr, &drum_kit(inst)),
        InstrumentKind::Bass => render_bass(notes, total_secs, sr, &bass_params(inst)),
        InstrumentKind::Organ => render_organ(notes, total_secs, sr, &organ_params(inst)),
        InstrumentKind::Harpsichord => render_harpsichord(notes, total_secs, sr, &harpsichord_params(inst)),
        InstrumentKind::Synth => render_synth(notes, total_secs, sr, &synth_params(inst), &verlauf(inst), spb),
    };
    fx::warmth(&mut out, shaping_value(inst, "waerme"), sr);
    fx::chorus(&mut out, shaping_value(inst, "schwebung"), sr);
    fx::roughness(&mut out, shaping_value(inst, "rauheit"), sr);
    fx::tone(&mut out, shaping_value(inst, "farbe"), sr);
    fx::distance(&mut out, shaping_value(inst, "raum"), sr);
    let side = fx::widen(&out, shaping_value(inst, "breite"), sr);
    Rendered { mid: out, side }
}

/// Wie [`render_track`], nur die Mitte (Mono) – für Vorhören ohne Stereo.
pub fn render_instrument(inst: Option<&Instrument>, notes: &[NoteSpec], total_secs: f32, sr: f32) -> Vec<f32> {
    render_track(inst, notes, total_secs, sr, 0.5).mid
}

/// Übersicht der Wellenform für die Oberfläche: Testklang des Instruments,
/// je Spalte (Minimum, Maximum).
#[derive(Clone, Debug, Serialize)]
pub struct WaveView {
    pub secs: f32,
    pub peaks: Vec<[f32; 2]>,
    /// Ende des gehaltenen Tons (danach Ausklang), Sekunden
    pub held: f32,
    pub testton: &'static str,
}

pub fn wave_view(inst: &Instrument, spb: f32, columns: usize) -> WaveView {
    let (notes, secs, held, testton): (Vec<NoteSpec>, f32, f32, &'static str) = match inst.kind {
        InstrumentKind::Drums => (vec![NoteSpec::new(0.0, 0.5, drums::KICK as f32, 0.95)], 1.2, 0.5, "Bassdrum"),
        InstrumentKind::Synth => {
            let v = verlauf(Some(inst));
            let last = v.laut.last().map(|p| p[0]).unwrap_or(0.0).max(v.hell.last().map(|p| p[0]).unwrap_or(0.0));
            let held = (last + 0.8).clamp(2.0, 8.0);
            let notes = [57.0, 60.0, 64.0].iter().map(|&m| NoteSpec::new(0.0, held, m, 0.85)).collect();
            (notes, held, held, "Akkord a-Moll, gehalten")
        }
        InstrumentKind::Bass => (vec![NoteSpec::new(0.0, 1.2, 33.0, 0.95)], 1.6, 1.2, "Ton A1"),
        InstrumentKind::Organ => (vec![NoteSpec::new(0.0, 1.5, 60.0, 0.85)], 2.0, 1.5, "Ton c′"),
        _ => (vec![NoteSpec::new(0.0, 2.0, 57.0, 0.9)], 2.6, 2.0, "Ton A3"),
    };
    // Übersicht: halbe Abtastrate genügt (alle Filter liegen darunter)
    let sr = 24_000.0;
    let r = render_track(Some(inst), &notes, secs, sr, spb);
    let x = &r.mid;
    let total = (x.len() as f32 / sr).max(0.1);
    let per = (x.len() / columns.max(1)).max(1);
    let peaks = x
        .chunks(per)
        .map(|c| [c.iter().copied().fold(0.0f32, f32::min), c.iter().copied().fold(0.0f32, f32::max)])
        .collect();
    WaveView { secs: total, peaks, held, testton }
}

// ---------------------------------------------------------------- Muster

/// Ein Takt zum Ausprobieren (Schläge relativ zum Taktanfang).
pub fn pattern(kind: InstrumentKind) -> Vec<Note> {
    let n = |pitch: u8, start: f64, len: f64, vel: u8| Note::new(pitch, start, len, vel);
    match kind {
        InstrumentKind::Drums => {
            let mut v = vec![n(drums::KICK, 0.0, 0.25, 120), n(drums::KICK, 1.75, 0.25, 100), n(drums::KICK, 2.5, 0.25, 110),
                n(drums::SNARE, 1.0, 0.25, 115), n(drums::SNARE, 3.0, 0.25, 115)];
            for i in 0..8 {
                let t = i as f64 * 0.5;
                if t == 3.5 {
                    v.push(n(drums::HAT_OPEN, t, 0.25, 90));
                } else {
                    v.push(n(drums::HAT_CLOSED, t, 0.25, if i % 2 == 0 { 95 } else { 70 }));
                }
            }
            v
        }
        InstrumentKind::Bass => vec![
            n(33, 0.0, 0.5, 115), n(33, 0.75, 0.25, 90), n(45, 1.0, 0.5, 105), n(33, 1.5, 0.5, 100),
            n(36, 2.0, 0.75, 110), n(38, 3.0, 0.5, 105), n(31, 3.5, 0.5, 100),
        ],
        InstrumentKind::Pluck => vec![
            n(60, 0.0, 0.5, 100), n(64, 0.5, 0.5, 90), n(67, 1.0, 0.5, 95), n(72, 1.5, 0.5, 90),
            n(67, 2.0, 0.5, 95), n(64, 2.5, 0.5, 90), n(60, 3.0, 1.0, 100),
        ],
        // Choralartige Kadenz in Halben (Bass, Tenor, Alt, Sopran): C – G/H
        InstrumentKind::Organ => vec![
            n(48, 0.0, 2.0, 90), n(55, 0.0, 2.0, 85), n(64, 0.0, 2.0, 85), n(72, 0.0, 2.0, 90),
            n(47, 2.0, 2.0, 90), n(55, 2.0, 2.0, 85), n(62, 2.0, 2.0, 85), n(67, 2.0, 2.0, 90),
        ],
        // Liegender Akkord (a-Moll)
        InstrumentKind::Synth => vec![n(57, 0.0, 3.8, 85), n(60, 0.0, 3.8, 80), n(64, 0.0, 3.8, 80)],
        // Gebrochene Akkorde wie in einem Präludium
        InstrumentKind::Harpsichord => vec![
            n(48, 0.0, 2.0, 90), n(52, 0.25, 1.75, 85), n(55, 0.5, 0.25, 85), n(60, 0.75, 0.25, 85), n(64, 1.0, 0.25, 90),
            n(55, 1.25, 0.25, 85), n(60, 1.5, 0.25, 85), n(64, 1.75, 0.25, 85),
            n(48, 2.0, 2.0, 90), n(52, 2.25, 1.75, 85), n(55, 2.5, 0.25, 85), n(60, 2.75, 0.25, 85), n(64, 3.0, 0.25, 90),
            n(55, 3.25, 0.25, 85), n(60, 3.5, 0.25, 85), n(64, 3.75, 0.25, 85),
        ],
    }
}

/// Muster über mehrere Takte wiederholt (für neue Spuren). Der Synth spielt
/// eine Akkordfolge (a – F – C – G), je Takt ein Akkord.
pub fn pattern_bars(kind: InstrumentKind, bars: usize, beats_per_bar: f64) -> Vec<Note> {
    if kind == InstrumentKind::Synth {
        const FOLGE: [[u8; 3]; 4] = [[57, 60, 64], [53, 57, 60], [48, 52, 55], [55, 59, 62]];
        return (0..bars)
            .flat_map(|b| FOLGE[b % 4].iter().map(move |&m| Note::new(m, b as f64 * beats_per_bar, beats_per_bar * 0.95, 85)))
            .collect();
    }
    let one = pattern(kind);
    (0..bars)
        .flat_map(|b| one.iter().map(move |n| Note { start_beats: n.start_beats + b as f64 * beats_per_bar, ..n.clone() }))
        .collect()
}

/// Note (Schläge) → zu spielender Ton (Sekunden) mit Ausdruck.
pub fn note_spec(n: &Note, spb: f32, shift: f32) -> NoteSpec {
    NoteSpec {
        start_secs: n.start_beats as f32 * spb,
        dur_secs: n.length_beats as f32 * spb,
        midi: n.pitch as f32 + shift,
        velocity: n.velocity as f32 / 127.0,
        vibrato: n.vibrato,
        brightness: n.brightness,
        bend: n.bend,
    }
}

fn notes_to_specs(notes: &[Note], spb: f32, shift: f32) -> Vec<NoteSpec> {
    notes.iter().map(|n| note_spec(n, spb, shift)).collect()
}

/// Vorhör-Muster: Noten in Sekunden und Gesamtlänge.
pub fn preview_notes(inst: Option<&Instrument>, key: &str, bpm: f64, shift: f32) -> (Vec<NoteSpec>, f32) {
    let spb = (60.0 / bpm) as f32;
    let one = |midi: u8, secs: f32| (vec![NoteSpec::new(0.0, secs * 0.8, midi as f32, 0.95)], secs);
    match kind_of(inst) {
        InstrumentKind::Pluck | InstrumentKind::Organ | InstrumentKind::Harpsichord => {
            let base = if kind_of(inst) == InstrumentKind::Organ { 60.0 } else { 57.0 };
            let n = |start: f32, dur: f32, iv: f32, vel: f32| NoteSpec::new(start, dur, base + shift + iv, vel);
            match key {
                "akkord" => {
                    let mut v: Vec<NoteSpec> = [0.0, 4.0, 7.0, 12.0].iter().enumerate().map(|(i, &iv)| n(i as f32 * 0.22, 0.5, iv, 0.8)).collect();
                    v.extend([0.0, 4.0, 7.0, 12.0].iter().map(|&iv| n(1.0, 1.6, iv, 0.7)));
                    (v, 3.0)
                }
                "tonleiter" => {
                    let steps = [0.0, 2.0, 4.0, 5.0, 7.0, 9.0, 11.0, 12.0];
                    (steps.iter().enumerate().map(|(i, &iv)| n(i as f32 * 0.25, 0.3, iv, 0.8)).collect(), 2.6)
                }
                _ => (vec![n(0.0, 1.8, 0.0, 0.9)], 2.4),
            }
        }
        InstrumentKind::Drums => match key {
            "kick" => one(drums::KICK, 1.2),
            "snare" => one(drums::SNARE, 0.8),
            "clap" => one(drums::CLAP, 0.8),
            "hihat" => one(drums::HAT_CLOSED, 0.5),
            "hihat-offen" => one(drums::HAT_OPEN, 1.0),
            "toms" => (
                [drums::TOM_HIGH, drums::TOM_MID, drums::TOM_LOW]
                    .iter()
                    .enumerate()
                    .map(|(i, &m)| NoteSpec::new(i as f32 * 0.3, 0.25, m as f32, 0.9))
                    .collect(),
                1.8,
            ),
            _ => (notes_to_specs(&pattern_bars(InstrumentKind::Drums, 2, 4.0), spb, 0.0), 8.0 * spb + 0.6),
        },
        InstrumentKind::Bass => match key {
            "einzelton" => (vec![NoteSpec::new(0.0, 1.5, 33.0 + shift, 0.95)], 1.8),
            _ => (notes_to_specs(&pattern_bars(InstrumentKind::Bass, 2, 4.0), spb, shift), 8.0 * spb + 0.3),
        },
        InstrumentKind::Synth => {
            let n = |start: f32, dur: f32, m: f32| NoteSpec::new(start, dur, m + shift, 0.8);
            match key {
                "folge" => (notes_to_specs(&pattern_bars(InstrumentKind::Synth, 4, 4.0), spb, shift), 16.0 * spb + 1.5),
                "einzelton" => (vec![n(0.0, 2.5, 57.0)], 4.0),
                _ => (vec![n(0.0, 3.0, 57.0), n(0.0, 3.0, 60.0), n(0.0, 3.0, 64.0)], 4.6),
            }
        }
    }
}

/// Alle Vorhör-Muster aller Instrumente (für das MCP-Schema).
pub fn all_preview_keys() -> Vec<&'static str> {
    let mut v: Vec<&'static str> = Vec::new();
    for k in specs().iter().flat_map(|s| s.previews.iter().map(|p| p.key)) {
        if !v.contains(&k) {
            v.push(k);
        }
    }
    v
}

// ---------------------------------------------------------------- Messwerte

const NOTE_NAMES: [&str; 12] = ["C", "C#", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "H"];

pub fn note_name(midi: u8) -> String {
    format!("{}{}", NOTE_NAMES[midi as usize % 12], midi as i32 / 12 - 1)
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Messung {
    pub label: String,
    pub wort: String,
    /// 0…1 für die Balkenanzeige
    pub wert: f32,
    pub detail: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SoundReport {
    pub instrument: &'static str,
    pub params: BTreeMap<String, f32>,
    /// Messwerte für Oberfläche und Agent (Wort + Zahl).
    pub anzeige: Vec<Messung>,
    /// Kurzbeschreibung: Eigenschaft → Wort.
    pub beschreibung: BTreeMap<String, String>,
    pub testton: String,
}

fn bucket(v: f32, edges: [f32; 2], words: [&'static str; 3]) -> &'static str {
    if v < edges[0] {
        words[0]
    } else if v < edges[1] {
        words[1]
    } else {
        words[2]
    }
}

fn word3(v: f32, words: [&'static str; 3]) -> &'static str {
    bucket(v, [0.34, 0.67], words)
}

fn messung(label: &str, wort: &str, wert: f32, detail: String) -> Messung {
    Messung { label: label.into(), wort: wort.into(), wert: wert.clamp(0.0, 1.0), detail }
}

fn hz(midi: f32) -> f32 {
    klang_dsp::notes::midi_to_hz(midi)
}

const SR: f32 = 48_000.0;

/// Misst den Klang des Instruments an Testtönen.
pub fn report(inst: &Instrument) -> SoundReport {
    let kind = inst.kind;
    let s = spec(kind);
    let params: BTreeMap<String, f32> = s
        .params
        .iter()
        .map(|p| (p.key.to_string(), (param(Some(inst), p.key, p.default) * 100.0).round() / 100.0))
        .collect();
    let (anzeige, testton) = match kind {
        InstrumentKind::Pluck => report_pluck(&pluck_params(Some(inst))),
        InstrumentKind::Drums => report_drums(&drum_kit(Some(inst))),
        InstrumentKind::Bass => report_bass(&bass_params(Some(inst))),
        InstrumentKind::Organ => report_organ(&organ_params(Some(inst))),
        InstrumentKind::Harpsichord => report_harpsichord(&harpsichord_params(Some(inst))),
        InstrumentKind::Synth => report_synth(&synth_params(Some(inst)), &verlauf(Some(inst))),
    };
    let beschreibung = anzeige.iter().map(|m| (m.label.clone(), m.wort.clone())).collect();
    SoundReport { instrument: s.name, params, anzeige, beschreibung, testton }
}

fn report_pluck(p: &PluckParams) -> (Vec<Messung>, String) {
    const TEST_HZ: f32 = 220.0;
    let x = klang_dsp::synth::pluck(TEST_HZ, 2.5, 1.0, SR, p, 7);
    let f = analyze_sound(&x, SR);
    let rel = f.centroid_hz / TEST_HZ;
    let onset_rel = f.onset_centroid_hz / TEST_HZ;
    // Schwellen kalibriert mit `cargo test -p klang-dsp print_calibration_grid -- --ignored --nocapture`.
    (
        vec![
            messung("Helligkeit", bucket(rel, [2.4, 3.6], ["dunkel", "ausgewogen", "hell"]), (rel - 1.8) / 5.0, format!("{:.0} Hz", f.centroid_hz)),
            messung("Wärme", bucket(f.low_share, [0.52, 0.61], ["dünn", "mittel", "warm"]), (f.low_share - 0.44) / 0.22, format!("{:.1} % tief", f.low_share * 100.0)),
            messung("Anschlag", bucket(onset_rel, [5.0, 9.0], ["weich", "mittel", "hart"]), (onset_rel - 1.5) / 12.0, format!("{:.0} Hz", f.onset_centroid_hz)),
            messung("Ausklingen", bucket(f.decay_s, [0.6, 1.6], ["kurz", "mittel", "lang"]), f.decay_s / 3.0, format!("{:.2} s", f.decay_s)),
        ],
        "Testton A3 (220 Hz)".into(),
    )
}

fn report_drums(kit: &DrumKit) -> (Vec<Messung>, String) {
    let kick = drums::kick(SR, &kit.kick, 1.0, 3);
    let kick_pitch = hit_pitches(&kick, SR, &[0.0])[0].midi;
    let kick_f = analyze_sound(&kick, SR);
    let snare = drums::snare(SR, &kit.snare, 1.0, 5);
    let snare_pitch = hit_pitches(&snare, SR, &[0.0])[0].midi;
    let hat = drums::hat(SR, &kit.hat, false, 1.0, 9);
    let hat_f = analyze_sound(&hat, SR);
    let name = |m: Option<f32>| m.map(|m| note_name(m.round().clamp(0.0, 127.0) as u8)).unwrap_or_else(|| "–".into());
    let detail_hz = |m: Option<f32>| m.map(|m| format!("{:.1} Hz gemessen", hz(m))).unwrap_or_else(|| "nicht messbar".into());
    (
        vec![
            messung("Bassdrum-Grundton", &name(kick_pitch), (kick_pitch.unwrap_or(24.0) - 24.0) / 28.0, detail_hz(kick_pitch)),
            messung("Bassdrum-Punch", word3(kit.kick.punch, ["weich", "mittel", "punchig"]), kit.kick.punch, format!("Anschlag {:.0} Hz", kick_f.onset_centroid_hz)),
            messung("Bassdrum-Länge", bucket(kick_f.decay_s, [0.35, 0.8], ["kurz", "mittel", "lang"]), kick_f.decay_s / 1.3, format!("{:.2} s", kick_f.decay_s)),
            messung("Snare-Kessel", &name(snare_pitch), (snare_pitch.unwrap_or(45.0) - 45.0) / 22.0, detail_hz(snare_pitch)),
            messung("Snare-Teppich", word3(kit.snare.teppich, ["trocken", "mittel", "rasselnd"]), kit.snare.teppich, format!("Schärfe {:.0} %", kit.snare.schaerfe * 100.0)),
            messung("Hi-Hat", word3(kit.hat.helligkeit, ["dunkel", "mittel", "hell"]), kit.hat.helligkeit, format!("{:.0} Hz", hat_f.centroid_hz)),
        ],
        "Einzelschläge (Bassdrum, Snare, Hi-Hat)".into(),
    )
}

fn report_organ(p: &OrganParams) -> (Vec<Messung>, String) {
    let x = render_organ(&[NoteSpec::new(0.0, 1.2, 60.0, 0.8)], 1.5, SR, p);
    let f = analyze_sound(&x, SR);
    let rel = f.centroid_hz / 261.6;
    let registers = [("16′", p.subbass), ("8′", p.prinzipal), ("4′", p.floete), ("2⅔′", p.quinte), ("2′", p.oktave), ("Mixtur", p.mixtur)];
    let drawn: Vec<&str> = registers.iter().filter(|r| r.1 > 0.05).map(|r| r.0).collect();
    let weight = registers.iter().map(|r| r.1).sum::<f32>();
    let tiefe = (p.subbass * 1.0 + p.prinzipal * 0.3) / 1.3;
    (
        vec![
            messung("Registrierung", bucket(weight, [1.2, 2.6], ["leise", "mittel", "volles Werk"]), weight / 4.5, if drawn.is_empty() { "kein Register".into() } else { drawn.join(" + ") }),
            messung("Helligkeit", bucket(rel, [2.2, 4.5], ["dunkel", "ausgewogen", "glänzend"]), (rel - 1.0) / 7.0, format!("{:.0} Hz", f.centroid_hz)),
            messung("Tiefe", word3(tiefe, ["schlank", "mittel", "gravitätisch"]), tiefe, format!("Subbass {:.0} %", p.subbass * 100.0)),
            messung("Ansprache", word3(p.chiff, ["weich", "mittel", "spuckend"]), p.chiff, if p.tremulant > 0.05 { format!("Tremulant {:.0} %", p.tremulant * 100.0) } else { "ohne Tremulant".into() }),
        ],
        "Testton c' (262 Hz)".into(),
    )
}

fn report_harpsichord(p: &HarpsichordParams) -> (Vec<Messung>, String) {
    let x = render_harpsichord(&[NoteSpec::new(0.0, 2.0, 57.0, 0.8)], 2.4, SR, p);
    let f = analyze_sound(&x, SR);
    let rel = f.centroid_hz / 220.0;
    let chor = match (p.achtfuss > 0.05, p.vierfuss > 0.05) {
        (true, true) => "8′ + 4′",
        (true, false) => "8′",
        (false, true) => "4′",
        _ => "kein Chor",
    };
    (
        vec![
            messung("Register", chor, (p.achtfuss + p.vierfuss) / 2.0, if p.laute > 0.05 { format!("Lautenzug {:.0} %", p.laute * 100.0) } else { "ohne Lautenzug".into() }),
            messung("Brillanz", bucket(rel, [5.0, 9.0], ["weich", "mittel", "brillant"]), (rel - 2.0) / 10.0, format!("{:.0} Hz", f.centroid_hz)),
            messung("Nachklang", bucket(f.decay_s, [0.5, 1.2], ["kurz", "mittel", "lang"]), f.decay_s / 2.0, format!("{:.2} s", f.decay_s)),
        ],
        "Testton A3 (220 Hz)".into(),
    )
}

fn report_synth(p: &SynthParams, v: &Verlauf) -> (Vec<Messung>, String) {
    let notes: Vec<NoteSpec> = [57.0, 60.0, 64.0].iter().map(|&m| NoteSpec::new(0.0, 3.0, m, 0.85)).collect();
    let x = render_synth(&notes, 3.0, SR, p, v, 0.5);
    let f = analyze_sound(&x[(1.0 * SR) as usize..(2.8 * SR) as usize], SR);
    let rel = f.centroid_hz / 220.0;
    // Einschwingen: bis die Lautstärke-Kurve 90 % ihres Höchstwerts (in 6 s) erreicht
    let peak = (0..600).map(|i| klang_dsp::pad::curve_at(&v.laut, i as f32 * 0.01)).fold(0.0f32, f32::max).max(1e-3);
    let rise = (0..600).map(|i| i as f32 * 0.01).find(|&t| klang_dsp::pad::curve_at(&v.laut, t) >= 0.9 * peak).unwrap_or(6.0);
    let fuelle = (p.stimmen + p.sub) / 2.0;
    let puls = match v.puls {
        1 => "Viertel",
        2 => "Achtel",
        3 => "Sechzehntel",
        _ => "ohne Puls",
    };
    (
        vec![
            messung("Helligkeit", bucket(rel, [3.0, 6.0], ["dunkel", "ausgewogen", "hell"]), (rel - 1.5) / 8.0, format!("{:.0} Hz", f.centroid_hz)),
            messung("Fülle", word3(fuelle, ["dünn", "mittel", "breit"]), fuelle, format!("{} Stimmen je Ton", 1 + (p.stimmen * 6.0).round() as u32)),
            messung("Bewegung", word3(p.bewegung, ["ruhig", "bewegt", "wogend"]), p.bewegung, format!("{:.1} Hz", 0.05 + 5.95 * p.tempo * p.tempo)),
            messung("Einschwingen", bucket(rise, [0.15, 1.0], ["sofort", "weich", "langsam"]), (rise / 3.0).min(1.0), format!("{rise:.2} s")),
            messung("Ausklang", bucket(v.ausklang, [0.6, 2.0], ["kurz", "mittel", "lang"]), (v.ausklang / 4.0).min(1.0), format!("{:.1} s · {puls}", v.ausklang)),
        ],
        "Akkord a-Moll (3 s gehalten)".into(),
    )
}

fn report_bass(p: &BassParams) -> (Vec<Messung>, String) {
    let note = NoteSpec::new(0.0, 1.5, 33.0, 0.95);
    let x = render_bass(&[note], 1.8, SR, p);
    let f = analyze_sound(&x, SR);
    // Tonhöhe ohne Sub messen: der Sub-Oszillator verdoppelt sonst die Periode (eine Oktave tiefer).
    let no_sub = render_bass(&[note], 1.8, SR, &BassParams { sub: 0.0, ..*p });
    let mut pitches: Vec<f32> = detect_pitch(&no_sub[..(1.4 * SR) as usize], SR, &YinConfig::default()).iter().filter_map(|f| f.freq).collect();
    let measured = if pitches.is_empty() { None } else { Some(median(&mut pitches)) };
    (
        vec![
            messung(
                "Grundton",
                &measured.map(|h| note_name(klang_dsp::notes::hz_to_midi(h).round() as u8)).unwrap_or_else(|| "–".into()),
                0.3,
                measured.map(|h| format!("{h:.1} Hz gemessen")).unwrap_or_else(|| "nicht messbar".into()),
            ),
            messung("Helligkeit", word3(p.filter, ["dunkel", "mittel", "hell"]), p.filter, format!("{:.0} Hz", f.centroid_hz)),
            messung("Tiefe", word3(p.sub, ["dünn", "mittel", "tief"]), p.sub, format!("Sub-Oszillator {:.0} %", p.sub * 100.0)),
            messung("Biss", word3(p.biss, ["sauber", "leicht angezerrt", "verzerrt"]), p.biss, format!("Resonanz {:.0} %", p.resonanz * 100.0)),
            messung(
                "Ausklingen",
                word3(p.abklingen, ["kurz", "mittel", "lang"]),
                p.abklingen,
                format!("Abklingzeit {:.2} s, Halten {:.0} %", 0.05 + 0.8 * p.abklingen, (0.3 + 0.6 * p.abklingen) * 100.0),
            ),
        ],
        "Testton A1 (55 Hz)".into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pluck_inst(params: &[(&str, f32)]) -> Instrument {
        let mut i = default_instrument(InstrumentKind::Pluck);
        for (k, v) in params {
            i.params.insert((*k).into(), *v);
        }
        i
    }

    #[test]
    fn calibration_extremes_get_opposite_words() {
        let mid = [("helligkeit", 0.5), ("abklingen", 0.5), ("koerper", 0.5), ("anschlag", 0.5)];
        for (key, label, lo_word, hi_word) in [
            ("helligkeit", "Helligkeit", "dunkel", "hell"),
            ("koerper", "Wärme", "dünn", "warm"),
            ("anschlag", "Anschlag", "weich", "hart"),
            ("abklingen", "Ausklingen", "kurz", "lang"),
        ] {
            let mut lo = mid.to_vec();
            let mut hi = mid.to_vec();
            for (k, v) in lo.iter_mut().chain(hi.iter_mut()) {
                if *k == key {
                    *v = 0.0;
                }
            }
            for (k, v) in hi.iter_mut() {
                if *k == key {
                    *v = 1.0;
                }
            }
            let (a, b) = (report(&pluck_inst(&lo)), report(&pluck_inst(&hi)));
            assert_eq!(a.beschreibung[label], lo_word, "{key} tief");
            assert_eq!(b.beschreibung[label], hi_word, "{key} hoch");
        }
    }

    #[test]
    fn drum_report_measures_the_set_fundamental() {
        let mut inst = default_instrument(InstrumentKind::Drums);
        inst.params.insert("kick.grundton".into(), 31.0); // G1
        let r = report(&inst);
        assert_eq!(r.beschreibung["Bassdrum-Grundton"], "G1", "{:?}", r.anzeige);
        assert_eq!(r.beschreibung["Snare-Kessel"], "F#3");
    }

    #[test]
    fn bass_report_and_validation() {
        let r = report(&default_instrument(InstrumentKind::Bass));
        assert_eq!(r.beschreibung["Grundton"], "A1", "{:?}", r.anzeige);
        let ok = validate_params(InstrumentKind::Bass, &[("filter".to_string(), 3.0)].into_iter().collect()).unwrap();
        assert_eq!(ok["filter"], 1.0);
        assert!(validate_params(InstrumentKind::Bass, &[("kick.punch".to_string(), 0.5)].into_iter().collect()).is_err());
    }

    #[test]
    fn each_field_sets_only_its_own_controls() {
        for s in specs() {
            assert_eq!(s.fields.iter().map(|f| f.key).collect::<Vec<_>>(), ["charakter", "klangfarbe", "raum"], "{}", s.name);
            // die Regler der Felder überschneiden sich nicht und gibt es alle
            let mut all: Vec<&str> = s.fields.iter().flat_map(|f| f.sets.iter().copied()).collect();
            let n = all.len();
            all.sort_unstable();
            all.dedup();
            assert_eq!(all.len(), n, "{}: Felder teilen sich Regler", s.name);
            for k in &all {
                assert!(s.params.iter().any(|p| p.key == *k), "{}: {k} fehlt", s.name);
            }
            for f in &s.fields {
                for preset in &f.presets {
                    let mut inst = default_instrument(s.kind);
                    let before = inst.params.clone();
                    apply_field(&mut inst, f.key, preset.x, preset.y).unwrap();
                    for (k, v) in &inst.params {
                        let own = f.sets.contains(&k.as_str()) || (f.key == "charakter" && k.starts_with("charakter."));
                        if !own {
                            assert_eq!(before.get(k), Some(v), "{} / {} / {}: {k} hat sich geändert", s.name, f.name, preset.name);
                        }
                    }
                    let (x, y) = field_preset(s.kind, f.key, &preset.name.to_lowercase()).unwrap();
                    assert_eq!((x, y), (preset.x, preset.y));
                }
            }
        }
        assert!(apply_field(&mut default_instrument(InstrumentKind::Bass), "farbraum", 0.5, 0.5).is_err());
    }

    #[test]
    fn character_axes_move_the_sound_the_named_way() {
        // Schlagzeug: mächtig = längere, tiefere Bassdrum, weniger Klick
        let kit = |x: f32| {
            let mut i = default_instrument(InstrumentKind::Drums);
            apply_field(&mut i, "charakter", x, 0.4).unwrap();
            drum_kit(Some(&i))
        };
        let (knackig, maechtig) = (kit(0.0), kit(1.0));
        assert!(maechtig.kick.laenge > knackig.kick.laenge + 0.5);
        assert!(maechtig.kick.grundton < knackig.kick.grundton - 5.0);
        assert!(maechtig.kick.klick < knackig.kick.klick);
        // Standardwerte = Zuordnung an der Grundstellung
        let d = default_instrument(InstrumentKind::Synth);
        let (x, y) = (d.params["charakter.x"], d.params["charakter.y"]);
        for (k, v) in charakter_params(InstrumentKind::Synth, x, y) {
            assert!((d.params[k] - v).abs() < 0.01, "{k}: {} ≠ {v}", d.params[k]);
        }
    }

    #[test]
    fn synth_has_a_drawn_course_and_width_makes_stereo() {
        let mut inst = default_instrument(InstrumentKind::Synth);
        assert_eq!(inst.curves["laut"], VERLAUF_PRESETS[0].laut.to_vec());
        apply_verlauf(&mut inst, "pulsieren").unwrap();
        assert_eq!(verlauf(Some(&inst)).puls, 2);
        assert!(apply_verlauf(&mut default_instrument(InstrumentKind::Bass), "Glocke").is_err());
        let notes = [NoteSpec::new(0.0, 1.0, 57.0, 0.9)];
        let wide = render_track(Some(&inst), &notes, 1.5, SR, 0.5);
        assert!(wide.side.is_some(), "Synth ist von Haus aus breit");
        let mut narrow = inst.clone();
        narrow.params.insert("breite".into(), 0.0);
        assert!(render_track(Some(&narrow), &notes, 1.5, SR, 0.5).side.is_none());
        let w = wave_view(&inst, 0.5, 200);
        assert!(w.peaks.len() >= 190 && w.peaks.iter().any(|p| p[1] > 0.01), "Wellenform: {} Spalten", w.peaks.len());
        // Kurven prüfen
        let ok = validate_curves(&[("laut".to_string(), vec![[0.0, 0.0], [1.0, 2.0], [0.5, 0.3]])].into_iter().collect()).unwrap();
        assert_eq!(ok["laut"], vec![[0.0, 0.0], [1.0, 1.0], [1.0, 0.3]], "Werte begrenzt, Zeit nie rückwärts");
        assert!(validate_curves(&[("tief".to_string(), vec![[0.0, 0.0], [1.0, 1.0]])].into_iter().collect()).is_err());
        assert!(validate_curves(&[("hell".to_string(), vec![[0.0, 0.0]])].into_iter().collect()).is_err());
        assert_eq!(report(&inst).beschreibung.len(), 5);
    }

    #[test]
    fn previews_render_for_all_instruments() {
        for s in specs() {
            let inst = default_instrument(s.kind);
            for pvw in &s.previews {
                let (notes, secs) = preview_notes(Some(&inst), pvw.key, 110.0, 0.0);
                let x = render_instrument(Some(&inst), &notes, secs, SR);
                assert!(x.len() > (0.3 * SR) as usize && x.iter().all(|v| v.is_finite()), "{} / {}", s.name, pvw.key);
                assert!(klang_dsp::util::peak(&x) > 0.02, "{} / {} ist stumm", s.name, pvw.key);
            }
            assert!(!pattern(s.kind).is_empty());
        }
    }
}
