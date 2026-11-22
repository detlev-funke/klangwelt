//! The change history for the UI: the steps of the active variant as a
//! table with key figures, checkpoints, variants and the branches that leave
//! the line – and combining variants (what each changed, conflicts, preview,
//! the combined variant). Undo, redo, jumping, variants and the merge itself run
//! in the core ([`klang_core::Session`], [`klang_core::merge`]); this module
//! only puts them into words.

use crate::state::{lock, AppState, CmdResult, Snapshot};
use klang_core::merge::{self, Choices, Segment, Source, ORIGIN};
use klang_core::{ClipSource, History, Metrics, NodeId, Scope, Track};
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

// ---------------------------------------------------------------- Combining variants

/// Where the variants came from: their newest common step.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct OriginDto {
    pub node: NodeId,
    /// Position on its line (0 = the first state).
    pub index: usize,
    pub label: String,
    pub time: u64,
    pub checkpoints: Vec<String>,
}

/// Tempo and time signature of one version.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MeterDto {
    pub source: Source,
    pub tempo: f64,
    pub beats_per_bar: u32,
    pub beat_unit: u32,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDecision {
    pub by: Vec<Source>,
    pub conflict: bool,
    /// The origin's first, then the variants that changed it.
    pub options: Vec<MeterDto>,
}

/// What one version changed about a track itself, in words ("sound, volume").
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SettingsChange {
    pub source: Source,
    pub what: String,
}

/// The notes (and recordings) of one version of a track, for the lanes view.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LaneDto {
    pub source: Source,
    pub present: bool,
    /// [start, length, pitch] in beats of the song.
    pub notes: Vec<[f32; 3]>,
    /// [start, end] of recordings.
    pub audio: Vec<[f32; 2]>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MergeTrackDto {
    pub key: String,
    pub track_id: u64,
    pub name: String,
    pub color: String,
    pub in_origin: bool,
    pub present_in: Vec<Source>,
    pub settings_by: Vec<Source>,
    pub settings_conflict: bool,
    pub changes: Vec<SettingsChange>,
    pub segments: Vec<Segment>,
    pub lanes: Vec<LaneDto>,
}

/// Everything the "Combine" view shows.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MergeView {
    pub origin: OriginDto,
    pub target: Source,
    /// Taking part, the target first.
    pub variants: Vec<NamedRef>,
    pub beats_per_bar: u32,
    /// The longest version, in beats.
    pub length_beats: f64,
    pub project: ProjectDecision,
    pub tracks: Vec<MergeTrackDto>,
    /// Name the combined variant gets.
    pub name: String,
}

/// What the preview plays.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MergePreview {
    pub end_beats: f64,
    pub tempo: f64,
}

/// The track itself in words: what `t` changed compared with `o`.
fn settings_text(o: Option<&Track>, t: Option<&Track>) -> String {
    match (o, t) {
        (None, Some(_)) => tr!("new track"),
        (Some(_), None) => tr!("removed"),
        (Some(o), Some(t)) => {
            let mut w = Vec::new();
            if o.instrument != t.instrument || o.pitch_semitones != t.pitch_semitones || o.kind != t.kind {
                w.push(tr!("sound"));
            }
            if o.gain_db != t.gain_db || o.pan != t.pan || o.mute != t.mute || o.solo != t.solo || o.group != t.group {
                w.push(tr!("mix"));
            }
            if o.name != t.name || o.color != t.color {
                w.push(tr!("name"));
            }
            w.join(", ")
        }
        (None, None) => String::new(),
    }
}

const LANE_NOTES: usize = 4000;

fn lane_of(source: Source, t: Option<&Track>, bpm: f64) -> LaneDto {
    let mut notes = Vec::new();
    let mut audio = Vec::new();
    for c in t.map(|t| t.clips.as_slice()).unwrap_or_default() {
        match &c.source {
            ClipSource::Notes { notes: ns, .. } => {
                for n in ns.iter().take(LANE_NOTES.saturating_sub(notes.len())) {
                    notes.push([(c.start_beats + n.start_beats) as f32, n.length_beats as f32, n.pitch as f32]);
                }
            }
            ClipSource::Audio { .. } => audio.push([c.start_beats as f32, (c.start_beats + c.length_beats(bpm)) as f32]),
        }
    }
    LaneDto { source, present: t.is_some(), notes, audio }
}

/// The request of the view: which variant to build on, which ones to add, the choices.
pub struct MergeRequest {
    pub target: u32,
    pub others: Vec<u32>,
    pub choices: Choices,
}

impl AppState {
    /// Common origin, its project and the variants' projects (the target first).
    fn merge_inputs(&self, target: u32, others: &[u32]) -> CmdResult<klang_core::MergeSources> {
        let mut ids = vec![target];
        for o in others {
            if !ids.contains(o) {
                ids.push(*o);
            }
        }
        lock(&self.session)?.merge_sources(&ids).map_err(|e| e.to_string())
    }

    /// What combining `others` onto `target` means: per track the changed
    /// bars, the track itself, conflicts – and the notes of each version.
    pub fn history_merge_plan(&self, target: u32, others: &[u32]) -> CmdResult<MergeView> {
        let src = self.merge_inputs(target, others)?;
        let (origin, op, versions) = (src.origin, &src.origin_project, &src.variants);
        let v = src.versions();
        let plan = merge::plan(&v, target);
        let s = lock(&self.session)?;
        let h = s.history();
        let name_of = |id: u32| h.variant(id).map(|x| translate(&x.name)).unwrap_or_default();
        let on = h.node(origin).expect("origin in the history");
        let origin_dto = OriginDto {
            node: origin,
            index: h.depth(origin),
            label: translate(&on.label),
            time: on.time,
            checkpoints: h.checkpoints().iter().filter(|c| c.node == origin).map(|c| c.name.clone()).collect(),
        };
        let meter = |s: Source| {
            let p = v.project(s);
            MeterDto { source: s, tempo: p.tempo_bpm, beats_per_bar: p.beats_per_bar, beat_unit: p.beat_unit }
        };
        let project = ProjectDecision {
            by: plan.project_by.clone(),
            conflict: plan.project_conflict,
            options: std::iter::once(ORIGIN).chain(plan.project_by.iter().copied()).map(meter).collect(),
        };
        let length_beats = std::iter::once(op).chain(versions.iter().map(|(_, p)| p)).map(|p| Metrics::of(p).length_beats).fold(0.0, f64::max);
        let track_in = |s: Source, key: &str, id: klang_core::Id, in_origin: bool| -> Option<&Track> {
            let p = v.project(s);
            // tracks a variant added under an ID another variant used too have the key "id@variant"
            match key.split_once('@') {
                Some((_, owner)) if owner.parse::<u32>().ok() != Some(s) => None,
                _ if s == ORIGIN && !in_origin => None,
                _ => p.track(id),
            }
        };
        let tracks = plan
            .tracks
            .iter()
            .map(|tp| {
                let get = |s: Source| track_in(s, &tp.key, tp.track_id, tp.in_origin);
                let shown = get(target).or_else(|| tp.present_in.first().and_then(|s| get(*s))).or_else(|| get(ORIGIN));
                let mut sources: Vec<Source> = vec![ORIGIN];
                for s in tp.segments.iter().flat_map(|x| x.by.iter()).chain(&tp.settings_by) {
                    if !sources.contains(s) {
                        sources.push(*s);
                    }
                }
                let lanes = if tp.segments.is_empty() { Vec::new() } else { sources.iter().map(|s| lane_of(*s, get(*s), v.project(*s).tempo_bpm)).collect() };
                MergeTrackDto {
                    key: tp.key.clone(),
                    track_id: tp.track_id.0,
                    name: shown.map(|t| t.name.clone()).unwrap_or_default(),
                    color: shown.map(|t| t.color.clone()).unwrap_or_default(),
                    in_origin: tp.in_origin,
                    present_in: tp.present_in.clone(),
                    settings_by: tp.settings_by.clone(),
                    settings_conflict: tp.settings_conflict,
                    changes: tp.settings_by.iter().map(|s| SettingsChange { source: *s, what: settings_text(get(ORIGIN), get(*s)) }).collect(),
                    segments: tp.segments.clone(),
                    lanes,
                }
            })
            .collect();
        let variants: Vec<NamedRef> = versions.iter().map(|(id, _)| NamedRef { id: *id, name: name_of(*id) }).collect();
        let name = variants.iter().map(|x| x.name.as_str()).collect::<Vec<_>>().join(" + ");
        Ok(MergeView { origin: origin_dto, target, variants, beats_per_bar: plan.beats_per_bar, length_beats, project, tracks, name })
    }

    /// Plays the combination from `start` (beats) without changing the project;
    /// open conflicts sound as the target has them.
    pub fn history_merge_preview(&self, req: &MergeRequest, start: f64) -> CmdResult<MergePreview> {
        let src = self.merge_inputs(req.target, &req.others)?;
        let v = src.versions();
        let plan = merge::plan(&v, req.target);
        let result = merge::preview(&v, &plan, &req.choices);
        let engine = self.engine()?;
        let sr = engine.info().sample_rate;
        let arrangement = {
            let mut pool = lock(&self.pool)?;
            crate::state::build_arrangement(&result, &mut pool, sr)
        };
        engine.stop();
        engine.set_arrangement_with_tempo(arrangement, result.tempo_bpm, result.beats_per_bar);
        let end = Metrics::of(&result).length_beats;
        let start = start.clamp(0.0, (end - 1e-3).max(0.0));
        engine.set_loop(None);
        engine.set_stop_at((end > 0.0).then_some((end, start)));
        engine.seek_frames((start * sr as f64 * 60.0 / result.tempo_bpm).round() as u64);
        engine.play();
        Ok(MergePreview { end_beats: end, tempo: result.tempo_bpm })
    }

    /// Ends the preview: the engine plays the project again.
    pub fn history_merge_preview_stop(&self) -> CmdResult<()> {
        if let Ok(e) = self.engine() {
            e.stop();
        }
        self.sync_engine()
    }

    /// Combines the variants into a new variant (one step) and switches to it.
    pub fn history_merge_apply(&self, req: &MergeRequest, name: Option<String>) -> CmdResult<Snapshot> {
        let src = self.merge_inputs(req.target, &req.others)?;
        let v = src.versions();
        let plan = merge::plan(&v, req.target);
        let result = merge::combine(&v, &plan, &req.choices).map_err(|open| trn!(open.len(), "Decide the conflict first", "Decide the {n} conflicts first"))?;
        {
            let mut s = lock(&self.session)?;
            let names: Vec<String> = src.variants.iter().map(|(id, _)| s.history().variant(*id).map(|x| translate(&x.name)).unwrap_or_default()).collect();
            let label = tr!("Combined: {names}", names = names[1..].join(", "));
            let name = name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| names.join(" + "));
            s.add_combined(req.target, &result, Some(name), label).map_err(|e| e.to_string())?;
        }
        self.sync_engine()?;
        self.snapshot()
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

    /// Lead with one note per bar (8 bars), then two variants: the main version
    /// changes bars 5–6, "Refrain" bars 6–8 and the lead's sound, and adds a pad.
    fn two_variants(st: &AppState) -> (u32, u32, Id) {
        let notes: Vec<Note> = (0..8).map(|i| Note::new(60, i as f64 * 4.0, 1.0, 90)).collect();
        let mut lead = Track::new(Id::NEW, "Lead", TrackKind::Instrument);
        lead.clips.push(klang_core::Clip { id: Id::NEW, name: "A".into(), start_beats: 0.0, source: ClipSource::Notes { length_beats: 32.0, notes: notes.clone() } });
        st.execute(Command::AddTrack { track: lead, index: None }, false).unwrap();
        let (tid, cid) = {
            let s = lock(&st.session).unwrap();
            (s.project.tracks[0].id, s.project.tracks[0].clips[0].id)
        };
        st.history_checkpoint(Some("Rohfassung".into())).unwrap();
        let base = st.history_view().unwrap().current;
        let main = st.history_view().unwrap().active;
        let edit = |f: &dyn Fn(&mut Vec<Note>)| {
            let mut n = notes.clone();
            f(&mut n);
            st.execute(Command::SetClipNotes { track_id: tid, clip_id: cid, notes: n, length_beats: None }, false).unwrap();
        };
        edit(&|n| (4..6).for_each(|i| n[i].pitch = 62));
        st.history_variant_new(Some("Refrain".into()), Some(base)).unwrap();
        edit(&|n| (5..8).for_each(|i| n[i].pitch = 67));
        let mut inst = lock(&st.session).unwrap().project.tracks[0].instrument.clone().unwrap();
        inst.preset = "dunkel".into();
        st.execute(Command::SetTrack { track_id: tid, patch: klang_core::TrackPatch { instrument: Some(inst), ..Default::default() } }, false).unwrap();
        let mut pad = Track::new(Id::NEW, "Pad", TrackKind::Instrument);
        pad.clips.push(klang_core::Clip { id: Id::NEW, name: "P".into(), start_beats: 16.0, source: ClipSource::Notes { length_beats: 16.0, notes: vec![Note::new(48, 0.0, 8.0, 80)] } });
        st.execute(Command::AddTrack { track: pad, index: None }, false).unwrap();
        let refrain = st.history_view().unwrap().active;
        (main, refrain, tid)
    }

    #[test]
    fn combine_view_lists_changed_bars_conflicts_and_lanes() {
        let st = AppState::without_audio();
        let (main, refrain, tid) = two_variants(&st);
        let v = st.history_merge_plan(main, &[refrain]).unwrap();
        assert_eq!((v.origin.label.as_str(), v.origin.checkpoints.clone()), ("Add track", vec!["Rohfassung".to_string()]));
        assert_eq!(v.variants.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["Main version", "Refrain"]);
        assert_eq!(v.name, "Main version + Refrain");
        assert_eq!((v.beats_per_bar, v.length_beats), (4, 32.0));
        assert!(v.project.by.is_empty());
        let lead = &v.tracks[0];
        assert_eq!((lead.name.as_str(), lead.track_id), ("Lead", tid.0));
        let segs: Vec<(f64, f64, Vec<u32>, bool)> = lead.segments.iter().map(|s| (s.start, s.end, s.by.clone(), s.conflict)).collect();
        assert_eq!(segs, vec![(16.0, 20.0, vec![main], false), (20.0, 24.0, vec![main, refrain], true), (24.0, 32.0, vec![refrain], false)]);
        assert_eq!(lead.changes.iter().map(|c| (c.source, c.what.as_str())).collect::<Vec<_>>(), [(refrain, "sound")]);
        assert_eq!(lead.lanes.iter().map(|l| l.source).collect::<Vec<_>>(), [ORIGIN, main, refrain]);
        assert_eq!(lead.lanes[2].notes[7], [28.0, 1.0, 67.0]);
        let pad = &v.tracks[1];
        assert_eq!((pad.name.as_str(), pad.in_origin, pad.changes[0].what.as_str()), ("Pad", false, "new track"));
        // a conflict left open: no combining
        let req = MergeRequest { target: main, others: vec![refrain], choices: Choices::default() };
        assert_eq!(st.history_merge_apply(&req, None).err().as_deref(), Some("Decide the conflict first"));
        // the refrain's bar 6, everything else by default
        let choices = Choices { segments: vec![merge::SegmentChoice { key: lead.key.clone(), start: 20.0, source: refrain }], ..Default::default() };
        let req = MergeRequest { target: main, others: vec![refrain], choices };
        st.history_merge_apply(&req, None).unwrap();
        let h = st.history_view().unwrap();
        assert_eq!(h.variants.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["Main version", "Refrain", "Main version + Refrain"]);
        assert!(h.variants[2].active);
        let last = h.rows.last().unwrap();
        assert_eq!(last.label, "Combined: Refrain");
        assert_eq!(last.metrics.tracks, 2);
        let p = lock(&st.session).unwrap().project.clone();
        let pitches: Vec<u8> = match &p.tracks[0].clips[0].source { ClipSource::Notes { notes, .. } => notes.iter().map(|n| n.pitch).collect(), _ => vec![] };
        assert_eq!(pitches, vec![60, 60, 60, 60, 62, 67, 67, 67]);
        assert_eq!(p.tracks[0].instrument.as_ref().unwrap().preset, "dunkel");
        assert_eq!(p.tracks[1].name, "Pad");
        // undo: back at the main version's state
        st.undo().unwrap();
        assert_eq!(lock(&st.session).unwrap().project.tracks.len(), 1);
        // the preview needs an engine
        assert!(st.history_merge_preview(&req, 0.0).is_err());
    }
}
