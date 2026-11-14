//! Bearbeitungen über alle Spuren hinweg.
//!
//! **Zeitbereich löschen** (Entf bei markiertem Bereich): Der Bereich
//! `[start, end)` verschwindet aus allen Spuren, alles danach rückt um seine
//! Länge nach vorn (Aufrücken, „Ripple-Delete“) – so entsteht keine Lücke.
//! Rechnerisch ist das eine Zeitabbildung, die den Bereich auf einen Punkt
//! zusammenzieht ([`collapse`]); Clips und Noten werden mit ihr abgebildet:
//!
//! * Clips ganz im Bereich fallen weg, Clips dahinter rücken nach vorn.
//! * Ein Noten-Clip, der den Bereich schneidet, wird gekürzt; seine Noten im
//!   Bereich fallen weg, Noten über eine Kante hinweg werden um den Anteil im
//!   Bereich kürzer (ein gehaltener Ton bleibt ein Ton).
//! * Bei einem Audio-Clip bleibt nur das Material außerhalb des Bereichs; das
//!   Ausschneiden selbst erledigt der Aufrufer (`cut`), der die Audiodaten hält.
//!
//! Alles zusammen ist ein Befehl (`Batch`) – also ein Rückgängig-Schritt.

use crate::command::{ClipPatch, Command, CoreError};
use crate::model::{Clip, ClipSource, Note, Project};

const EPS: f64 = 1e-9;
/// Kürzere Noten (in Schlägen) fallen nach dem Zusammenziehen weg.
const MIN_NOTE: f64 = 1.0 / 128.0;

/// Position nach dem Löschen von `[start, end)`.
pub fn collapse(x: f64, start: f64, end: f64) -> f64 {
    if x <= start {
        x
    } else if x >= end {
        x - (end - start)
    } else {
        start
    }
}

/// Teile der Quelle (Sekunden ab Anfang der Audiodatei), die von einem
/// Audio-Clip übrig bleiben sollen – in dieser Reihenfolge aneinandergehängt.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioKeep {
    pub asset_id: String,
    pub parts: Vec<(f64, f64)>,
}

/// Schläge je Sekunde Quellmaterial eines Audio-Clips.
pub fn audio_beats_per_sec(source: &ClipSource, project_bpm: f64) -> f64 {
    match source {
        ClipSource::Audio { source_bpm, warp, .. } => (if *warp { *source_bpm } else { project_bpm }) / 60.0,
        ClipSource::Notes { .. } => project_bpm / 60.0,
    }
}

/// Takt.Schlag einer Position (1-basiert), z. B. „3.1“ oder „2.2,5“.
fn bar_beat(beats: f64, bpb: u32) -> String {
    let bpb = bpb.max(1) as f64;
    let bar = (beats / bpb + EPS).floor();
    let beat = beats - bar * bpb + 1.0;
    let beat = (beat * 1000.0).round() / 1000.0;
    let beat = if (beat - beat.round()).abs() < 1e-6 { format!("{}", beat.round() as i64) } else { format!("{beat}").replace('.', ",") };
    format!("{}.{beat}", bar as i64 + 1)
}

fn map_notes(notes: &[Note], clip_start: f64, start: f64, end: f64) -> Vec<Note> {
    let new_start = collapse(clip_start, start, end);
    notes
        .iter()
        .filter_map(|n| {
            let a = collapse(clip_start + n.start_beats, start, end);
            let b = collapse(clip_start + n.start_beats + n.length_beats, start, end);
            (b - a >= MIN_NOTE).then(|| Note { start_beats: (a - new_start).max(0.0), length_beats: b - a, ..n.clone() })
        })
        .collect()
}

/// Plant das Löschen von `[start, end)` aus allen Spuren mit Aufrücken.
///
/// `cut` schneidet Audio: Es bekommt den Clip und die zu behaltenden Teile der
/// Quelle und liefert die neue Quelle (`ClipSource::Audio` mit neuer Datei und
/// Dauer). Ergebnis ist ein Stapelbefehl; Fehler, wenn nichts betroffen ist.
pub fn delete_range(
    project: &Project,
    start: f64,
    end: f64,
    cut: &mut dyn FnMut(&Clip, &AudioKeep) -> Result<ClipSource, String>,
) -> Result<Command, CoreError> {
    if !start.is_finite() || !end.is_finite() || start < 0.0 || end - start < 1e-6 {
        return Err(CoreError::Invalid("Ungültiger Zeitbereich".into()));
    }
    let len = end - start;
    let bpm = project.tempo_bpm;
    let mut commands = Vec::new();
    for track in &project.tracks {
        for clip in &track.clips {
            let cs = clip.start_beats;
            let ce = cs + clip.length_beats(bpm);
            if ce <= start + EPS {
                continue; // ganz davor
            }
            let (ns, ne) = (collapse(cs, start, end), collapse(ce, start, end));
            if cs >= end - EPS {
                // ganz dahinter: nur aufrücken
                let patch = ClipPatch { start_beats: Some((cs - len).max(0.0)), ..Default::default() };
                commands.push(Command::SetClip { track_id: track.id, clip_id: clip.id, patch });
                continue;
            }
            if ne - ns < 1e-6 {
                commands.push(Command::RemoveClip { track_id: track.id, clip_id: clip.id });
                continue;
            }
            match &clip.source {
                ClipSource::Notes { notes, .. } => {
                    commands.push(Command::SetClipNotes {
                        track_id: track.id,
                        clip_id: clip.id,
                        notes: map_notes(notes, cs, start, end),
                        length_beats: Some(ne - ns),
                    });
                    if (ns - cs).abs() > EPS {
                        let patch = ClipPatch { start_beats: Some(ns), ..Default::default() };
                        commands.push(Command::SetClip { track_id: track.id, clip_id: clip.id, patch });
                    }
                }
                ClipSource::Audio { asset_id, duration_secs, .. } => {
                    let bps = audio_beats_per_sec(&clip.source, bpm);
                    // gelöschter Teil in Sekunden der Quelle
                    let a = ((start.max(cs) - cs) / bps).clamp(0.0, *duration_secs);
                    let b = ((end.min(ce) - cs) / bps).clamp(0.0, *duration_secs);
                    let parts: Vec<(f64, f64)> = [(0.0, a), (b, *duration_secs)].into_iter().filter(|(x, y)| y - x > 1e-4).collect();
                    let keep = AudioKeep { asset_id: asset_id.clone(), parts };
                    let source = cut(clip, &keep).map_err(CoreError::Invalid)?;
                    // derselbe Clip (gleiche ID) mit neuer Quelle an neuer Stelle
                    commands.push(Command::RemoveClip { track_id: track.id, clip_id: clip.id });
                    commands.push(Command::AddClip { track_id: track.id, clip: Clip { start_beats: ns, source, ..clip.clone() } });
                }
            }
        }
    }
    if commands.is_empty() {
        return Err(CoreError::Invalid("Im markierten Bereich und dahinter liegt nichts zum Löschen".into()));
    }
    let bpb = project.beats_per_bar;
    Ok(Command::Batch { label: format!("Zeitbereich löschen (Takt {} – {})", bar_beat(start, bpb), bar_beat(end, bpb)), commands })
}

/// Richtet Noten am Raster aus („am Takt ausrichten“). Gerechnet wird in
/// Projektzeit (`clip_start` + Notenanfang), damit auch ein Clip, der nicht auf
/// einem Taktstrich beginnt, sauber im Takt liegt. Der Anfang springt zum
/// nächsten Rasterpunkt (nie vor den Clipanfang); mit `lengths` auch das Ende,
/// mindestens ein Raster nach dem Anfang. `only`: Indizes der zu bearbeitenden
/// Noten (None = alle).
///
/// Ergebnis: Noten in der Reihenfolge von `SetClipNotes` (Anfang, Tonhöhe) und
/// die neuen Indizes der bearbeiteten Noten. Landen zwei Noten gleicher
/// Tonhöhe auf demselben Anfang, bleibt die längere.
pub fn quantize_notes(notes: &[Note], only: Option<&[usize]>, clip_start: f64, grid: f64, lengths: bool) -> Result<(Vec<Note>, Vec<usize>), CoreError> {
    if !grid.is_finite() || grid < 1.0 / 64.0 - EPS {
        return Err(CoreError::Invalid("Das Raster muss mindestens 1/64 Schlag sein".into()));
    }
    let chosen = |i: usize| only.is_none_or(|o| o.contains(&i));
    let snap = |b: f64| (b / grid).round() * grid;
    let mut out: Vec<(Note, bool)> = notes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            if !chosen(i) {
                return (n.clone(), false);
            }
            let abs = clip_start + n.start_beats;
            let mut s = snap(abs);
            if s < clip_start - EPS {
                s += grid;
            }
            let mut m = n.clone();
            m.start_beats = (s - clip_start).max(0.0);
            if lengths {
                m.length_beats = snap(abs + n.length_beats).max(s + grid) - s;
            }
            (m, true)
        })
        .collect();
    out.sort_by(|(a, _), (b, _)| a.start_beats.total_cmp(&b.start_beats).then(a.pitch.cmp(&b.pitch)));
    let mut merged: Vec<(Note, bool)> = Vec::with_capacity(out.len());
    for (n, sel) in out {
        match merged.last_mut() {
            Some((m, msel)) if m.pitch == n.pitch && (m.start_beats - n.start_beats).abs() < EPS => {
                if n.length_beats > m.length_beats {
                    *m = n;
                }
                *msel |= sel;
            }
            _ => merged.push((n, sel)),
        }
    }
    let selected = merged.iter().enumerate().filter(|(_, (_, s))| *s).map(|(i, _)| i).collect();
    Ok((merged.into_iter().map(|(n, _)| n).collect(), selected))
}

/// Notenwert eines Rasters in Worten: „1/8“ (bei Viertel als Schlag) oder „0,3 Schläge“.
pub fn grid_name(grid: f64, beat_unit: u32) -> String {
    let d = beat_unit as f64 / grid;
    if d >= 1.0 && (d - d.round()).abs() < 1e-6 {
        format!("1/{}", d.round() as i64)
    } else {
        format!("{} Schläge", grid.to_string().replace('.', ","))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Id, Session, Track, TrackKind};

    #[test]
    fn quantize_snaps_starts_and_optionally_lengths() {
        let notes: Vec<Note> = [(60, 0.1, 0.8), (62, 1.15, 0.3), (64, 1.6, 1.1), (65, 3.0, 0.5)].iter().map(|&(p, s, l)| Note::new(p, s, l, 90)).collect();
        // nur die ersten drei, Achtel (0,5), Clip ab Schlag 4
        let (q, sel) = quantize_notes(&notes, Some(&[0, 1, 2]), 4.0, 0.5, false).unwrap();
        let v: Vec<(u8, f64, f64)> = q.iter().map(|n| (n.pitch, n.start_beats, n.length_beats)).collect();
        assert_eq!(v, vec![(60, 0.0, 0.8), (62, 1.0, 0.3), (64, 1.5, 1.1), (65, 3.0, 0.5)]);
        assert_eq!(sel, vec![0, 1, 2]);
        // mit Längen: Ende auf das Raster, mindestens ein Raster
        let (q, _) = quantize_notes(&notes, None, 4.0, 0.5, true).unwrap();
        let v: Vec<(u8, f64, f64)> = q.iter().map(|n| (n.pitch, n.start_beats, n.length_beats)).collect();
        assert_eq!(v, vec![(60, 0.0, 1.0), (62, 1.0, 0.5), (64, 1.5, 1.0), (65, 3.0, 0.5)]);
    }

    #[test]
    fn quantize_uses_project_time_and_never_leaves_the_clip() {
        // Clip beginnt bei 4,2: Note bei 0,1 (Projekt 4,3) → Viertel-Raster 4,0 liegt vor dem Clip → 5,0
        let notes = vec![Note::new(60, 0.1, 0.5, 90), Note::new(60, 0.9, 0.5, 90)];
        let (q, sel) = quantize_notes(&notes, None, 4.2, 1.0, false).unwrap();
        // beide landen auf Projektschlag 5 → zusammengeführt
        assert_eq!(q.len(), 1);
        assert!((q[0].start_beats - 0.8).abs() < 1e-9);
        assert_eq!(sel, vec![0]);
        assert!(quantize_notes(&notes, None, 0.0, 0.0, false).is_err());
        assert_eq!(grid_name(0.25, 4), "1/16");
        assert_eq!(grid_name(1.0, 4), "1/4");
        assert_eq!(grid_name(0.5, 8), "1/16");
        assert_eq!(grid_name(0.3, 4), "0,3 Schläge");
    }

    fn notes_clip(start: f64, len: f64, notes: &[(u8, f64, f64)]) -> Clip {
        Clip {
            id: Id::NEW,
            name: "N".into(),
            start_beats: start,
            source: ClipSource::Notes { length_beats: len, notes: notes.iter().map(|&(p, s, l)| Note::new(p, s, l, 100)).collect() },
        }
    }

    fn audio_clip(start: f64, secs: f64) -> Clip {
        // 120 BPM ohne Warp: 2 Schläge je Sekunde
        Clip { id: Id::NEW, name: "A".into(), start_beats: start, source: ClipSource::Audio { asset_id: "a1".into(), source_bpm: 90.0, warp: false, duration_secs: secs } }
    }

    fn session(clips: Vec<(TrackKind, Vec<Clip>)>) -> Session {
        let mut s = Session::default();
        for (kind, cs) in clips {
            let mut t = Track::new(Id::NEW, "T", kind);
            t.clips = cs;
            s.execute(Command::AddTrack { track: t, index: None }, false).unwrap();
        }
        s
    }

    fn no_audio(_: &Clip, _: &AudioKeep) -> Result<ClipSource, String> {
        Err("kein Audio erwartet".into())
    }

    fn summary(c: &Clip) -> (f64, f64, Vec<(u8, f64, f64)>) {
        match &c.source {
            ClipSource::Notes { length_beats, notes } => (c.start_beats, *length_beats, notes.iter().map(|n| (n.pitch, n.start_beats, n.length_beats)).collect()),
            ClipSource::Audio { duration_secs, .. } => (c.start_beats, *duration_secs, vec![]),
        }
    }

    #[test]
    fn collapse_maps_range_to_a_point() {
        assert_eq!(collapse(1.0, 2.0, 4.0), 1.0);
        assert_eq!(collapse(3.0, 2.0, 4.0), 2.0);
        assert_eq!(collapse(4.0, 2.0, 4.0), 2.0);
        assert_eq!(collapse(7.0, 2.0, 4.0), 5.0);
    }

    #[test]
    fn notes_clips_shrink_and_later_clips_move_up() {
        // Spur 1: Clip 0–8 mit Noten; Spur 2: Clip ganz im Bereich und einer dahinter
        let mut s = session(vec![
            (TrackKind::Instrument, vec![notes_clip(0.0, 8.0, &[(60, 0.0, 1.0), (62, 2.5, 1.0), (64, 3.5, 2.0), (65, 5.0, 1.0), (67, 6.0, 1.0)])]),
            (TrackKind::Instrument, vec![notes_clip(3.0, 1.0, &[(50, 0.0, 1.0)]), notes_clip(8.0, 4.0, &[(52, 1.0, 1.0)])]),
        ]);
        let before = s.project.clone();
        let cmd = delete_range(&s.project, 2.0, 5.0, &mut no_audio).unwrap();
        assert_eq!(cmd.label(), "Zeitbereich löschen (Takt 1.3 – 2.2)");
        s.execute(cmd, false).unwrap();
        let t1 = &s.project.tracks[0].clips;
        // 62 im Bereich: weg; 64 (3,5–5,5) ragt hinaus: bleibt mit dem Rest 0,5 ab Schlag 2;
        // 65 (Start an der Endkante) rückt auf 2; 67 auf 3
        assert_eq!(summary(&t1[0]), (0.0, 5.0, vec![(60, 0.0, 1.0), (64, 2.0, 0.5), (65, 2.0, 1.0), (67, 3.0, 1.0)]));
        let t2 = &s.project.tracks[1].clips;
        assert_eq!(t2.len(), 1, "Clip im Bereich ist weg");
        assert_eq!(summary(&t2[0]), (5.0, 4.0, vec![(52, 1.0, 1.0)]));
        // ein Rückgängig-Schritt (ein gelöschter Clip kommt hinten in die Liste zurück)
        assert!(s.undo().unwrap());
        let sorted = |p: &Project| {
            let mut t = p.tracks.clone();
            t.iter_mut().for_each(|t| t.clips.sort_by_key(|c| c.id));
            t
        };
        assert_eq!(sorted(&s.project), sorted(&before));
    }

    #[test]
    fn held_note_across_the_range_stays_one_note() {
        let mut s = session(vec![(TrackKind::Instrument, vec![notes_clip(0.0, 8.0, &[(60, 1.0, 6.0)])])]);
        s.execute(delete_range(&s.project, 2.0, 4.0, &mut no_audio).unwrap(), false).unwrap();
        assert_eq!(summary(&s.project.tracks[0].clips[0]), (0.0, 6.0, vec![(60, 1.0, 4.0)]));
    }

    #[test]
    fn clip_starting_inside_loses_its_head() {
        let mut s = session(vec![(TrackKind::Instrument, vec![notes_clip(3.0, 4.0, &[(60, 0.0, 1.0), (62, 2.0, 1.0)])])]);
        s.execute(delete_range(&s.project, 2.0, 4.0, &mut no_audio).unwrap(), false).unwrap();
        // Clip 3–7 → 2–5; Note bei 3 (im Bereich) weg, Note bei 5 → 3 (relativ 1)
        assert_eq!(summary(&s.project.tracks[0].clips[0]), (2.0, 3.0, vec![(62, 1.0, 1.0)]));
    }

    #[test]
    fn audio_clip_keeps_material_outside_the_range() {
        // Audio 4 s ab Schlag 1 → Schläge 1–9 (120 BPM, ohne Warp)
        let mut s = session(vec![(TrackKind::Audio, vec![audio_clip(1.0, 4.0), audio_clip(10.0, 1.0)])]);
        let first_id = s.project.tracks[0].clips[0].id;
        let mut seen = Vec::new();
        let mut cut = |c: &Clip, k: &AudioKeep| {
            seen.push((c.id, k.clone()));
            let dur: f64 = k.parts.iter().map(|(a, b)| b - a).sum();
            Ok(ClipSource::Audio { asset_id: "neu".into(), source_bpm: 90.0, warp: false, duration_secs: dur })
        };
        let cmd = delete_range(&s.project, 3.0, 5.0, &mut cut).unwrap();
        s.execute(cmd, false).unwrap();
        assert_eq!(seen, vec![(first_id, AudioKeep { asset_id: "a1".into(), parts: vec![(0.0, 1.0), (2.0, 4.0)] })]);
        let clips = &s.project.tracks[0].clips;
        let first = clips.iter().find(|c| c.id == first_id).expect("gleiche ID");
        assert_eq!(summary(first), (1.0, 3.0, vec![]));
        assert!(matches!(&first.source, ClipSource::Audio { asset_id, .. } if asset_id == "neu"));
        let second = clips.iter().find(|c| c.id != first_id).unwrap();
        assert_eq!(second.start_beats, 8.0, "rückt um 2 Schläge auf");
        s.undo().unwrap();
        let back = s.project.tracks[0].clips.iter().find(|c| c.id == first_id).unwrap();
        assert!(matches!(&back.source, ClipSource::Audio { asset_id, duration_secs, .. } if asset_id == "a1" && *duration_secs == 4.0));
    }

    #[test]
    fn warped_audio_uses_its_source_tempo() {
        let clip = Clip { id: Id::NEW, name: "W".into(), start_beats: 0.0, source: ClipSource::Audio { asset_id: "w".into(), source_bpm: 60.0, warp: true, duration_secs: 8.0 } };
        let s = session(vec![(TrackKind::Audio, vec![clip])]);
        let mut parts = None;
        let mut cut = |_: &Clip, k: &AudioKeep| {
            parts = Some(k.parts.clone());
            Ok(ClipSource::Audio { asset_id: "x".into(), source_bpm: 60.0, warp: true, duration_secs: 6.0 })
        };
        // 60 BPM Quelle: ein Schlag = eine Sekunde
        delete_range(&s.project, 2.0, 4.0, &mut cut).unwrap();
        assert_eq!(parts, Some(vec![(0.0, 2.0), (4.0, 8.0)]));
    }

    #[test]
    fn nothing_to_delete_is_an_error() {
        let s = session(vec![(TrackKind::Instrument, vec![notes_clip(0.0, 2.0, &[(60, 0.0, 1.0)])])]);
        assert!(delete_range(&s.project, 4.0, 8.0, &mut no_audio).is_err());
        assert!(delete_range(&s.project, 3.0, 3.0, &mut no_audio).is_err());
    }
}
