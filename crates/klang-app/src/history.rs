//! The change history for the UI: the steps of the active variant as a
//! table with key figures, checkpoints, variants and the branches that leave
//! the line. Undo, redo, jumping and variants run in the core
//! ([`klang_core::Session`]); this module only puts them into words.

use crate::state::{lock, AppState, CmdResult, Snapshot};
use klang_core::{History, Metrics, NodeId, Scope};
use klang_i18n::{tr, translate, trn};
use klang_music::theory::estimate_key_from_histogram;
use serde::Serialize;

/// Key figures of one step for the table.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MetricsDto {
    pub notes: u32,
    pub tracks: u32,
    pub clips: u32,
    pub audio_clips: u32,
    pub length_beats: f64,
    pub length_secs: f64,
    pub tempo: f64,
    pub beats_per_bar: u32,
    pub beat_unit: u32,
    /// Estimated key ("A minor") – only with enough notes to tell.
    pub key: Option<String>,
    /// Lowest and highest note (MIDI).
    pub pitch_range: Option<(u8, u8)>,
}

/// The key the notes suggest, if they are clear enough.
pub fn key_of(m: &Metrics) -> Option<String> {
    let hist: [f64; 12] = std::array::from_fn(|i| m.pitch_classes[i] as f64);
    if hist.iter().sum::<f64>() < 4.0 {
        return None;
    }
    estimate_key_from_histogram(&hist).filter(|k| k.confidence >= 0.5).map(|k| k.name())
}

impl From<&Metrics> for MetricsDto {
    fn from(m: &Metrics) -> Self {
        MetricsDto {
            notes: m.notes,
            tracks: m.tracks,
            clips: m.clips,
            audio_clips: m.audio_clips,
            length_beats: m.length_beats,
            length_secs: (m.length_secs() * 100.0).round() / 100.0,
            tempo: m.tempo_bpm,
            beats_per_bar: m.beats_per_bar,
            beat_unit: m.beat_unit,
            key: key_of(m),
            pitch_range: m.pitch_range,
        }
    }
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct NamedRef {
    pub id: u32,
    pub name: String,
}

/// A branch that leaves the line at a row.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ForkDto {
    /// First step of the branch.
    pub node: NodeId,
    /// Its newest step.
    pub tip: NodeId,
    pub steps: usize,
    /// The variant that continues there (none: abandoned steps after an undo).
    pub variant: Option<NamedRef>,
}

/// One row of the table: a step on the active variant's line.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RowDto {
    pub id: NodeId,
    /// Position on the line (0 = the first state).
    pub index: usize,
    pub label: String,
    /// Where: tracks, bars, notes, tempo … (in words).
    pub detail: String,
    pub time: u64,
    pub metrics: MetricsDto,
    /// After the current step – can be redone.
    pub future: bool,
    pub current: bool,
    pub checkpoints: Vec<NamedRef>,
    pub forks: Vec<ForkDto>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct VariantDto {
    pub id: u32,
    pub name: String,
    pub head: NodeId,
    pub active: bool,
    /// Where it leaves the other variants.
    pub fork: NodeId,
    /// Its own steps since then.
    pub steps: usize,
    pub time: u64,
    /// Label of its newest step.
    pub last: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointDto {
    pub id: u32,
    pub name: String,
    pub node: NodeId,
    pub time: u64,
    /// On the active variant's line (otherwise on another variant or an abandoned branch).
    pub on_line: bool,
    /// Variant it belongs to, if it is not on the active line.
    pub variant: Option<String>,
}

/// Everything the history window shows.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct HistoryView {
    pub rows: Vec<RowDto>,
    pub current: NodeId,
    pub active: u32,
    pub variants: Vec<VariantDto>,
    pub checkpoints: Vec<CheckpointDto>,
    /// Steps in the whole tree (with other variants and abandoned branches).
    pub total: usize,
    pub limit: usize,
}

fn bars_text(range: (f64, f64), bpb: u32) -> String {
    let bpb = bpb.max(1) as f64;
    let from = (range.0 / bpb + 1e-9).floor() as i64 + 1;
    let to = ((range.1 / bpb) - 1e-9).ceil().max(from as f64) as i64;
    if from == to { tr!("bar {n}", n = from) } else { tr!("bars {from}–{to}", from = from, to = to) }
}

fn fmt_num(v: f64) -> String {
    let r = (v * 100.0).round() / 100.0;
    if r.fract() == 0.0 { format!("{r:.0}") } else { format!("{r}") }
}

/// Where a step changed something, in words: "Lead, Bass · bars 5–8 · +3 notes".
pub fn detail(scope: &Scope, before: Option<&Metrics>, after: &Metrics) -> String {
    let mut parts = Vec::new();
    if !scope.tracks.is_empty() {
        let names: Vec<&str> = scope.tracks.iter().map(|t| t.name.as_str()).collect();
        parts.push(if names.len() > 3 {
            tr!("{names} +{n}", names = names[..2].join(", "), n = names.len() - 2)
        } else {
            names.join(", ")
        });
    }
    if let Some(r) = scope.range {
        parts.push(bars_text(r, after.beats_per_bar));
    }
    if let Some(b) = before {
        if b.tempo_bpm != after.tempo_bpm {
            parts.push(tr!("{from} → {to} BPM", from = fmt_num(b.tempo_bpm), to = fmt_num(after.tempo_bpm)));
        }
        if (b.beats_per_bar, b.beat_unit) != (after.beats_per_bar, after.beat_unit) {
            parts.push(format!("{}/{} → {}/{}", b.beats_per_bar, b.beat_unit, after.beats_per_bar, after.beat_unit));
        }
    }
    if scope.sound {
        parts.push(tr!("sound"));
    }
    if scope.notes_added > 0 {
        parts.push(trn!(scope.notes_added, "+{n} note", "+{n} notes"));
    }
    if scope.notes_removed > 0 {
        parts.push(trn!(scope.notes_removed, "−{n} note", "−{n} notes"));
    }
    parts.join(" · ")
}

/// Steps on the branch from `start` to its tip.
fn branch_len(h: &History, start: NodeId) -> (NodeId, usize) {
    let tip = h.tip(start);
    (tip, h.depth(tip) + 1 - h.depth(start))
}

pub fn view(h: &History) -> HistoryView {
    let line = h.line();
    let cur = h.current();
    let active = h.active().id;
    let on_line: std::collections::BTreeSet<NodeId> = line.iter().copied().collect();
    let mut rows = Vec::with_capacity(line.len());
    let mut past = true;
    for (i, &id) in line.iter().enumerate() {
        let n = h.node(id).expect("node on the line");
        let before = n.parent.and_then(|p| h.node(p)).map(|p| &p.metrics);
        let forks = n
            .children
            .iter()
            .filter(|c| !on_line.contains(c))
            .map(|&c| {
                let (tip, steps) = branch_len(h, c);
                let variant = h
                    .variants()
                    .iter()
                    .filter(|v| v.id != active && h.is_ancestor(c, v.head))
                    .min_by_key(|v| h.depth(v.head))
                    .map(|v| NamedRef { id: v.id, name: translate(&v.name) });
                ForkDto { node: c, tip, steps, variant }
            })
            .collect();
        rows.push(RowDto {
            id,
            index: i,
            label: translate(&n.label),
            detail: detail(&n.scope, before, &n.metrics),
            time: n.time,
            metrics: (&n.metrics).into(),
            future: !past,
            current: id == cur,
            checkpoints: h.checkpoints().iter().filter(|c| c.node == id).map(|c| NamedRef { id: c.id, name: c.name.clone() }).collect(),
            forks,
        });
        if id == cur {
            past = false;
        }
    }
    let variants = h
        .variants()
        .iter()
        .map(|v| {
            let fork = h.fork_of(v.id);
            VariantDto {
                id: v.id,
                name: translate(&v.name),
                head: v.head,
                active: v.id == active,
                fork,
                steps: h.depth(v.head) - h.depth(fork),
                time: v.time,
                last: h.node(v.head).map(|n| translate(&n.label)).unwrap_or_default(),
            }
        })
        .collect();
    let checkpoints = h
        .checkpoints()
        .iter()
        .map(|c| {
            let on = on_line.contains(&c.node);
            CheckpointDto {
                id: c.id,
                name: c.name.clone(),
                node: c.node,
                time: c.time,
                on_line: on,
                variant: (!on).then(|| h.variants_through(c.node).first().map(|v| translate(&v.name))).flatten(),
            }
        })
        .collect();
    HistoryView { rows, current: cur, active, variants, checkpoints, total: h.len(), limit: klang_core::history::HISTORY_LIMIT }
}

impl AppState {
    pub fn history_view(&self) -> CmdResult<HistoryView> {
        Ok(view(lock(&self.session)?.history()))
    }

    /// Runs a change of the history that moves the project, then brings the
    /// engine up to date.
    fn history_move(&self, f: impl FnOnce(&mut klang_core::Session) -> Result<(), klang_core::CoreError>) -> CmdResult<Snapshot> {
        f(&mut *lock(&self.session)?).map_err(|e| e.to_string())?;
        self.sync_engine()?;
        self.snapshot()
    }

    /// Jumps to the state after step `node`.
    pub fn history_goto(&self, node: NodeId) -> CmdResult<Snapshot> {
        self.history_move(|s| s.goto(node))
    }

    pub fn history_checkpoint(&self, name: Option<String>) -> CmdResult<HistoryView> {
        let mut s = lock(&self.session)?;
        s.add_checkpoint(name).map_err(|e| e.to_string())?;
        Ok(view(s.history()))
    }

    pub fn history_checkpoint_rename(&self, id: u32, name: &str) -> CmdResult<HistoryView> {
        let mut s = lock(&self.session)?;
        s.rename_checkpoint(id, name).map_err(|e| e.to_string())?;
        Ok(view(s.history()))
    }

    pub fn history_checkpoint_delete(&self, id: u32) -> CmdResult<HistoryView> {
        let mut s = lock(&self.session)?;
        s.remove_checkpoint(id).map_err(|e| e.to_string())?;
        Ok(view(s.history()))
    }

    /// New variant from step `node` (default: the current one).
    pub fn history_variant_new(&self, name: Option<String>, node: Option<NodeId>) -> CmdResult<Snapshot> {
        self.history_move(|s| s.create_variant(name, node).map(|_| ()))
    }

    pub fn history_variant_switch(&self, id: u32) -> CmdResult<Snapshot> {
        self.history_move(|s| s.switch_variant(id))
    }

    pub fn history_variant_rename(&self, id: u32, name: &str) -> CmdResult<HistoryView> {
        let mut s = lock(&self.session)?;
        s.rename_variant(id, name).map_err(|e| e.to_string())?;
        Ok(view(s.history()))
    }

    pub fn history_variant_delete(&self, id: u32) -> CmdResult<Snapshot> {
        self.history_move(|s| s.delete_variant(id))
    }

    /// Forgets all steps; the project stays as it is.
    pub fn history_clear(&self) -> CmdResult<Snapshot> {
        self.history_move(|s| {
            s.clear_history(klang_i18n::tk!("History cleared").into());
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use klang_core::{Command, Id, Note, Track, TrackKind};

    fn run(st: &AppState, v: serde_json::Value) {
        st.execute(serde_json::from_value(v).unwrap(), false).unwrap();
    }

    #[test]
    fn table_rows_with_metrics_detail_and_variants() {
        let st = AppState::without_audio();
        let v = st.history_view().unwrap();
        assert_eq!(v.rows.len(), 1);
        assert_eq!(v.rows[0].label, "New project");
        assert!(v.rows[0].current);
        run(&st, serde_json::json!({ "type": "addTrack", "track": { "id": 0, "name": "Lead", "kind": "instrument" } }));
        let tid = lock(&st.session).unwrap().project.tracks[0].id;
        let notes: Vec<Note> = [60, 64, 67, 72, 69, 65, 62, 60].iter().enumerate().map(|(i, &p)| Note::new(p, i as f64, 1.0, 90)).collect();
        st.execute(Command::AddClip { track_id: tid, clip: klang_core::Clip { id: Id::NEW, name: "A".into(), start_beats: 8.0, source: klang_core::ClipSource::Notes { length_beats: 8.0, notes } } }, false).unwrap();
        run(&st, serde_json::json!({ "type": "setProject", "patch": { "tempoBpm": 90 } }));
        let v = st.history_view().unwrap();
        assert_eq!(v.rows.iter().map(|r| r.label.as_str()).collect::<Vec<_>>(), ["New project", "Add track", "Add clip", "Change tempo"]);
        let clip = &v.rows[2];
        assert_eq!(clip.detail, "Lead · bars 3–4 · +8 notes");
        assert_eq!((clip.metrics.notes, clip.metrics.length_beats, clip.metrics.length_secs), (8, 16.0, 8.0));
        assert_eq!(clip.metrics.key.as_deref(), Some("C major"));
        assert_eq!(v.rows[3].detail, "120 → 90 BPM");
        assert_eq!(v.rows[3].metrics.length_secs, 10.67);
        // jump back: the later steps are future, a variant from there leaves them as a fork
        st.history_goto(v.rows[1].id).unwrap();
        let v = st.history_view().unwrap();
        assert!(v.rows[1].current && v.rows[2].future && v.rows[3].future);
        st.history_variant_new(Some("Ohne Melodie".into()), None).unwrap();
        run(&st, serde_json::json!({ "type": "setTrack", "trackId": tid, "patch": { "gainDb": -6 } }));
        let v = st.history_view().unwrap();
        assert_eq!(v.rows.len(), 3);
        assert_eq!(v.variants.iter().map(|x| (x.name.as_str(), x.active, x.steps)).collect::<Vec<_>>(), [("Main version", false, 2), ("Ohne Melodie", true, 1)]);
        let fork = &v.rows[1].forks[0];
        assert_eq!((fork.steps, fork.variant.as_ref().map(|x| x.name.as_str())), (2, Some("Main version")));
        // checkpoint, then the variant comes back after saving and opening
        st.history_checkpoint(Some("Leiser".into())).unwrap();
        let dir = std::env::temp_dir().join(format!("klang-historie-{}", std::process::id()));
        let path = dir.join("Lied.klangwelt");
        st.save_project(&path).unwrap();
        let other = AppState::without_audio();
        other.open_project(&path).unwrap();
        let back = other.history_view().unwrap();
        assert_eq!(back.rows.iter().map(|r| &r.label).collect::<Vec<_>>(), v.rows.iter().map(|r| &r.label).collect::<Vec<_>>());
        assert_eq!(back.variants.len(), 2);
        assert_eq!(back.checkpoints[0].name, "Leiser");
        assert_eq!(back.rows[2].checkpoints[0].name, "Leiser");
        let main = back.variants[0].id;
        other.history_variant_switch(main).unwrap();
        let project = lock(&other.session).unwrap().project.clone();
        assert_eq!(project.tempo_bpm, 90.0);
        // a file of an older version (without history) opens with a fresh history
        crate::project::save(&path, &project, &[]).unwrap();
        other.open_project(&path).unwrap();
        let fresh = other.history_view().unwrap();
        assert_eq!((fresh.rows.len(), fresh.rows[0].label.as_str()), (1, "Project opened"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn deleted_recordings_stay_in_the_file_for_undo() {
        let st = AppState::without_audio();
        let id = lock(&st.pool).unwrap().insert("Aufnahme".into(), vec![0.1; 4800], 48_000);
        let mut t = Track::new(Id::NEW, "Gesang", TrackKind::Audio);
        t.clips.push(klang_core::Clip { id: Id::NEW, name: "A".into(), start_beats: 0.0, source: klang_core::ClipSource::Audio { asset_id: id.clone(), source_bpm: 120.0, warp: true, duration_secs: 0.1 } });
        st.execute(Command::AddTrack { track: t, index: None }, false).unwrap();
        let tid = lock(&st.session).unwrap().project.tracks[0].id;
        st.execute(Command::RemoveTrack { track_id: tid }, false).unwrap();
        let dir = std::env::temp_dir().join(format!("klang-historie-audio-{}", std::process::id()));
        let path = dir.join("Lied.klangwelt");
        st.save_project(&path).unwrap();
        let loaded = crate::project::load_with_history(&path).unwrap();
        assert!(loaded.project.tracks.is_empty());
        assert_eq!(loaded.audio.len(), 1, "the recording of the deleted track is in the file");
        let other = AppState::without_audio();
        other.open_project(&path).unwrap();
        other.undo().unwrap();
        assert_eq!(lock(&other.session).unwrap().project.tracks.len(), 1);
        assert!(lock(&other.pool).unwrap().get(&id).is_some());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn detail_texts() {
        let m = Metrics { beats_per_bar: 4, tempo_bpm: 120.0, beat_unit: 4, ..Default::default() };
        let scope = Scope {
            tracks: ["A", "B", "C", "D"].iter().map(|n| klang_core::history::ScopeTrack { id: Id(1), name: n.to_string() }).collect(),
            range: Some((4.0, 4.5)),
            sound: true,
            notes_removed: 1,
            ..Default::default()
        };
        assert_eq!(detail(&scope, Some(&m), &m), "A, B +2 · bar 2 · sound · −1 note");
        let waltz = Metrics { beats_per_bar: 3, ..m.clone() };
        assert_eq!(detail(&Scope::default(), Some(&m), &waltz), "4/4 → 3/4");
    }
}
