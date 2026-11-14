//! Musikbibliothek, der plattformneutrale Teil: Stand, Stöbern, Suche nach
//! Titel und Melodie (auch vorgesungen), Partituren mit Stimmanalyse anzeigen,
//! vorspielen und Stimmen als Spur übernehmen.
//!
//! Herunterladen, Indizieren und das Verwalten mehrerer Bibliotheken macht die
//! Hülle der Plattform (Desktop: `app/src/library.rs`, `libraries.rs`).

use crate::state::{lock, AppState, CmdResult, Snapshot};
use klang_core::{Clip, ClipSource, Command, Id, Note, Track, TrackKind};
use klang_dsp::synth::NoteSpec;
use klang_music::analysis::analyze_score;
use klang_music::diskindex::AnyIndex;
use klang_music::facets::{dimension, tree_children, Dimension, TreeLevel, DIMENSIONS};
use klang_music::index::MIN_QUERY_NOTES;
use klang_music::library::voice_label;
use klang_music::melody::{skyline, MelodyNote};
use klang_music::score::{load_score, Score};
use serde::Serialize;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Meldet den Stand der Bibliothek an die Oberfläche („library-changed“).
pub fn emit_info(st: &AppState) {
    if let Ok(i) = info(st) {
        st.host().emit("library-changed", &i);
    }
}

/// Fortschritt und letzte Meldung der Bibliothek.
#[derive(Default)]
pub struct LibStatus {
    pub building: Option<(usize, usize)>,
    /// Einzelne Dateien werden gerade zum Index hinzugefügt.
    pub adding: bool,
    pub download: Option<DownloadProgress>,
    pub message: Option<String>,
    /// Neue Partituren werden mit der Bibliothek verschmolzen (Anteil 0…1).
    pub merging: Option<f32>,
    /// Beim Start: die aktive Bibliothek wird geladen (bzw. eingerichtet).
    pub loading: bool,
    /// Ein Download-Auftrag läuft – vom Start bis zum Ende des Übernehmens.
    /// Deckt auch die kurzen Lücken zwischen Laden, Kategorien und Indizieren ab.
    pub pipeline: Option<QueuedDownload>,
    /// Beauftragte Pakete, die warten, bis der laufende Auftrag fertig ist.
    pub queue: Vec<QueuedDownload>,
    /// Der Faden, der die Warteschlange abarbeitet, läuft.
    pub queue_runner: bool,
}

/// Ein beauftragtes Paket (läuft gerade oder wartet in der Warteschlange).
/// Beauftragt und abgearbeitet wird in der Desktop-Hülle (`app/src/library.rs`).
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct QueuedDownload {
    /// Paket aus dem Katalog (leer bei alten, beim Start fortgesetzten Aufträgen)
    pub id: String,
    pub name: String,
    /// Zielbibliothek: die beim Beauftragen aktive (Kennung, Name)
    pub library: String,
    pub library_name: String,
    #[serde(skip)]
    pub folder: Option<String>,
    #[serde(skip)]
    pub with_meta: bool,
}

/// Fortschritt eines Bibliotheks-Downloads.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DownloadProgress {
    pub name: String,
    /// „Partituren“ oder „Kategorien“ (Genre/Komponist aus PDMX.csv)
    pub phase: String,
    pub folder: String,
    /// Partituren im Zielordner
    pub files: usize,
    /// gewünschte Anzahl (None = alle)
    pub target: Option<usize>,
    /// geladene (komprimierte) Bytes und Gesamtgröße des Archivs
    pub bytes: u64,
    pub total_bytes: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LibraryInfo {
    /// Aktive Bibliothek (Kennung und Name); None, solange sie geladen wird
    id: Option<String>,
    name: Option<String>,
    voices: usize,
    scores: usize,
    /// (fertig, gesamt) während des Indizierens
    building: Option<(usize, usize)>,
    download: Option<DownloadProgress>,
    message: Option<String>,
    /// Partituren mit Merkmalen (Baumansicht); 0 = Index ohne Merkmale (neu indizieren)
    tree_scores: usize,
    /// Genre/Komponist liegen für alle enthaltenen PDMX-Pakete vor
    has_meta: bool,
    /// Partituren mit Akkordfolge [Dur, Moll]; None = Index ohne Akkordstatistik (neu indizieren)
    chord_scores: Option<[u32; 2]>,
    /// Gerade werden Dateien hinzugefügt
    adding: bool,
    /// Gerade wird in die Bibliothek übernommen (Anteil 0…1)
    merging: Option<f32>,
    /// Die Bibliothek wird beim Start geladen
    loading: bool,
    /// Paket, das gerade geladen bzw. übernommen wird
    current_pack: Option<String>,
    /// Wartende Pakete in der Reihenfolge, in der sie drankommen
    queue: Vec<QueuedDownload>,
}

pub fn info(st: &AppState) -> CmdResult<LibraryInfo> {
    let status = lock(&st.library_status)?;
    let lib = lock(&st.library)?;
    Ok(LibraryInfo {
        id: lib.as_ref().map(|l| l.id.clone()),
        name: lib.as_ref().map(|l| l.name.clone()),
        voices: lib.as_ref().map(|l| l.index.doc_count()).unwrap_or(0),
        scores: lib.as_ref().map(|l| l.index.score_count()).unwrap_or(0),
        building: status.building,
        download: status.download.clone(),
        message: status.message.clone(),
        tree_scores: lib.as_ref().map(|l| l.index.facets().len()).unwrap_or(0),
        has_meta: lib.as_ref().map(|l| l.has_meta).unwrap_or(true),
        chord_scores: lib.as_ref().and_then(|l| l.index.chords()).map(|c| c.scores),
        adding: status.adding,
        merging: status.merging,
        loading: status.loading,
        current_pack: status.pipeline.as_ref().map(|p| p.id.clone()).filter(|id| !id.is_empty()),
        queue: status.queue.clone(),
    })
}

/// Was im Hintergrund gerade läuft (für die Rückfrage beim Schließen).
pub fn busy(st: &AppState) -> Option<String> {
    let s = st.library_status.lock().ok()?;
    let now = if let Some(d) = &s.download {
        Some(if d.phase == "Kategorien" { format!("{}: Genre und Komponist werden geladen", d.name) } else { format!("{} wird heruntergeladen", d.name) })
    } else if s.merging.is_some() {
        Some("Partituren werden in die Bibliothek übernommen".to_string())
    } else {
        s.building.map(|(d, t)| if t > 0 { format!("Die Bibliothek wird indiziert ({d} von {t} Dateien)") } else { "Die Bibliothek wird indiziert".to_string() })
    };
    // Wartende Pakete gehen beim Schließen verloren – dazusagen.
    let waiting = match s.queue.len() {
        0 => None,
        1 => Some(format!("„{}“ wartet noch in der Warteschlange", s.queue[0].name)),
        n => Some(format!("{n} Pakete warten noch in der Warteschlange")),
    };
    match (now, waiting) {
        (Some(a), Some(b)) => Some(format!("{a}; {b}")),
        (a, b) => a.or(b),
    }
}

// ---------------------------------------------------------------- Stöbern (Baum)

/// Ebenen, nach denen sich die Bibliothek ordnen lässt.
pub fn dimensions() -> CmdResult<Vec<Dimension>> {
    Ok(DIMENSIONS.to_vec())
}

/// Inhalt eines Baumknotens: Zweige der nächsten Ebene oder Partituren.

pub fn tree(st: &AppState, levels: &[String], path: &[String], offset: usize, limit: usize) -> CmdResult<TreeLevel> {
    let lib = lock(&st.library)?;
    let lib = lib.as_ref().ok_or("Keine Bibliothek geladen.")?;
    if lib.index.facets().is_empty() {
        return Err("Diese Bibliothek hat noch keine Merkmale – bitte neu indizieren.".into());
    }
    let mut lv: Vec<&str> = Vec::new();
    for l in levels {
        let d = dimension(l).ok_or_else(|| format!("Unbekannte Ebene: {l} (verfügbar: {})", DIMENSIONS.map(|d| d.key).join(", ")))?;
        lv.push(d.key);
    }
    Ok(tree_children(lib.index.facets(), &lv, path, offset, limit.clamp(1, 1000)))
}

// ---------------------------------------------------------------- Suche

/// Melodie aus Text: Notennamen (deutsch: H = h, B = b; Cis/Es/Fis …, auch C#/Eb)
/// mit Oktave oder MIDI-Zahlen, optional mit Dauer: „C4:1 D4:0.5 E4“ oder „60 62 64“.
pub fn parse_melody(s: &str) -> Result<Vec<MelodyNote>, String> {
    let mut t = 0.0;
    let mut out = Vec::new();
    // Trennung an Leerzeichen/Semikolon; Kommas trennen nur ohne Dauerangabe
    // („60,62,64“), sonst sind sie Dezimalkommas („D4:0,5“).
    let tokens = s.split(|c: char| c.is_whitespace() || c == ';').flat_map(|t| {
        let t = t.trim_end_matches(',');
        if t.contains(':') { vec![t] } else { t.split(',').collect() }
    });
    for tok in tokens.filter(|p| !p.is_empty()) {
        let (name, dur) = tok.split_once(':').unwrap_or((tok, "1"));
        let duration: f64 = dur.replace(',', ".").parse().map_err(|_| format!("Ungültige Dauer „{dur}“"))?;
        let midi = parse_note(name).ok_or_else(|| format!("Unbekannter Ton „{name}“ (z. B. C4, Fis3, Bb2 oder 60)"))?;
        out.push(MelodyNote { onset: t, duration, midi });
        t += duration.max(0.01);
    }
    Ok(out)
}

fn parse_note(s: &str) -> Option<u8> {
    if let Ok(n) = s.parse::<u8>() {
        return (n <= 127).then_some(n);
    }
    let lower = s.to_lowercase();
    let mut chars = lower.chars().peekable();
    let letter = chars.next()?;
    let mut pc: i32 = match letter {
        'c' => 0,
        'd' => 2,
        'e' => 4,
        'f' => 5,
        'g' => 7,
        'a' => 9,
        'h' => 11,
        'b' => 10, // deutsch: B = B♭
        _ => return None,
    };
    let rest: String = chars.collect();
    let (acc, oct) = rest.split_at(rest.find(|c: char| c.is_ascii_digit() || c == '-').unwrap_or(rest.len()));
    pc += match acc {
        "" => 0,
        "#" | "♯" | "is" => 1,
        "##" | "isis" => 2,
        "b" | "♭" if letter != 'b' => -1,
        "b" | "♭" => 0, // „Bb“ = B♭
        "es" | "s" => -1,
        "eses" => -2,
        _ => return None,
    };
    let octave: i32 = oct.parse().ok()?;
    let midi = (octave + 1) * 12 + pc;
    (0..=127).contains(&midi).then_some(midi as u8)
}

fn clip_melody(st: &AppState, track_id: Id, clip_id: Id) -> CmdResult<Vec<MelodyNote>> {
    let clip = st.clip(track_id, clip_id)?;
    match &clip.source {
        ClipSource::Notes { notes, .. } => {
            let mut v: Vec<MelodyNote> = notes.iter().map(|n| MelodyNote { onset: n.start_beats, duration: n.length_beats, midi: n.pitch }).collect();
            v.sort_by(|a, b| a.onset.total_cmp(&b.onset).then(b.midi.cmp(&a.midi)));
            v.dedup_by(|b, a| (a.onset - b.onset).abs() < 1e-6); // oberster Ton je Einsatz
            Ok(v)
        }
        ClipSource::Audio { .. } => {
            let (_, a) = st.clip_analysis(track_id, clip_id)?;
            Ok(a.notes.iter().map(|n| MelodyNote { onset: n.start_secs, duration: n.end_secs - n.start_secs, midi: n.midi }).collect())
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibHit {
    doc: u32,
    title: String,
    composer: String,
    part: String,
    voice: String,
    source: String,
    note_count: u32,
    /// Melodie-Übereinstimmung 0…1 (nur bei Melodiesuche)
    score: Option<f32>,
    note_index: Option<usize>,
    query_len: usize,
}

pub fn search(
    st: &AppState,
    text: Option<String>,
    melody: Option<String>,
    track_id: Option<u64>,
    clip_id: Option<u64>,
    limit: Option<usize>,
) -> CmdResult<Vec<LibHit>> {
    let limit = limit.unwrap_or(40).clamp(1, 500);
    let text = text.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
    let query = match (melody.as_deref().map(str::trim).filter(|m| !m.is_empty()), track_id, clip_id) {
        (Some(m), _, _) => Some(parse_melody(m)?),
        (None, Some(t), Some(c)) => Some(clip_melody(st, Id(t), Id(c))?),
        _ => None,
    };
    let lib = lock(&st.library)?;
    let lib = lib.as_ref().ok_or(NO_LIBRARY)?;
    match query {
        Some(q) => melody_hits(&lib.index, &q, text.as_deref(), limit),
        None if text.is_some() => Ok(lib
            .index
            .search_text(text.as_deref().unwrap_or(""), limit)
            .into_iter()
            .map(|(doc, i)| LibHit {
                doc,
                title: i.title.clone(),
                composer: i.composer.clone(),
                part: i.part.clone(),
                voice: i.voice.clone(),
                source: i.source.clone(),
                note_count: i.note_count,
                score: None,
                note_index: None,
                query_len: 0,
            })
            .collect()),
        None => Err("Suchtext oder Melodie angeben (oder einen Clip auswählen).".into()),
    }
}

const NO_LIBRARY: &str = "Keine Bibliothek geladen – sie wird beim Start geladen (Fenster „Bibliothek“).";

/// Melodiesuche, optional eingeschränkt auf Stücke, deren Titel/Komponist/Stimme alle Wörter von `text` enthält.
fn melody_hits(index: &AnyIndex, q: &[MelodyNote], text: Option<&str>, limit: usize) -> CmdResult<Vec<LibHit>> {
    if q.len() < MIN_QUERY_NOTES {
        return Err(format!("Für die Melodiesuche mindestens {MIN_QUERY_NOTES} Töne angeben (erkannt: {}).", q.len()));
    }
    let words: Vec<String> = text.unwrap_or("").split_whitespace().map(str::to_lowercase).collect();
    let matches_text = |i: &klang_music::index::DocInfo| {
        let hay = format!("{} {} {} {}", i.title, i.composer, i.part, i.source).to_lowercase();
        words.iter().all(|w| hay.contains(w.as_str()))
    };
    Ok(index
        .search(q, 200)
        .into_iter()
        .filter(|h| matches_text(&h.info))
        .take(limit)
        .map(|h| LibHit {
            doc: h.doc,
            title: h.info.title,
            composer: h.info.composer,
            part: h.info.part,
            voice: h.info.voice,
            source: h.info.source,
            note_count: h.info.note_count,
            score: Some(h.score),
            note_index: Some(h.note_index),
            query_len: q.len(),
        })
        .collect())
}

// ---------------------------------------------------------------- Vorsingen

/// Läuft gerade eine Vorsing-Aufnahme für die Suche? (Dann bleibt die normale Aufnahme gesperrt.)
pub static LISTENING: AtomicBool = AtomicBool::new(false);
/// Längste Vorsing-Aufnahme, die ausgewertet wird.
const LISTEN_MAX_SECS: f64 = 45.0;

const NOTE_NAMES_DE: [&str; 12] = ["C", "Cis", "D", "Es", "E", "F", "Fis", "G", "As", "A", "B", "H"];

/// Töne für das Melodiefeld: „E5:1 D5:0,5 …“ – Dauern relativ zur mittleren
/// Tonlänge (auf Viertel gerundet), deutsche Tonnamen, c' = C4.
pub fn format_melody(notes: &[MelodyNote]) -> String {
    let mut d: Vec<f64> = notes.iter().map(|n| n.duration).filter(|d| *d > 0.0).collect();
    d.sort_by(f64::total_cmp);
    let unit = d.get(d.len() / 2).copied().unwrap_or(1.0).max(1e-3);
    notes
        .iter()
        .map(|n| {
            let beats = ((n.duration / unit) * 4.0).round().max(1.0) / 4.0;
            let len = if (beats - beats.round()).abs() < 1e-9 { format!("{}", beats as i64) } else { format!("{beats}").replace('.', ",") };
            format!("{}{}:{len}", NOTE_NAMES_DE[n.midi as usize % 12], n.midi as i32 / 12 - 1)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListenResult {
    /// Erkannte Töne im Format des Melodiefelds.
    melody: String,
    notes: usize,
    key: Option<String>,
    seconds: f64,
    hits: Vec<LibHit>,
}

/// Mikrofon aufnehmen, ohne etwas ins Projekt zu legen (für die Melodiesuche).
pub fn listen_start(st: &AppState) -> CmdResult<()> {
    if lock(&st.library)?.is_none() {
        return Err(NO_LIBRARY.into());
    }
    let engine = st.engine()?;
    if engine.status().recording {
        return Err("Es läuft schon eine Aufnahme.".into());
    }
    LISTENING.store(true, Ordering::SeqCst);
    if let Err(e) = engine.start_recording() {
        LISTENING.store(false, Ordering::SeqCst);
        return Err(format!("Mikrofon: {e}"));
    }
    Ok(())
}

/// Vorsing-Aufnahme beenden, Melodie erkennen (Gesang oder Pfeifen) und suchen.
/// `cancel` verwirft die Aufnahme nur.
pub fn listen_stop(st: &AppState, text: Option<String>, cancel: Option<bool>) -> CmdResult<ListenResult> {
    if !LISTENING.swap(false, Ordering::SeqCst) {
        return Err("Es läuft keine Vorsing-Aufnahme.".into());
    }
    let rec = st.engine()?.stop_recording().map_err(|e| e.to_string())?;
    let seconds = rec.samples.len() as f64 / rec.sample_rate.max(1) as f64;
    if cancel.unwrap_or(false) {
        return Ok(ListenResult { melody: String::new(), notes: 0, key: None, seconds, hits: Vec::new() });
    }
    if seconds < 1.0 {
        return Err("Die Aufnahme war zu kurz – singe oder pfeife ein paar Takte.".into());
    }
    let mut samples = rec.samples;
    samples.truncate((LISTEN_MAX_SECS * rec.sample_rate as f64) as usize);
    let peak = klang_dsp::util::peak(&samples);
    if peak < 0.003 {
        return Err("Es kam nichts an – ist das Mikrofon an und freigegeben?".into());
    }
    if peak < 0.5 {
        klang_dsp::util::normalize(&mut samples, 0.5);
    }
    let (q, key) = crate::analysis::query_melody(&samples, rec.sample_rate);
    if q.len() < MIN_QUERY_NOTES {
        let found = match q.len() {
            0 => "Keine Töne erkannt".to_string(),
            1 => "Nur 1 Ton erkannt".to_string(),
            n => format!("Nur {n} Töne erkannt"),
        };
        return Err(format!("{found} – bitte mindestens {MIN_QUERY_NOTES} Töne deutlich und einzeln singen oder pfeifen (am besten auf „da“ oder „du“)."));
    }
    let lib = lock(&st.library)?;
    let lib = lib.as_ref().ok_or(NO_LIBRARY)?;
    let text = text.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
    let hits = melody_hits(&lib.index, &q, text.as_deref(), 40)?;
    Ok(ListenResult { melody: format_melody(&q), notes: q.len(), key, seconds, hits })
}

// ---------------------------------------------------------------- Partitur

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceView {
    part: String,
    label: String,
    role: Option<String>,
    /// Alle Töne: [Einsatz, Dauer, MIDI] in Vierteln
    notes: Vec<(f64, f64, u8)>,
    is_hit: bool,
    /// Treffer-Bereich (Viertel) in dieser Stimme
    highlight: Option<(f64, f64)>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScoreView {
    doc: u32,
    title: String,
    composer: String,
    source: String,
    key_name: Option<String>,
    tempo_bpm: Option<f64>,
    time_signature: Option<(u32, u32)>,
    mood: String,
    duration_quarters: f64,
    voices: Vec<VoiceView>,
    cadences: Vec<(f64, String)>,
    /// Hervorgehobene Stelle über alle Stimmen (z. B. ein Akkordwechsel), in Vierteln.
    range: Option<(f64, f64)>,
}

pub fn doc_and_score(st: &AppState, doc: u32) -> CmdResult<(klang_music::index::DocInfo, Score)> {
    let info = {
        let lib = lock(&st.library)?;
        let lib = lib.as_ref().ok_or("Keine Bibliothek geladen")?;
        lib.index.doc(doc).ok_or_else(|| format!("Eintrag {doc} nicht gefunden"))?
    };
    let score = load_score(Path::new(&info.source)).map_err(|e| format!("{}: {e}", info.source))?;
    Ok((info, score))
}

pub fn open(
    st: &AppState,
    doc: u32,
    note_index: Option<usize>,
    query_len: Option<usize>,
    range: Option<(f64, f64)>,
) -> CmdResult<ScoreView> {
    let (info, score) = doc_and_score(st, doc)?;
    let analysis = analyze_score(&score);
    let mut voices = Vec::new();
    for part in &score.parts {
        for v in &part.voices {
            let label = voice_label(v);
            let role = analysis
                .voices
                .iter()
                .find(|p| p.part == part.name && p.staff == v.staff && p.voice == v.voice)
                .map(|p| p.role_label.to_string());
            let is_hit = part.name == info.part && label == info.voice;
            let highlight = match (is_hit, note_index) {
                (true, Some(ni)) => {
                    let line = skyline(&v.notes);
                    line.get(ni).map(|start| {
                        let last = line.get((ni + query_len.unwrap_or(1)).saturating_sub(1).min(line.len().saturating_sub(1))).unwrap_or(start);
                        (start.onset, last.onset + last.duration)
                    })
                }
                _ => None,
            };
            voices.push(VoiceView {
                part: part.name.clone(),
                label,
                role,
                notes: v.notes.iter().map(|n| (n.onset, n.duration, n.midi)).collect(),
                is_hit,
                highlight,
            });
        }
    }
    Ok(ScoreView {
        doc,
        title: if info.title.is_empty() { "(ohne Titel)".into() } else { info.title },
        composer: info.composer,
        source: info.source,
        key_name: analysis.key_name.clone(),
        tempo_bpm: score.tempo_bpm,
        time_signature: score.time_signature,
        mood: analysis.mood.label.to_string(),
        duration_quarters: analysis.duration_quarters,
        voices,
        cadences: analysis.cadences.iter().map(|c| (c.onset, c.label.to_string())).collect(),
        range,
    })
}

/// Längster Ausschnitt, der vorgespielt wird (Viertel).
const PLAY_MAX_QUARTERS: f64 = 128.0;

/// Tempo einer Partitur (Viertel je Minute); ohne brauchbare Angabe 90.
pub fn score_tempo(score: &Score) -> f64 {
    score.tempo_bpm.filter(|b| (30.0..=240.0).contains(b)).unwrap_or(90.0)
}

/// Gewünschtes Tempo (Viertel je Minute) in vernünftigen Grenzen; ohne: das der Partitur.
fn chosen_tempo(tempo: Option<f64>, score: &Score) -> f64 {
    tempo.filter(|t| t.is_finite() && *t > 0.0).map(|t| t.clamp(TEMPO_MIN, TEMPO_MAX)).unwrap_or_else(|| score_tempo(score))
}

const TEMPO_MIN: f64 = 10.0;
const TEMPO_MAX: f64 = 400.0;

/// Noten der gewählten Stimmen (alle bei `None`) als (Beginn, Dauer, MIDI) in Vierteln, nach Beginn sortiert.
fn score_notes(score: &Score, only: Option<&[(String, String)]>) -> Vec<(f64, f64, u8)> {
    let mut notes = Vec::new();
    for p in &score.parts {
        for v in &p.voices {
            if only.is_some_and(|sel| !sel.iter().any(|(part, label)| p.name == *part && voice_label(v) == *label)) {
                continue;
            }
            notes.extend(v.notes.iter().filter(|n| n.duration > 0.0).map(|n| (n.onset, n.duration, n.midi)));
        }
    }
    notes.sort_by(|a, b| a.0.total_cmp(&b.0));
    notes
}

/// Ausschnitt [start, end) einer Partitur (Viertel) anspielen – alle Stimmen oder
/// nur die gewählten (`only` = Paare aus Teil und Stimmbezeichnung) – im Tempo der Partitur.
pub fn play_range(st: &AppState, doc: u32, start: f64, end: f64, only: Option<&[(String, String)]>) -> CmdResult<f32> {
    let (_, score) = doc_and_score(st, doc)?;
    let from = start.max(0.0);
    let to = end.max(from + 0.25).min(from + PLAY_MAX_QUARTERS);
    let spq = (60.0 / score_tempo(&score)) as f32;
    let notes: Vec<NoteSpec> = score_notes(&score, only)
        .into_iter()
        .filter_map(|(on, dur, midi)| {
            let (a, b) = (on.max(from), (on + dur).min(to));
            (b - a > 1e-6).then(|| NoteSpec::new((a - from) as f32 * spq, (b - a) as f32 * spq * 0.97, midi as f32, 0.7))
        })
        .collect();
    if notes.is_empty() {
        return Err("An dieser Stelle klingt nichts".into());
    }
    let engine = st.engine()?;
    let sr = engine.info().sample_rate as f32;
    let total = (to - from) as f32 * spq + 1.2;
    let inst = crate::sound::default_instrument(klang_core::InstrumentKind::Pluck);
    let data = crate::sound::render_instrument(Some(&inst), &notes, total, sr);
    let secs = data.len() as f32 / sr;
    engine.preview_send(data.into(), 0.8, crate::sound::reverb_send(Some(&inst)));
    Ok(secs)
}

/// So viele Viertel rendert der Partitur-Abspieler auf einmal.
const PIECE_QUARTERS: f64 = 4.0;

/// Spielt eine Partitur stückweise ab (für die Engine, siehe [`PreviewSource`]):
/// ab `pos` bis `to`; mit `looping` danach immer wieder ab `from`.
struct ScorePlayer {
    notes: Vec<(f64, f64, u8)>,
    pos: f64,
    from: f64,
    to: f64,
    looping: bool,
    /// Sekunden je Viertel.
    spq: f64,
    sr: f64,
    inst: klang_core::Instrument,
    /// Sekunden ab Beginn des Vorhörens, an denen das nächste Stück beginnt.
    t: f64,
    /// Erstes Stück eines Durchgangs: auch Noten, die schon vorher angeschlagen wurden und noch klingen.
    fresh: bool,
}

impl ScorePlayer {
    /// Noten des nächsten Stücks (Sekunden ab dessen Beginn) und dessen Länge in Vierteln; `None` = zu Ende.
    fn next_piece(&mut self) -> Option<(Vec<NoteSpec>, f64)> {
        if self.pos >= self.to - 1e-9 {
            if !self.looping {
                return None;
            }
            self.pos = self.from;
            self.fresh = true;
        }
        let (a, b) = (self.pos, (self.pos + PIECE_QUARTERS).min(self.to));
        let fresh = std::mem::take(&mut self.fresh);
        let spq = self.spq;
        let to = self.to;
        let specs = self
            .notes
            .iter()
            .filter(|&&(on, dur, _)| (on >= a - 1e-9 && on < b - 1e-9) || (fresh && on < a - 1e-9 && on + dur > a + 1e-6))
            .filter_map(|&(on, dur, midi)| {
                let (s, e) = (on.max(a), (on + dur).min(to));
                (e - s > 1e-6).then(|| NoteSpec::new(((s - a) * spq) as f32, ((e - s) * spq * 0.97) as f32, midi as f32, 0.7))
            })
            .collect();
        self.pos = b;
        Some((specs, b - a))
    }
}

impl klang_engine::PreviewSource for ScorePlayer {
    fn next(&mut self) -> Option<(klang_engine::PlacedClip, u64)> {
        let (specs, len) = self.next_piece()?;
        let start = (self.t * self.sr).round() as u64;
        self.t += len * self.spq;
        let until = (self.t * self.sr).round() as u64;
        let data: Arc<[f32]> = if specs.is_empty() {
            Vec::new().into()
        } else {
            let total = specs.iter().map(|n| n.start_secs + n.dur_secs).fold(0.0f32, f32::max) + 1.2;
            crate::sound::render_instrument(Some(&self.inst), &specs, total, self.sr as f32).into()
        };
        let send = crate::sound::reverb_send(Some(&self.inst));
        Some((klang_engine::PlacedClip { start_frame: start, data, side: None, gain_l: 0.8, gain_r: 0.8, send }, until))
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayInfo {
    /// Tempo, in dem gespielt wird (Viertel je Minute).
    pub tempo: f64,
    /// Ab hier (Viertel) …
    pub start: f64,
    /// … bis hier; mit `looping` danach wieder ab `loopStart`.
    pub end: f64,
    pub loop_start: f64,
    pub looping: bool,
}

/// Partitur vorspielen, bis gestoppt wird (`preview_stop`): ab `start` (Viertel)
/// bis zum Ende – mit `region` nur diesen Bereich, immer wieder. `voices` = Paare
/// (Teil, Stimme), ohne: alle Stimmen; `tempo` in Vierteln je Minute, ohne: das der Partitur.
pub fn play(
    st: &AppState,
    doc: u32,
    start: f64,
    region: Option<(f64, f64)>,
    voices: Option<Vec<(String, String)>>,
    tempo: Option<f64>,
) -> CmdResult<PlayInfo> {
    let (_, score) = doc_and_score(st, doc)?;
    let only = voices.as_deref().filter(|v| !v.is_empty());
    let notes = score_notes(&score, only);
    let piece_end = notes.iter().map(|n| n.0 + n.1).fold(0.0, f64::max);
    let (from, to, looping) = match region {
        Some((a, b)) if b - a > 1e-6 => (a.max(0.0), b, true),
        _ => (0.0, piece_end, false),
    };
    // Cursor außerhalb des Bereichs: an dessen Anfang
    let start = if start.is_finite() && start >= from && start < to { start } else { from };
    if !notes.iter().any(|&(on, dur, _)| on < to && on + dur > if looping { from } else { start }) {
        return Err(if looping { "Im markierten Bereich klingt nichts".into() } else { "Ab hier klingt nichts mehr".into() });
    }
    let tempo = chosen_tempo(tempo, &score);
    let engine = st.engine()?;
    let player = ScorePlayer {
        notes,
        pos: start,
        from,
        to,
        looping,
        spq: 60.0 / tempo,
        sr: engine.info().sample_rate as f64,
        inst: crate::sound::default_instrument(klang_core::InstrumentKind::Pluck),
        t: 0.0,
        fresh: true,
    };
    engine.preview_stream(Box::new(player));
    Ok(PlayInfo { tempo, start, end: to, loop_start: from, looping })
}

/// Schläge des Projekts je Viertel der Partitur: ohne `tempo` ein Viertel je
/// Viertel-Schlag (bei x/8-Takten zwei Schläge), sonst so, dass es im
/// Projekttempo wie `tempo` (Viertel je Minute) klingt.
fn beats_per_quarter(tempo: Option<f64>, project_bpm: f64, beat_unit: f64) -> f64 {
    match tempo.filter(|t| t.is_finite() && *t > 0.0) {
        Some(t) => project_bpm.max(1.0) / t.clamp(TEMPO_MIN, TEMPO_MAX),
        None => beat_unit / 4.0,
    }
}

/// Übernimmt Stimmen als Instrumentspuren – je Stimme eine Spur, alles ein
/// Undo-Schritt; mit `range` (Viertel) nur diesen Ausschnitt, ab Schlag 0.
/// Mit `tempo` (Viertel je Minute) werden die Noten so gestreckt, dass sie im
/// Projekttempo in diesem Tempo klingen; ohne gilt Viertel = Schlag.
pub fn to_track(
    st: &AppState,
    doc: u32,
    voices: Vec<(String, String)>,
    range: Option<(f64, f64)>,
    tempo: Option<f64>,
) -> CmdResult<Snapshot> {
    let (info, score) = doc_and_score(st, doc)?;
    if voices.is_empty() {
        return Err("Keine Stimme gewählt.".into());
    }
    let (bpb, per_quarter) = {
        let s = lock(&st.session)?;
        let p = &s.project;
        (p.beats_per_bar as f64, beats_per_quarter(tempo, p.tempo_bpm, p.beat_unit as f64))
    };
    // Ausschnitt: Noten im Bereich, an den Rändern gekürzt, ab 0
    let (from, to) = match range {
        Some((a, b)) if b > a => (a.max(0.0), b),
        _ => (0.0, f64::MAX),
    };
    let bars = range.map(|(a, b)| {
        let (num, den) = score.time_signature.unwrap_or((4, 4));
        let bar_q = (num.max(1) as f64 * 4.0 / den.max(1) as f64).max(0.25);
        format!(" (T. {}–{})", (a / bar_q).floor() as i64 + 1, ((b / bar_q) - 1e-6).ceil() as i64)
    });
    let suffix = bars.unwrap_or_default();
    let title = if info.title.is_empty() { "Bibliothek".to_string() } else { info.title.clone() };
    // Mehrere Stimmen desselben Teils (z. B. Klavier oben/unten): Stimmbezeichnung mit in den Namen
    let shared_part = |part: &str| voices.iter().filter(|(p, _)| p == part).count() > 1;
    // Stimmen ohne Noten im Bereich werden übersprungen
    let mut tracks = Vec::new();
    for (part, label) in &voices {
        let v = score
            .parts
            .iter()
            .filter(|p| p.name == *part)
            .flat_map(|p| p.voices.iter())
            .find(|v| voice_label(v) == *label)
            .ok_or_else(|| format!("Stimme „{part} {label}“ nicht gefunden"))?;
        let notes: Vec<Note> = v
            .notes
            .iter()
            .filter_map(|n| {
                let (a, b) = (n.onset.max(from), (n.onset + n.duration).min(to));
                (b - a > 1e-6).then(|| Note::new(n.midi, (a - from) * per_quarter, ((b - a) * per_quarter).max(0.05), 96))
            })
            .collect();
        if notes.is_empty() {
            continue;
        }
        let end = notes.iter().map(|n| n.start_beats + n.length_beats).fold(0.0, f64::max);
        let mean = notes.iter().map(|n| n.pitch as f64).sum::<f64>() / notes.len() as f64;
        let kind = if mean < 48.0 { klang_core::InstrumentKind::Bass } else { klang_core::InstrumentKind::Pluck };
        let name = if shared_part(part) { format!("{part} {label}") } else { part.clone() };
        tracks.push((name, kind, end, notes));
    }
    if tracks.is_empty() {
        let what = if voices.len() == 1 { "Diese Stimme hat" } else { "Die gewählten Stimmen haben" };
        return Err(if range.is_some() { format!("{what} im markierten Bereich keine Noten.") } else { format!("{what} keine Noten.") });
    }
    let cmd = {
        let mut s = lock(&st.session)?;
        let p = &mut s.project;
        let commands = tracks
            .into_iter()
            .map(|(name, kind, end, notes)| {
                let mut track = Track::new(p.alloc_id(), format!("{title} – {name}{suffix}"), TrackKind::Instrument);
                track.instrument = Some(crate::sound::default_instrument(kind));
                track.clips.push(Clip {
                    id: p.alloc_id(),
                    name: format!("{title}{suffix}"),
                    start_beats: 0.0,
                    source: ClipSource::Notes { length_beats: (end / bpb).ceil().max(1.0) * bpb, notes },
                });
                Command::AddTrack { track, index: None }
            })
            .collect();
        let label = if voices.len() > 1 { format!("{} Stimmen aus Bibliothek übernehmen", voices.len()) } else { "Aus Bibliothek übernehmen".into() };
        Command::Batch { label, commands }
    };
    st.execute(cmd, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn player(notes: Vec<(f64, f64, u8)>, pos: f64, from: f64, to: f64, looping: bool) -> ScorePlayer {
        ScorePlayer {
            notes,
            pos,
            from,
            to,
            looping,
            spq: 0.5,
            sr: 1000.0,
            inst: crate::sound::default_instrument(klang_core::InstrumentKind::Pluck),
            t: 0.0,
            fresh: true,
        }
    }

    #[test]
    fn score_player_starts_at_the_cursor_with_sounding_notes() {
        // lange Note ab 0 klingt beim Cursor (Viertel 2) noch: wird ab dort gespielt
        let mut p = player(vec![(0.0, 4.0, 60), (3.0, 1.0, 62), (6.5, 1.0, 64)], 2.0, 0.0, 7.5, false);
        let (first, len) = p.next_piece().unwrap();
        assert_eq!(len, 4.0);
        let got: Vec<_> = first.iter().map(|n| (n.start_secs, n.midi)).collect();
        assert_eq!(got, vec![(0.0, 60.0), (0.5, 62.0)]);
        assert!((first[0].dur_secs - 2.0 * 0.5 * 0.97).abs() < 1e-6, "nur der Rest der Note");
        let (second, len) = p.next_piece().unwrap();
        assert_eq!((len, second.len()), (1.5, 1), "letztes Stück bis zum Ende");
        assert!(p.next_piece().is_none(), "ohne Schleife ist am Ende Schluss");
    }

    #[test]
    fn score_player_loops_the_region_until_stopped() {
        let mut p = player(vec![(0.0, 1.0, 60), (1.0, 2.0, 62), (8.0, 1.0, 64)], 1.5, 1.0, 2.0, true);
        let mut rounds = Vec::new();
        for _ in 0..4 {
            let (n, len) = p.next_piece().unwrap();
            rounds.push((n.iter().map(|n| n.midi as u8).collect::<Vec<_>>(), len));
        }
        // erst der Rest ab dem Cursor, dann immer wieder der ganze Bereich – die Note bis zu dessen Ende gekürzt
        assert_eq!(rounds, vec![(vec![62], 0.5), (vec![62], 1.0), (vec![62], 1.0), (vec![62], 1.0)]);
        assert!((klang_engine::PreviewSource::next(&mut p).unwrap().1) > 0);
    }

    #[test]
    fn insert_tempo_maps_onto_the_project_tempo() {
        // ohne Angabe: Viertel = Schlag (bei x/8 zwei Schläge)
        assert_eq!(beats_per_quarter(None, 120.0, 4.0), 1.0);
        assert_eq!(beats_per_quarter(None, 120.0, 8.0), 2.0);
        // Partitur in 60 im Projekt mit 120: jede Viertel wird zwei Schläge lang
        assert_eq!(beats_per_quarter(Some(60.0), 120.0, 4.0), 2.0);
        assert_eq!(beats_per_quarter(Some(240.0), 120.0, 4.0), 0.5);
    }

    #[test]
    fn parses_german_and_english_note_names() {
        let n = |s: &str| parse_melody(s).unwrap().iter().map(|m| m.midi).collect::<Vec<_>>();
        assert_eq!(n("C4 D4 E4"), vec![60, 62, 64]);
        assert_eq!(n("Cis4 Es4 Fis3 As3"), vec![61, 63, 54, 56]);
        assert_eq!(n("H3 B3 Bb3 C#4 Eb4"), vec![59, 58, 58, 61, 63]);
        assert_eq!(n("60, 62; 64"), vec![60, 62, 64]);
        let d = parse_melody("C4:2 D4:0,5 E4").unwrap();
        assert_eq!((d[1].onset, d[2].onset), (2.0, 2.5));
        assert!(parse_melody("X4").is_err());
        assert!(parse_melody("C4:x").is_err());
    }

    /// (MIDI, Schläge) → Melodie in Schlägen
    fn tune(n: &[(u8, f64)]) -> Vec<MelodyNote> {
        let mut t = 0.0;
        n.iter()
            .map(|&(midi, d)| {
                let m = MelodyNote { onset: t, duration: d, midi };
                t += d;
                m
            })
            .collect()
    }

    const HAENSCHEN: [(u8, f64); 13] =
        [(67, 1.), (64, 1.), (64, 2.), (65, 1.), (62, 1.), (62, 2.), (60, 1.), (62, 1.), (64, 1.), (65, 1.), (67, 1.), (67, 1.), (67, 2.)];
    const ENTCHEN: [(u8, f64); 16] = [(60, 1.), (62, 1.), (64, 1.), (65, 1.), (67, 2.), (67, 2.), (69, 1.), (69, 1.), (69, 1.), (69, 1.), (67, 4.), (69, 1.), (69, 1.), (69, 1.), (69, 1.), (67, 4.)];
    const JAKOB: [(u8, f64); 14] = [(60, 1.), (62, 1.), (64, 1.), (60, 1.), (60, 1.), (62, 1.), (64, 1.), (60, 1.), (64, 1.), (65, 1.), (67, 2.), (64, 1.), (65, 1.), (67, 2.)];

    /// Gepfiffen: fast reiner Sinus mit leichtem Vibrato und Atemrauschen, kurze Lücken.
    fn whistle(notes: &[(u8, f64)], transpose: i32, secs_per_beat: f32, sr: f32) -> Vec<f32> {
        let mut out = Vec::new();
        let (mut phase, mut seed) = (0.0f32, 12345u32);
        for &(midi, beats) in notes {
            let secs = beats as f32 * secs_per_beat - 0.04;
            for i in 0..(secs * sr) as usize {
                let t = i as f32 / sr;
                let m = (midi as i32 + transpose) as f32 + 0.1 * (2.0 * std::f32::consts::PI * 6.0 * t).sin();
                phase += 2.0 * std::f32::consts::PI * 440.0 * 2f32.powf((m - 69.0) / 12.0) / sr;
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let noise = (seed >> 9) as f32 / (1u32 << 23) as f32 - 0.5;
                let env = (t / 0.015).min(1.0) * ((secs - t) / 0.025).clamp(0.0, 1.0);
                out.push(env * (0.35 * phase.sin() + 0.02 * noise));
            }
            out.extend(std::iter::repeat(0.0).take((0.04 * sr) as usize));
        }
        out
    }

    #[test]
    fn whistled_melody_above_1400_hz_is_found() {
        let sr = 44_100.0;
        // Hänschen klein, 26 Halbtöne höher (a''' ≈ 1760 Hz) und schneller gepfiffen
        let x = whistle(&HAENSCHEN[..10], 26, 0.3, sr);
        let (q, key) = crate::analysis::query_melody(&x, sr as u32);
        let got: Vec<i32> = q.windows(2).map(|w| w[1].midi as i32 - w[0].midi as i32).collect();
        let want: Vec<i32> = HAENSCHEN[..10].windows(2).map(|w| w[1].0 as i32 - w[0].0 as i32).collect();
        assert_eq!(got, want, "Intervalle: {q:?}");
        assert_eq!(q[0].midi, 93);
        // Aus zehn Tönen sind D-Dur und e-Moll gleich plausibel
        assert!(matches!(key.as_deref(), Some("D-Dur" | "e-Moll")), "{key:?}");
        assert!(format_melody(&q).starts_with("A6:1 Fis6:1 Fis6:2 G6:1"), "{}", format_melody(&q));

        let mut idx = klang_music::index::MelodyIndex::new();
        for (title, t) in [("Alle meine Entchen", &ENTCHEN[..]), ("Hänschen klein", &HAENSCHEN[..]), ("Bruder Jakob", &JAKOB[..])] {
            let info = klang_music::index::DocInfo { title: title.into(), ..Default::default() };
            idx.add_melody(info, &tune(t));
        }
        let index = AnyIndex::Mem(idx);
        let hits = melody_hits(&index, &q, None, 5).unwrap();
        assert_eq!(hits[0].title, "Hänschen klein", "{:?}", hits.iter().map(|h| (&h.title, h.score)).collect::<Vec<_>>());
        // Textfilter schränkt ein
        assert!(melody_hits(&index, &q, Some("jakob"), 5).unwrap().iter().all(|h| h.title == "Bruder Jakob"));
        // zu wenige Töne
        assert!(melody_hits(&index, &q[..3], None, 5).is_err());
    }

    #[test]
    fn sung_melody_in_low_voice_is_found() {
        // Männerstimme: Bruder Jakob eine Oktave tiefer, mit Obertönen (wie gesungen)
        let sr = 44_100.0;
        let mut x = Vec::new();
        let mut phase = 0.0f32;
        for &(midi, beats) in &JAKOB {
            let secs = beats as f32 * 0.35 - 0.04;
            for i in 0..(secs * sr) as usize {
                let t = i as f32 / sr;
                let f = 440.0 * 2f32.powf((midi as f32 - 12.0 + 0.15 * (2.0 * std::f32::consts::PI * 5.5 * t).sin() - 69.0) / 12.0);
                phase += 2.0 * std::f32::consts::PI * f / sr;
                let env = (t / 0.02).min(1.0) * ((secs - t) / 0.03).clamp(0.0, 1.0);
                x.push(env * (0.4 * phase.sin() + 0.2 * (2.0 * phase).sin() + 0.1 * (3.0 * phase).sin()));
            }
            x.extend(std::iter::repeat(0.0).take((0.04 * sr) as usize));
        }
        let (q, _) = crate::analysis::query_melody(&x, sr as u32);
        assert_eq!(q.iter().map(|n| n.midi).collect::<Vec<_>>(), JAKOB.iter().map(|n| n.0 - 12).collect::<Vec<_>>());
        let mut idx = klang_music::index::MelodyIndex::new();
        for (title, t) in [("Alle meine Entchen", &ENTCHEN[..]), ("Hänschen klein", &HAENSCHEN[..]), ("Bruder Jakob", &JAKOB[..])] {
            idx.add_melody(klang_music::index::DocInfo { title: title.into(), ..Default::default() }, &tune(t));
        }
        let hits = melody_hits(&AnyIndex::Mem(idx), &q, None, 5).unwrap();
        assert_eq!(hits[0].title, "Bruder Jakob");
    }
}
