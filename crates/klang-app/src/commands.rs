//! Die Befehlstabelle der Oberfläche – eine für alle Plattformen.
//!
//! Ein Befehl ist ein Name plus Argumente als JSON-Objekt (Schlüssel in
//! camelCase, wie die Oberfläche sie schickt); die Antwort ist JSON.
//!
//! * Desktop: Tauri leitet jeden Aufruf, dessen Name hier steht, an [`call`]
//!   weiter (`app/src/lib.rs`).
//! * Browser: der Web-Kern ruft [`call`] direkt auf (`crates/klang-web`).
//!
//! Die Oberfläche (`ui/src/bridge.ts`) benutzt auf beiden Plattformen dieselbe
//! Zuordnung. Befehle, die es nur auf einer Plattform gibt (Fenster schließen,
//! „Im Ordner zeigen“, Bibliotheken herunterladen …), behandelt die jeweilige
//! Hülle selbst.

use crate::state::{AppState, CmdResult};
use crate::{export, fugue, harmony, library, project, rhythm, song, sound, sound_library};
use klang_core::{Command, Id, InstrumentKind};
use klang_i18n::{tr, trn};
use klang_music::counterpoint::VNote;
use klang_music::fugue::{FugueOptions, Transform};
use klang_music::harmony::Chord;
use klang_music::theory::Key;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Alle Befehle des Kerns.
pub const COMMANDS: &[&str] = &[
    "set_language",
    "get_state",
    "execute",
    "undo",
    "redo",
    "transport",
    "seek",
    "set_metronome",
    "set_loop",
    "set_loop_range",
    "audio_devices",
    "audio_set_device",
    "record_start",
    "record_stop",
    "get_waveform",
    "analyze_asset",
    "create_melody_track",
    "audio_import",
    "audio_to_notes",
    "delete_range",
    "quantize_notes",
    "create_demo",
    "preview_track",
    "preview_note",
    "preview_stop",
    "describe_sound",
    "sound_wave",
    "sound_field",
    "sounds_list",
    "sound_save",
    "sound_load",
    "sound_rename",
    "sound_delete",
    "instrument_specs",
    "create_instrument_track",
    "insert_pattern",
    "chat_send",
    "chat_history",
    "project_save",
    "project_open",
    "project_new",
    "project_list",
    "library_info",
    "library_dimensions",
    "library_tree",
    "library_search",
    "library_open",
    "library_to_track",
    "library_play",
    "library_listen_start",
    "library_listen_stop",
    "song_templates",
    "song_write",
    "rhythm_info",
    "rhythm_read",
    "rhythm_toggle",
    "rhythm_swing",
    "rhythm_write",
    "rhythm_preview",
    "harmony_view",
    "harmony_suggest",
    "harmony_play_example",
    "harmony_describe",
    "harmony_chords",
    "harmony_preview",
    "harmony_write",
    "harmony_timeline",
    "fugue_subject",
    "fugue_transform",
    "fugue_countersubjects",
    "fugue_plan",
    "fugue_add_voices",
    "fugue_preview",
    "counterpoint_check",
    "export_info",
    "export_run",
];

/// Gibt es den Befehl im Kern?
pub fn has(cmd: &str) -> bool {
    COMMANDS.contains(&cmd)
}

/// Argumente als Tupel lesen: `let (a, b) = args!(v; a: u64, b: Option<f64>);`
/// – fehlende `Option`-Felder sind `None`, überzählige werden ignoriert.
macro_rules! args {
    ($v:expr; $($name:ident : $ty:ty),* $(,)?) => {{
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct A {
            $($name: $ty),*
        }
        let A { $($name),* } = A::deserialize($v).map_err(|e| tr!("Invalid input: {error}", error = e))?;
        ($($name,)*)
    }};
}

fn json<T: Serialize>(v: T) -> CmdResult<String> {
    serde_json::to_string(&v).map_err(|e| tr!("Couldn't serialize response: {error}", error = e))
}

/// Führt einen Befehl aus. `args` ist das JSON-Objekt der Oberfläche (oder
/// `null`); die Antwort ist JSON, ein Fehler ein Text für die Oberfläche.
pub fn call(st: &AppState, cmd: &str, args: &Value) -> CmdResult<String> {
    let empty = Value::Object(Default::default());
    let a = if args.is_null() { &empty } else { args };
    match cmd {
        // ------------------------------------------------ Language (the UI's dictionary)
        "set_language" => {
            let (lang, dict) = args!(a; lang: String, dict: Option<std::collections::HashMap<String, String>>);
            klang_i18n::set_language(&lang, dict.unwrap_or_default());
            json(())
        }
        // ------------------------------------------------ Projekt, Undo, Transport
        "get_state" => json(st.snapshot()?),
        "execute" => {
            let (command, merge) = args!(a; command: Command, merge: Option<bool>);
            json(st.execute(command, merge.unwrap_or(false))?)
        }
        "undo" => json(st.undo()?),
        "redo" => json(st.redo()?),
        "transport" => {
            let (action,) = args!(a; action: String);
            json(st.transport(&action)?)
        }
        "seek" => {
            let (seconds,) = args!(a; seconds: f64);
            json(st.seek_seconds(seconds)?)
        }
        "set_metronome" => {
            let (enabled,) = args!(a; enabled: bool);
            json(st.set_metronome(enabled)?)
        }
        // Wiederholen ein/aus (am Ende des Stücks bzw. des markierten Bereichs wieder von vorn)
        "set_loop" => {
            let (enabled,) = args!(a; enabled: bool);
            json(st.set_loop(Some(enabled), None)?)
        }
        // Zeitbereich (Schläge) markieren bzw. mit `null` aufheben
        "set_loop_range" => {
            let (range,) = args!(a; range: Option<(f64, f64)>);
            json(st.set_loop(None, Some(range))?)
        }
        // Ausgabe- und Aufnahmegeräte samt Wahl (Browser: nur das Gerät des Systems)
        "audio_devices" => json(st.audio_devices()?),
        // Audiogerät wählen (kind: output/input; name null = Systemstandard) – wirkt sofort und gilt beim nächsten Start
        "audio_set_device" => {
            let (kind, name) = args!(a; kind: String, name: Option<String>);
            json(st.set_audio_device(&kind, name)?)
        }

        // ------------------------------------------------ Aufnahme, Analyse
        "record_start" => json(st.record_start()?),
        "record_stop" => json(st.record_stop()?),
        "get_waveform" => {
            let (asset_id, buckets) = args!(a; asset_id: String, buckets: usize);
            json(st.waveform(&asset_id, buckets)?)
        }
        "analyze_asset" => {
            let (asset_id,) = args!(a; asset_id: String);
            json(st.analysis_for(&asset_id)?)
        }
        // Erkannte Melodie eines Audio-Clips als Instrumentspur (quantisiert auf Sechzehntel)
        "create_melody_track" => {
            let (track_id, clip_id) = args!(a; track_id: u64, clip_id: u64);
            json(st.melody_to_track(Id(track_id), Id(clip_id), &tr!("Import melody"))?.0)
        }
        // Audiodateien (Drag & Drop) als Spuren einfügen – gelesen über das Dateisystem des
        // Kerns (Desktop: die Platte; Browser: hereingezogene Dateien unter /Klangwelt/Import)
        "audio_import" => {
            let (paths, track_id, start_beats) = args!(a; paths: Vec<String>, track_id: Option<u64>, start_beats: Option<f64>);
            let files = paths
                .into_iter()
                .map(|p| {
                    let bytes = crate::vfs::read(std::path::Path::new(&p)).map_err(|e| tr!("Can't read file ({error})", error = e));
                    (p, bytes)
                })
                .collect();
            json(st.import_audio(files, track_id.map(Id), start_beats.unwrap_or(0.0))?)
        }
        // Aufnahme in eine Melodie- oder Rhythmus-Spur umwandeln (ohne clipId: alle Audio-Clips der Spur)
        "audio_to_notes" => {
            let (track_id, clip_id, target) = args!(a; track_id: u64, clip_id: Option<u64>, target: crate::convert::Target);
            let label = match target {
                crate::convert::Target::Melodie => tr!("Convert to melody track"),
                crate::convert::Target::Rhythmus => tr!("Convert to rhythm track"),
            };
            json(st.audio_to_notes(Id(track_id), clip_id.map(Id), target, &label)?)
        }
        // Zeitbereich aus allen Spuren löschen, alles danach rückt auf
        "delete_range" => {
            let (start_beats, end_beats) = args!(a; start_beats: f64, end_beats: f64);
            json(st.delete_range(start_beats, end_beats)?)
        }
        // Noten am Raster ausrichten (ohne indices: alle Noten des Clips)
        "quantize_notes" => {
            let (track_id, clip_id, indices, grid, lengths) = args!(a; track_id: u64, clip_id: u64, indices: Option<Vec<usize>>, grid: f64, lengths: bool);
            json(st.quantize_notes(Id(track_id), Id(clip_id), indices, grid, lengths, None)?)
        }
        "create_demo" => json(st.create_demo()?),

        // ------------------------------------------------ Klang, Vorhören
        "preview_track" => {
            let (track_id, pattern) = args!(a; track_id: u64, pattern: String);
            json(st.preview_track(Id(track_id), &pattern)?)
        }
        "preview_note" => {
            let (track_id, midi) = args!(a; track_id: u64, midi: u8);
            json(st.preview_note(Id(track_id), midi)?)
        }
        "preview_stop" => json(st.preview_stop()?),
        "describe_sound" => {
            let (track_id,) = args!(a; track_id: u64);
            json(st.describe_track(Id(track_id))?)
        }
        "sound_wave" => {
            let (track_id,) = args!(a; track_id: u64);
            json(st.sound_wave(Id(track_id))?)
        }
        // Punkt bzw. Startpunkt in einem Feld des Klang-Fensters setzen
        "sound_field" => {
            let (track_id, field, x, y, preset, merge) =
                args!(a; track_id: u64, field: String, x: Option<f32>, y: Option<f32>, preset: Option<String>, merge: Option<bool>);
            json(st.sound_field(Id(track_id), &field, x.zip(y), preset.as_deref(), merge.unwrap_or(false))?)
        }
        "sounds_list" => json(sound_library::list(&sound_library::file(st))?),
        "sound_save" => {
            let (track_id, name) = args!(a; track_id: u64, name: String);
            let inst = st.track_sound(Id(track_id))?;
            let r = sound_library::save(&sound_library::file(st), &name, &inst)?;
            sound_library::announce(st, &r.sounds);
            json(r)
        }
        "sound_load" => {
            let (track_id, id) = args!(a; track_id: u64, id: String);
            let saved = sound_library::find(&sound_library::file(st), &id, None)?;
            json(st.load_sound(Id(track_id), &saved, "")?)
        }
        "sound_rename" => {
            let (id, name) = args!(a; id: String, name: String);
            let l = sound_library::rename(&sound_library::file(st), &id, &name)?;
            sound_library::announce(st, &l);
            json(l)
        }
        "sound_delete" => {
            let (id,) = args!(a; id: String);
            let l = sound_library::delete(&sound_library::file(st), &id)?;
            sound_library::announce(st, &l);
            json(l)
        }
        "instrument_specs" => json(sound::specs()),
        "create_instrument_track" => {
            let (kind,) = args!(a; kind: InstrumentKind);
            json(st.create_instrument_track(kind, &tr!("Add track"))?.0)
        }
        "insert_pattern" => {
            let (track_id,) = args!(a; track_id: u64);
            json(st.insert_pattern(Id(track_id))?)
        }

        // ------------------------------------------------ Chat mit dem Agenten
        "chat_send" => {
            let (text,) = args!(a; text: String);
            // Lage im Projekt mitgeben: welche Spuren klingen, welcher Zeitraum markiert ist
            st.chat.send_user(&text, st.chat_context().ok())?;
            json(st.chat_view())
        }
        "chat_history" => json(st.chat_view()),

        // ------------------------------------------------ Projektdateien
        // Ohne `path` in die zuletzt benutzte Datei; ein bloßer Name landet im Projektordner.
        "project_save" => {
            let (path,) = args!(a; path: Option<String>);
            let target = match path.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
                Some(p) => project::resolve(p, &st.host().projects_dir()),
                None => crate::lock(&st.project_file)?.clone().ok_or_else(|| tr!("No file name yet – please use \"Save as\"."))?,
            };
            st.save_project(&target)?;
            json(st.snapshot()?)
        }
        "project_open" => {
            let (path,) = args!(a; path: String);
            let target = project::resolve(&path, &st.host().projects_dir());
            st.open_project(&target)?;
            json(changed(st)?)
        }
        "project_new" => {
            st.new_project()?;
            json(changed(st)?)
        }
        // [Projektordner, Projekte darin (neueste zuerst)]
        "project_list" => {
            let dir = st.host().projects_dir();
            json((dir.display().to_string(), project::list(&dir)))
        }

        // ------------------------------------------------ Bibliothek (Abfragen)
        "library_info" => json(library::info(st)?),
        "library_dimensions" => json(library::dimensions()?),
        "library_tree" => {
            let (levels, path, offset, limit) = args!(a; levels: Vec<String>, path: Vec<String>, offset: Option<usize>, limit: Option<usize>);
            json(library::tree(st, &levels, &path, offset.unwrap_or(0), limit.unwrap_or(200))?)
        }
        "library_search" => {
            let (text, melody, track_id, clip_id, limit) =
                args!(a; text: Option<String>, melody: Option<String>, track_id: Option<u64>, clip_id: Option<u64>, limit: Option<usize>);
            json(library::search(st, text, melody, track_id, clip_id, limit)?)
        }
        "library_open" => {
            let (doc, note_index, query_len, range) = args!(a; doc: u32, note_index: Option<usize>, query_len: Option<usize>, range: Option<(f64, f64)>);
            json(library::open(st, doc, note_index, query_len, range)?)
        }
        "library_to_track" => {
            let (doc, voices, range, tempo) = args!(a; doc: u32, voices: Vec<(String, String)>, range: Option<(f64, f64)>, tempo: Option<f64>);
            json(library::to_track(st, doc, voices, range, tempo)?)
        }
        "library_play" => {
            let (doc, start, region, voices, tempo) =
                args!(a; doc: u32, start: f64, region: Option<(f64, f64)>, voices: Option<Vec<(String, String)>>, tempo: Option<f64>);
            json(library::play(st, doc, start, region, voices, tempo)?)
        }
        "library_listen_start" => json(library::listen_start(st)?),
        "library_listen_stop" => {
            let (text, cancel) = args!(a; text: Option<String>, cancel: Option<bool>);
            json(library::listen_stop(st, text, cancel)?)
        }

        // ------------------------------------------------ Song, Rhythmus
        "song_templates" => json(song::templates()),
        "song_write" => {
            let (plan, replace) = args!(a; plan: song::SongPlan, replace: Option<bool>);
            json(st.song_write(&plan, replace.unwrap_or(true), "")?)
        }
        "rhythm_info" => json(rhythm::info()),
        "rhythm_read" => {
            let (track_id, clip_id) = args!(a; track_id: u64, clip_id: u64);
            json(st.rhythm_read(Id(track_id), Id(clip_id))?)
        }
        "rhythm_toggle" => {
            let (track_id, clip_id, row, step, on, swing) = args!(a; track_id: u64, clip_id: u64, row: String, step: usize, on: bool, swing: f64);
            json(st.rhythm_toggle(Id(track_id), Id(clip_id), &row, step, on, swing)?)
        }
        "rhythm_swing" => {
            let (track_id, clip_id, swing, merge) = args!(a; track_id: u64, clip_id: u64, swing: f64, merge: Option<bool>);
            json(st.rhythm_swing(Id(track_id), Id(clip_id), swing, merge.unwrap_or(false))?)
        }
        "rhythm_write" => {
            let (grid, swing, track_id, clip_id, start_beats, name, sound, tempo) = args!(a;
                grid: rhythm::Grid, swing: f64, track_id: Option<u64>, clip_id: Option<u64>, start_beats: f64, name: String, sound: Option<String>, tempo: Option<f64>);
            json(st.rhythm_write(grid, swing, track_id.map(Id), clip_id.map(Id), start_beats, &name, sound.as_deref(), tempo, "")?)
        }
        "rhythm_preview" => {
            let (track_id, clip_id, grid, swing, sound) = args!(a; track_id: Option<u64>, clip_id: Option<u64>, grid: Option<rhythm::Grid>, swing: f64, sound: Option<String>);
            json(st.rhythm_preview(track_id.map(Id), clip_id.map(Id), grid, swing, sound.as_deref())?)
        }

        // ------------------------------------------------ Harmonie
        "harmony_view" => {
            let (key,) = args!(a; key: Option<Key>);
            let info = harmony::resolve_key(st, key)?;
            let (circle, cadences) = (klang_music::harmony::circle(&info.key), klang_music::harmony::cadences(&info.key));
            json(harmony::HarmonyView { key: info, circle, cadences })
        }
        "harmony_suggest" => {
            let (key, chords, group) = args!(a; key: Key, chords: Vec<Chord>, group: Option<usize>);
            json(harmony::suggest_with_library(st, &key, &chords, group.unwrap_or(0))?)
        }
        "harmony_play_example" => {
            let (doc, start, end) = args!(a; doc: u32, start: f64, end: f64);
            json(harmony::play_example(st, doc, start, end)?)
        }
        "harmony_describe" => {
            let (key, chords) = args!(a; key: Key, chords: Vec<Chord>);
            json(chords.iter().map(|c| klang_music::harmony::describe(c, &key)).collect::<Vec<_>>())
        }
        "harmony_chords" => {
            let (key, track_id, clip_id) = args!(a; key: Key, track_id: Option<u64>, clip_id: Option<u64>);
            json(harmony::chords_in(st, track_id.map(Id), clip_id.map(Id), &key)?)
        }
        "harmony_preview" => {
            let (key, chords, beats_per_chord, track_id, before) =
                args!(a; key: Key, chords: Vec<Chord>, beats_per_chord: f64, track_id: Option<u64>, before: Option<Chord>);
            json(harmony::preview(st, track_id.map(Id), &chords, &key, beats_per_chord, before)?)
        }
        // `mode` „einfuegen“ schafft Platz (alles danach rückt nach hinten), sonst wird überschrieben
        "harmony_write" => {
            let (key, chords, beats_per_chord, start_beats, track_id, before, mode) = args!(a;
                key: Key, chords: Vec<Chord>, beats_per_chord: f64, start_beats: f64, track_id: Option<u64>, before: Option<Chord>, mode: Option<String>);
            let n = chords.len();
            let insert = mode.as_deref() == Some("einfuegen");
            let label = if insert { trn!(n, "Insert {n} chord", "Insert {n} chords") } else { trn!(n, "Write {n} chord", "Write {n} chords") };
            json(harmony::write_mode(st, track_id.map(Id), start_beats, &chords, &key, beats_per_chord, before, &label, insert)?.0)
        }
        "harmony_timeline" => {
            let (key,) = args!(a; key: Option<Key>);
            json(harmony::line(st, key)?)
        }

        // ------------------------------------------------ Fuge, Kontrapunkt
        "fugue_subject" => {
            let (track_id, clip_id, key) = args!(a; track_id: u64, clip_id: u64, key: Option<Key>);
            json(fugue::subject_view(st, Id(track_id), Id(clip_id), key)?)
        }
        "fugue_transform" => {
            let (track_id, clip_id, key, transform) = args!(a; track_id: u64, clip_id: u64, key: Key, transform: Transform);
            let s = fugue::subject_of(st, Id(track_id), Id(clip_id))?;
            json(klang_music::fugue::transform(&s.notes, &key, transform))
        }
        "fugue_countersubjects" => {
            let (track_id, clip_id, key) = args!(a; track_id: u64, clip_id: u64, key: Key);
            let s = fugue::subject_of(st, Id(track_id), Id(clip_id))?;
            json(klang_music::fugue::countersubjects(&s.notes, &key, fugue::bpb(st)?, 4))
        }
        "fugue_plan" => {
            let (track_id, clip_id, key, options) = args!(a; track_id: u64, clip_id: u64, key: Key, options: FugueOptions);
            let s = fugue::subject_of(st, Id(track_id), Id(clip_id))?;
            let opt = FugueOptions { beats_per_bar: fugue::bpb(st)?, ..options };
            json(klang_music::fugue::plan_fugue(&s.notes, &key, &opt))
        }
        "fugue_add_voices" => {
            let (voices, start_beats, kind, label) = args!(a; voices: Vec<fugue::VoiceIn>, start_beats: f64, kind: InstrumentKind, label: String);
            json(fugue::add_voices(st, &voices, start_beats.max(0.0), kind, &label)?.0)
        }
        "fugue_preview" => {
            let (voices, kind) = args!(a; voices: Vec<Vec<VNote>>, kind: InstrumentKind);
            json(fugue::preview_voices(st, &voices, kind)?)
        }
        "counterpoint_check" => {
            let (track_ids,) = args!(a; track_ids: Option<Vec<u64>>);
            let only: Option<Vec<Id>> = track_ids.map(|v| v.into_iter().map(Id).collect());
            json(fugue::check_tracks(st, only.as_deref())?)
        }

        // ------------------------------------------------ Export
        "export_info" => json(export::info(&export::export_dir(st), st)?),
        "export_run" => {
            let (options,) = args!(a; options: export::ExportOptions);
            json(export::run(&export::export_dir(st), st, &options)?)
        }

        other => Err(tr!("Unknown command: {cmd}", cmd = other)),
    }
}

/// Projekt wurde ersetzt (Öffnen, Neu): Stand melden und zurückgeben.
fn changed(st: &AppState) -> CmdResult<crate::Snapshot> {
    let snap = st.snapshot()?;
    st.host().emit("project-changed", &snap);
    Ok(snap)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_command_is_handled() {
        let st = AppState::without_audio();
        for cmd in COMMANDS {
            // Mit kaputten Angaben antwortet jeder Befehl – aber nie „unbekannt“.
            if let Err(e) = call(&st, cmd, &serde_json::json!({ "kaputt": true })) {
                assert!(!e.starts_with("Unknown command"), "{cmd}: {e}");
            }
        }
        assert!(call(&st, "gibt_es_nicht", &Value::Null).unwrap_err().starts_with("Unknown command"));
    }

    /// Aufwand der Befehlstabelle gegenüber dem direkten Aufruf derselben
    /// Funktionen (wie früher die generierten Tauri-Befehle: Argumente aus
    /// JSON lesen, Antwort als JSON). Nur Messung, kein Grenzwert:
    /// `cargo test --release -p klang-app -- --ignored --nocapture aufwand`
    #[test]
    #[ignore = "Messung"]
    fn aufwand_der_befehlstabelle() {
        use std::time::Instant;
        let st = AppState::without_audio();
        st.create_demo().unwrap();
        let track = st.snapshot().unwrap();
        let track = serde_json::to_value(&track).unwrap()["project"]["tracks"][1]["id"].as_u64().unwrap();
        let n = 2000;
        let median = |mut xs: Vec<f64>| {
            xs.sort_by(f64::total_cmp);
            xs[xs.len() / 2]
        };
        let mut rows = Vec::new();
        for (name, args) in [
            ("get_state", serde_json::json!({})),
            ("execute", serde_json::json!({ "command": { "type": "setTrack", "trackId": track, "patch": { "gainDb": -3.0 } }, "merge": true })),
            ("harmony_describe", serde_json::json!({ "key": { "tonic": 0, "minor": false }, "chords": [{ "root": 0, "quality": "major" }, { "root": 7, "quality": "dom7" }] })),
        ] {
            let mut direct = Vec::new();
            let mut table = Vec::new();
            for _ in 0..5 {
                let t = Instant::now();
                for _ in 0..n {
                    let out = match name {
                        "get_state" => serde_json::to_string(&st.snapshot().unwrap()).unwrap(),
                        "execute" => {
                            let cmd = Command::deserialize(&args["command"]).unwrap();
                            serde_json::to_string(&st.execute(cmd, true).unwrap()).unwrap()
                        }
                        _ => {
                            let key = Key::deserialize(&args["key"]).unwrap();
                            let chords = Vec::<Chord>::deserialize(&args["chords"]).unwrap();
                            serde_json::to_string(&chords.iter().map(|c| klang_music::harmony::describe(c, &key)).collect::<Vec<_>>()).unwrap()
                        }
                    };
                    std::hint::black_box(out);
                }
                direct.push(t.elapsed().as_secs_f64() * 1e6 / n as f64);
                let t = Instant::now();
                for _ in 0..n {
                    std::hint::black_box(call(&st, name, &args).unwrap());
                }
                table.push(t.elapsed().as_secs_f64() * 1e6 / n as f64);
            }
            rows.push((name, median(direct), median(table)));
        }
        println!("Befehl            direkt (µs)   Tabelle (µs)   Unterschied");
        for (name, d, t) in rows {
            println!("{name:<18}{d:>11.2}{t:>15.2}{:>12.2}", t - d);
        }
    }

    #[test]
    fn arguments_come_in_camel_case_like_from_the_ui() {
        let st = AppState::without_audio();
        let snap: Value = serde_json::from_str(&call(&st, "create_instrument_track", &serde_json::json!({ "kind": "pluck" })).unwrap()).unwrap();
        let track = snap["project"]["tracks"][0]["id"].as_u64().unwrap();
        let wave: Value = serde_json::from_str(&call(&st, "sound_wave", &serde_json::json!({ "trackId": track })).unwrap()).unwrap();
        assert!(wave.is_object());
        let err = call(&st, "sound_wave", &serde_json::json!({ "track_id": track })).unwrap_err();
        assert!(err.contains("trackId"), "{err}");
        // fehlende Option-Angaben sind erlaubt
        assert!(call(&st, "harmony_timeline", &Value::Null).is_ok());
    }
}
