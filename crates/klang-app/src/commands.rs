//! The UI's command table – one for all platforms.
//!
//! A command is a name plus arguments as a JSON object (keys in camelCase, as
//! the UI sends them); the reply is JSON.
//!
//! * Desktop: Tauri forwards every call whose name is listed here to [`call`]
//!   (`app/src/lib.rs`).
//! * Browser: the web core calls [`call`] directly (`crates/klang-web`).
//!
//! The UI (`ui/src/bridge.ts`) uses the same mapping on both platforms. Commands
//! that exist on one platform only (close window, "Show in folder", download
//! libraries …) are handled by the respective shell itself.

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

/// All commands of the core.
pub const COMMANDS: &[&str] = &[
    "set_language",
    "get_state",
    "execute",
    "undo",
    "redo",
    "transport",
    "seek",
    "set_metronome",
    "simulate_load",
    "set_loop",
    "set_loop_range",
    "audio_devices",
    "audio_set_device",
    "record_start",
    "record_stop",
    "record_live",
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
    "chat_stop",
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

/// Does the core have this command?
pub fn has(cmd: &str) -> bool {
    COMMANDS.contains(&cmd)
}

/// Reads the arguments as a tuple: `let (a, b) = args!(v; a: u64, b: Option<f64>);`
/// – missing `Option` fields are `None`, surplus ones are ignored.
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

/// Runs a command. `args` is the UI's JSON object (or `null`); the reply is
/// JSON, an error is a text for the UI.
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
        // ------------------------------------------------ Project, undo, transport
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
        // for tests: the live mixer counts as at least this loaded (pre-rendering kicks in)
        "simulate_load" => {
            let (load,) = args!(a; load: f32);
            st.simulate_load(load);
            json(())
        }
        // Repeat on/off (at the end of the piece or the selected range, start over)
        "set_loop" => {
            let (enabled,) = args!(a; enabled: bool);
            json(st.set_loop(Some(enabled), None)?)
        }
        // Select a time range (beats), or clear it with `null`
        "set_loop_range" => {
            let (range,) = args!(a; range: Option<(f64, f64)>);
            json(st.set_loop(None, Some(range))?)
        }
        // Output and recording devices with the current choice (browser: only the system's device)
        "audio_devices" => json(st.audio_devices()?),
        // Choose an audio device (kind: output/input; name null = system default) – takes effect immediately and applies at the next start
        "audio_set_device" => {
            let (kind, name) = args!(a; kind: String, name: Option<String>);
            json(st.set_audio_device(&kind, name)?)
        }

        // ------------------------------------------------ Recording, analysis
        "record_start" => json(st.record_start()?),
        "record_stop" => json(st.record_stop()?),
        // What the running recording's live analysis found since the cursor (null: no recording running)
        "record_live" => {
            let (since,) = args!(a; since: Option<crate::analysis::LiveCursor>);
            json(st.record_live(since.unwrap_or_default())?)
        }
        "get_waveform" => {
            let (asset_id, buckets) = args!(a; asset_id: String, buckets: usize);
            json(st.waveform(&asset_id, buckets)?)
        }
        "analyze_asset" => {
            let (asset_id,) = args!(a; asset_id: String);
            json(st.analysis_for(&asset_id)?)
        }
        // Detected melody of an audio clip as an instrument track (quantized to sixteenth notes)
        "create_melody_track" => {
            let (track_id, clip_id) = args!(a; track_id: u64, clip_id: u64);
            json(st.melody_to_track(Id(track_id), Id(clip_id), &tr!("Import melody"))?.0)
        }
        // Insert audio files (drag & drop) as tracks – read through the core's file system
        // (desktop: the disk; browser: dropped files under /Klangwelt/Import)
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
        // Convert a recording into a melody or rhythm track (without clipId: all audio clips of the track)
        "audio_to_notes" => {
            let (track_id, clip_id, target) = args!(a; track_id: u64, clip_id: Option<u64>, target: crate::convert::Target);
            let label = match target {
                crate::convert::Target::Melodie => tr!("Convert to melody track"),
                crate::convert::Target::Rhythmus => tr!("Convert to rhythm track"),
            };
            json(st.audio_to_notes(Id(track_id), clip_id.map(Id), target, &label)?)
        }
        // Delete a time range from all tracks, everything after it moves up
        "delete_range" => {
            let (start_beats, end_beats) = args!(a; start_beats: f64, end_beats: f64);
            json(st.delete_range(start_beats, end_beats)?)
        }
        // Align notes to the grid (without indices: all notes of the clip)
        "quantize_notes" => {
            let (track_id, clip_id, indices, grid, lengths) = args!(a; track_id: u64, clip_id: u64, indices: Option<Vec<usize>>, grid: f64, lengths: bool);
            json(st.quantize_notes(Id(track_id), Id(clip_id), indices, grid, lengths, None)?)
        }
        "create_demo" => json(st.create_demo()?),

        // ------------------------------------------------ Sound, preview
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
        // Set a point, or the starting point, in a field of the sound window
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

        // ------------------------------------------------ Chat with the agent
        "chat_send" => {
            let (text,) = args!(a; text: String);
            // Pass along the situation in the project: which tracks are sounding, which range is selected
            st.chat.send_user(&text, st.chat_context().ok())?;
            json(st.chat_view())
        }
        // Stop button at a task: cancelled if nobody picked it up yet, otherwise
        // every tool response carries cancelRequested until the agent finishes it
        "chat_stop" => {
            let (id, text) = args!(a; id: u64, text: String);
            st.chat.stop_task(id, &text)?;
            json(st.chat_view())
        }
        "chat_history" => json(st.chat_view()),

        // ------------------------------------------------ Project files
        // Without `path` into the most recently used file; a bare name ends up in the project folder.
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
        // [project folder, projects in it (newest first)]
        "project_list" => {
            let dir = st.host().projects_dir();
            json((dir.display().to_string(), project::list(&dir)))
        }

        // ------------------------------------------------ Library (queries)
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

        // ------------------------------------------------ Song, rhythm
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

        // ------------------------------------------------ Harmony
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
        // `mode` "einfuegen" makes room (everything after it moves back), otherwise it overwrites
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

        // ------------------------------------------------ Fugue, counterpoint
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

/// Project was replaced (open, new): report the state and return it.
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
            // Every command replies to malformed arguments – but never with "unknown".
            if let Err(e) = call(&st, cmd, &serde_json::json!({ "kaputt": true })) {
                assert!(!e.starts_with("Unknown command"), "{cmd}: {e}");
            }
        }
        assert!(call(&st, "gibt_es_nicht", &Value::Null).unwrap_err().starts_with("Unknown command"));
    }

    /// Overhead of the command table compared with calling the same functions
    /// directly (like the generated Tauri commands used to: read arguments from
    /// JSON, reply as JSON). Measurement only, no limit:
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
        // missing Option arguments are allowed
        assert!(call(&st, "harmony_timeline", &Value::Null).is_ok());
    }
}
