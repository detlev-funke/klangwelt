//! Saving in the background and the recovery copy. The timer runs in the UI
//! (`ui/src/autosave.ts`: every minute by default, never while playing or
//! recording); this module decides where the project goes.
//!
//! * A project with a file is saved into it with [`AppState::save_project`] –
//!   the same atomic write as "Save" (temporary file, then rename). Earlier
//!   states stay reachable through the change history.
//! * A project without a file goes into the recovery copy next to the settings
//!   file (desktop: the app's local data folder, browser: `/Klangwelt` in
//!   OPFS) – before a reload with autosave off, a project with a file does too,
//!   so its file stays untouched. The project stays unsaved. [`recover`] opens
//!   the copy again: after a language switch (the browser's core starts anew
//!   with the page) or when the app didn't close properly. A small JSON next to
//!   it remembers the project's file and its state there, so "unsaved changes"
//!   stays as it was.
//! * Without changes an old recovery copy is removed, and so it is when a
//!   project is saved, opened or created (`commands.rs`) and when the desktop
//!   app closes normally (its shell calls [`clear_recovery`]).

use crate::state::{lock, project_hash, AppState, CmdResult};
use crate::{project, vfs};
use klang_core::Project;
use klang_i18n::tr;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

/// The recovery copy (a normal project file) and what it belongs to.
pub const RECOVERY_FILE: &str = "Wiederherstellung.klangwelt";
const RECOVERY_INFO: &str = "Wiederherstellung.json";

static WRITING: Mutex<()> = Mutex::new(());

/// Held while a project file is written: autosave and "Save" may come at the
/// same time (on the desktop each on a thread of its own) and must not write
/// through the same temporary file together.
pub fn writing() -> MutexGuard<'static, ()> {
    WRITING.lock().unwrap_or_else(|e| e.into_inner())
}

/// What the recovery copy belongs to.
#[derive(Serialize, Deserialize, Default, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
struct RecoveryInfo {
    /// The project's file (None: never saved).
    file: Option<String>,
    /// Checksum of the project as it is in that file – "unsaved changes" after recovering.
    saved_hash: u64,
    /// Checksum of the copy: an unchanged project isn't written again.
    hash: u64,
}

/// Answer of `project_autosave`.
#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AutosaveResult {
    /// "file", "recovery", or None (no changes, or no place for a recovery copy).
    pub saved: Option<&'static str>,
    /// The project's file afterwards.
    pub file: Option<String>,
    /// Changes not yet in the project's file.
    pub dirty: bool,
}

/// Recovery copy and its JSON – None before the platform set the settings file.
fn recovery_paths(st: &AppState) -> CmdResult<Option<(PathBuf, PathBuf)>> {
    let dir = lock(&st.settings_file)?.as_ref().and_then(|f| f.parent().map(Path::to_path_buf));
    Ok(dir.map(|d| (d.join(RECOVERY_FILE), d.join(RECOVERY_INFO))))
}

fn read_info(path: &Path) -> Option<RecoveryInfo> {
    serde_json::from_slice(&vfs::read(path).ok()?).ok()
}

/// Autosave (see above). `recovery_only`: a project with a file goes into the
/// recovery copy as well, its file stays as it is.
pub fn autosave(st: &AppState, recovery_only: bool) -> CmdResult<AutosaveResult> {
    let _w = writing();
    let file = lock(&st.project_file)?.clone();
    let saved = if !st.is_dirty() {
        clear_recovery(st);
        None
    } else {
        match &file {
            Some(f) if !recovery_only => {
                st.save_project(f)?;
                clear_recovery(st);
                Some("file")
            }
            _ => write_recovery(st, file.as_deref())?.then_some("recovery"),
        }
    };
    Ok(AutosaveResult { saved, file: file.map(|p| p.display().to_string()), dirty: st.is_dirty() })
}

/// Writes the open project into the recovery copy (unless it is there already).
/// False: no place for it.
fn write_recovery(st: &AppState, file: Option<&Path>) -> CmdResult<bool> {
    let Some((copy, info_path)) = recovery_paths(st)? else { return Ok(false) };
    let (project, hash) = {
        let s = lock(&st.session)?;
        (s.project.clone(), project_hash(&s.project))
    };
    let info = RecoveryInfo { file: file.map(|p| p.display().to_string()), saved_hash: *lock(&st.saved_hash)?, hash };
    if vfs::exists(&copy) && read_info(&info_path).as_ref() == Some(&info) {
        return Ok(true);
    }
    let audio: Vec<project::AudioData> = {
        let pool = lock(&st.pool)?;
        project::used_assets(&project)
            .into_iter()
            .filter_map(|id| pool.get(&id).map(|a| project::AudioData { name: a.name.clone(), samples: a.samples.to_vec(), sample_rate: a.sample_rate, id }))
            .collect()
    };
    let failed = |e: std::io::Error| tr!("Saving to {path} failed: {error}", path = copy.display(), error = e);
    project::save(&copy, &project, &audio).map_err(failed)?;
    let json = serde_json::to_vec(&info).map_err(|e| e.to_string())?;
    vfs::write_atomic(&info_path, &json, "json.teil").map_err(failed)?;
    Ok(true)
}

/// Opens the recovery copy as unsaved changes – of its project's file if it
/// still exists, otherwise as a project without a file. False: there is none.
pub fn recover(st: &AppState) -> CmdResult<bool> {
    let Some((copy, info_path)) = recovery_paths(st)? else { return Ok(false) };
    if !vfs::exists(&copy) {
        return Ok(false);
    }
    let info = read_info(&info_path).unwrap_or_default();
    st.open_project(&copy)?;
    let file = info.file.map(PathBuf::from).filter(|f| vfs::exists(f));
    let saved = if file.is_some() { info.saved_hash } else { project_hash(&Project::default()) };
    *lock(&st.saved_hash)? = saved;
    *lock(&st.project_file)? = file;
    Ok(true)
}

/// Removes the recovery copy (if there is one).
pub fn clear_recovery(st: &AppState) {
    if let Ok(Some((copy, info))) = recovery_paths(st) {
        for p in [copy, info] {
            if vfs::exists(&p) {
                let _ = vfs::remove_file(&p);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use klang_core::Command;

    fn app(dir: &Path) -> AppState {
        let st = AppState::without_audio();
        *st.settings_file.lock().unwrap() = Some(dir.join(crate::settings::FILE));
        st
    }

    fn add_track(st: &AppState, name: &str) {
        let cmd: Command = serde_json::from_value(serde_json::json!({ "type": "addTrack", "track": { "id": 0, "name": name, "kind": "instrument" } })).unwrap();
        st.execute(cmd, false).unwrap();
    }

    fn names(st: &AppState) -> Vec<String> {
        lock(&st.session).unwrap().project.tracks.iter().map(|t| t.name.clone()).collect()
    }

    /// The project's file and "unsaved changes".
    fn state(st: &AppState) -> (Option<PathBuf>, bool) {
        (lock(&st.project_file).unwrap().clone(), st.is_dirty())
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("klang-autosave-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_project_with_a_file_is_saved_into_it_only_when_it_changed() {
        let dir = temp("file");
        let st = app(&dir);
        let file = dir.join("Lied.klangwelt");
        add_track(&st, "Eins");
        st.save_project(&file).unwrap();
        let unchanged = autosave(&st, false).unwrap();
        assert_eq!(unchanged, AutosaveResult { saved: None, file: Some(file.display().to_string()), dirty: false });
        add_track(&st, "Zwei");
        assert!(st.is_dirty());
        let r = autosave(&st, false).unwrap();
        assert_eq!((r.saved, r.dirty), (Some("file"), false));
        let (saved, _) = project::load(&file).unwrap();
        assert_eq!(saved.tracks.len(), 2, "the file has the change");
        assert!(!dir.join(RECOVERY_FILE).exists(), "no recovery copy for a project with a file");
        assert!(!dir.join("Lied.klangwelt.teil").exists(), "no temporary file left");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_project_without_a_file_goes_into_the_recovery_copy_and_comes_back_unsaved() {
        let dir = temp("untitled");
        let st = app(&dir);
        assert_eq!(autosave(&st, false).unwrap().saved, None, "an empty new project needs nothing");
        assert!(!recover(&st).unwrap(), "nothing to recover");
        add_track(&st, "Skizze");
        let r = autosave(&st, false).unwrap();
        assert_eq!(r, AutosaveResult { saved: Some("recovery"), file: None, dirty: true });
        assert!(dir.join(RECOVERY_FILE).exists());
        let written = std::fs::metadata(dir.join(RECOVERY_FILE)).unwrap().modified().unwrap();
        // unchanged: the copy isn't written again
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert_eq!(autosave(&st, false).unwrap().saved, Some("recovery"));
        assert_eq!(std::fs::metadata(dir.join(RECOVERY_FILE)).unwrap().modified().unwrap(), written);

        // a new start (the browser's core after a reload): the project comes back, still without a file and unsaved
        let fresh = app(&dir);
        assert!(recover(&fresh).unwrap());
        assert_eq!(state(&fresh), (None, true), "without a file and unsaved");
        assert_eq!(names(&fresh), ["Skizze"]);
        assert_eq!(lock(&fresh.session).unwrap().project.name, Project::default().name, "the copy's file name doesn't name the project");

        // undone to the empty project: the copy is removed
        fresh.new_project().unwrap();
        autosave(&fresh, false).unwrap();
        assert!(!dir.join(RECOVERY_FILE).exists());
        assert!(!recover(&fresh).unwrap());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn recovery_only_keeps_the_file_untouched_and_recovers_its_unsaved_changes() {
        let dir = temp("keep");
        let st = app(&dir);
        let file = dir.join("Song.klangwelt");
        add_track(&st, "Gespeichert");
        st.save_project(&file).unwrap();
        add_track(&st, "Neu");
        let r = autosave(&st, true).unwrap();
        assert_eq!(r, AutosaveResult { saved: Some("recovery"), file: Some(file.display().to_string()), dirty: true });
        assert_eq!(project::load(&file).unwrap().0.tracks.len(), 1, "the file is as it was");

        let fresh = app(&dir);
        assert!(recover(&fresh).unwrap());
        assert_eq!(state(&fresh), (Some(file.clone()), true), "the file it belongs to, with unsaved changes");
        assert_eq!(names(&fresh), ["Gespeichert", "Neu"]);
        // saving it now goes into its file; the autosave then removes the copy
        let r = autosave(&fresh, false).unwrap();
        assert_eq!((r.saved, r.dirty), (Some("file"), false));
        assert!(!dir.join(RECOVERY_FILE).exists());
        assert_eq!(project::load(&file).unwrap().0.tracks.len(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_recovery_copy_whose_file_is_gone_comes_back_without_a_file() {
        let dir = temp("gone");
        let st = app(&dir);
        let file = dir.join("Weg.klangwelt");
        add_track(&st, "A");
        st.save_project(&file).unwrap();
        add_track(&st, "B");
        autosave(&st, true).unwrap();
        std::fs::remove_file(&file).unwrap();
        let fresh = app(&dir);
        assert!(recover(&fresh).unwrap());
        assert_eq!(state(&fresh), (None, true));
        let _ = std::fs::remove_dir_all(dir);
    }
}
