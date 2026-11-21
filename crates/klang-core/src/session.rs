use crate::command::{apply, Command, CoreError};
use crate::history::{Before, Checkpoint, History, HistoryFile, NodeId, SavedStep, Variant, HISTORY_FILE_VERSION};
use crate::metrics::Metrics;
use crate::model::Project;
use klang_i18n::{tk, tr};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

/// Revisions are unique within the process (also across sessions: opening
/// another project never repeats a revision an agent may still hold).
static REVISION: AtomicU64 = AtomicU64::new(0);

fn next_revision() -> u64 {
    REVISION.fetch_add(1, Ordering::Relaxed) + 1
}

fn invalid<T>(msg: String) -> Result<T, CoreError> {
    Err(CoreError::Invalid(msg))
}

/// An open project together with its history (undo/redo, checkpoints, variants).
#[derive(Clone, Debug)]
pub struct Session {
    pub project: Project,
    history: History,
    /// Changes with every change of the content (not with pure view changes).
    revision: u64,
    /// The step of a running control gesture and the state before it began:
    /// further values of the same gesture go into that step.
    gesture: Option<(NodeId, Before)>,
}

impl Default for Session {
    fn default() -> Self {
        Self::new(Project::default())
    }
}

/// Applies the way back (`up`) and forward (`down`) to `project`; on an error
/// everything applied so far is reverted.
fn walk(project: &mut Project, h: &History, up: &[NodeId], down: &[NodeId]) -> Result<(), CoreError> {
    let steps = up
        .iter()
        .map(|n| h.n(*n).inverse.clone())
        .chain(down.iter().map(|n| h.n(*n).forward.clone()));
    let mut done: Vec<Command> = Vec::new();
    for cmd in steps {
        let Some(cmd) = cmd else { return Err(CoreError::Invalid("history step without command".into())) };
        match apply(project, cmd) {
            Ok(back) => done.push(back),
            Err(e) => {
                for back in done.into_iter().rev() {
                    let _ = apply(project, back);
                }
                return Err(e);
            }
        }
    }
    Ok(())
}

impl Session {
    pub fn new(project: Project) -> Self {
        Self::with_label(project, tk!("Start").into())
    }

    /// New session whose first state carries `label` ("Project opened" …; a
    /// translation key is translated when shown).
    pub fn with_label(project: Project, label: String) -> Self {
        // stored as the key: shown in the language of the moment (see the history view)
        let history = History::new(&project, label, tk!("Main version").into());
        Self { project, history, revision: next_revision(), gesture: None }
    }

    /// The history (read-only).
    pub fn history(&self) -> &History {
        &self.history
    }

    /// Runs a command. With `merge = true` it merges with the previous
    /// step if both affect the same field (control gestures).
    pub fn execute(&mut self, mut cmd: Command, merge: bool) -> Result<(), CoreError> {
        if cmd.is_view_only() {
            apply(&mut self.project, cmd)?;
            return Ok(());
        }
        let next_id = self.project.next_id;
        cmd.assign_ids(&mut self.project);
        let label = cmd.label();
        let key = cmd.merge_key();
        let cur = self.history.current;
        let merging = merge
            && key.is_some()
            && self.gesture.as_ref().is_some_and(|(n, _)| *n == cur)
            && self.history.n(cur).merge_key == key
            && self.history.n(cur).children.is_empty()
            && !self.history.checkpoints.iter().any(|c| c.node == cur);
        let before = (!merging).then(|| Before::of(&self.project, &cmd));
        let forward = cmd.clone();
        let inverse = match apply(&mut self.project, cmd) {
            Ok(inv) => inv,
            Err(e) => {
                self.project.next_id = next_id;
                return Err(e);
            }
        };
        self.revision = next_revision();
        let metrics = Metrics::of(&self.project);
        if let Some(before) = before {
            let scope = before.scope(&self.project);
            let id = self.history.push(label, forward, inverse, key.clone(), metrics, scope);
            self.gesture = key.map(|_| (id, before));
            self.history.prune();
        } else if let Some((id, before)) = &self.gesture {
            // the step's way back still leads to the state before the gesture
            let scope = before.scope(&self.project);
            let n = self.history.n_mut(*id);
            n.forward = Some(forward);
            n.metrics = metrics;
            n.scope = scope;
            n.time = crate::history::now();
        }
        Ok(())
    }

    pub fn undo(&mut self) -> Result<bool, CoreError> {
        let cur = self.history.current;
        let Some(parent) = self.history.n(cur).parent else {
            return Ok(false);
        };
        let inverse = self.history.n(cur).inverse.clone().ok_or_else(|| CoreError::Invalid("history step without command".into()))?;
        apply(&mut self.project, inverse)?;
        self.history.n_mut(parent).redo = Some(cur);
        self.history.current = parent;
        self.gesture = None;
        self.revision = next_revision();
        Ok(true)
    }

    pub fn redo(&mut self) -> Result<bool, CoreError> {
        let Some(next) = self.history.redo_target() else {
            return Ok(false);
        };
        let forward = self.history.n(next).forward.clone().ok_or_else(|| CoreError::Invalid("history step without command".into()))?;
        apply(&mut self.project, forward)?;
        let cur = self.history.current;
        self.history.n_mut(cur).redo = Some(next);
        self.history.current = next;
        self.gesture = None;
        self.revision = next_revision();
        Ok(true)
    }

    pub fn undo_label(&self) -> Option<&str> {
        let n = self.history.n(self.history.current);
        n.parent.map(|_| n.label.as_str())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.history.redo_target().map(|n| self.history.n(n).label.as_str())
    }

    /// Revision of the content: changes with every change, undo and redo.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Moves to step `target` without touching variants.
    fn move_to(&mut self, target: NodeId) -> Result<(), CoreError> {
        if !self.history.nodes.contains_key(&target) {
            return invalid(tr!("Step {id} is not in the history", id = target));
        }
        let cur = self.history.current;
        if cur == target {
            return Ok(());
        }
        let (up, down) = self.history.path(cur, target);
        walk(&mut self.project, &self.history, &up, &down)?;
        for w in up.windows(2) {
            self.history.n_mut(w[1]).redo = Some(w[0]);
        }
        let mut prev = self.history.common_ancestor(cur, target);
        for &n in &down {
            self.history.n_mut(prev).redo = Some(n);
            prev = n;
        }
        self.history.current = target;
        self.gesture = None;
        self.revision = next_revision();
        Ok(())
    }

    /// Jumps to the state after step `target` (undo/redo across any number of
    /// steps, also onto another variant). A step on an abandoned branch makes
    /// that branch a variant of its own, so its steps stay safe.
    pub fn goto(&mut self, target: NodeId) -> Result<(), CoreError> {
        self.move_to(target)?;
        let h = &self.history;
        if h.is_ancestor(target, h.active().head) {
            return Ok(());
        }
        let nearest = h.variants_through(target).into_iter().min_by_key(|v| h.depth(v.head)).map(|v| v.id);
        match nearest {
            Some(v) => self.history.active = v,
            None => {
                let tip = h.tip(target);
                let name = h.free_variant_name(|n| tr!("Variant {n}", n = n));
                self.add_variant(name, tip);
            }
        }
        Ok(())
    }

    /// The project as it was after step `node` (the open project stays as it is).
    pub fn project_at(&self, node: NodeId) -> Result<Project, CoreError> {
        if !self.history.nodes.contains_key(&node) {
            return invalid(tr!("Step {id} is not in the history", id = node));
        }
        let mut p = self.project.clone();
        let (up, down) = self.history.path(self.history.current, node);
        walk(&mut p, &self.history, &up, &down)?;
        Ok(p)
    }

    // ------------------------------------------------------------ Checkpoints

    /// Names the current state. Without a name: "Checkpoint 1", "Checkpoint 2" …
    pub fn add_checkpoint(&mut self, name: Option<String>) -> Result<u32, CoreError> {
        let name = match name.map(|n| n.trim().to_string()) {
            Some(n) if !n.is_empty() => n,
            Some(_) => return invalid(tr!("The name can't be empty")),
            None => {
                let used: Vec<&str> = self.history.checkpoints.iter().map(|c| c.name.as_str()).collect();
                (self.history.checkpoints.len() + 1..).map(|n| tr!("Checkpoint {n}", n = n)).find(|n| !used.contains(&n.as_str())).unwrap_or_default()
            }
        };
        let id = self.history.next_mark;
        self.history.next_mark += 1;
        let node = self.history.current;
        self.history.checkpoints.push(Checkpoint { id, name, node, time: crate::history::now() });
        self.gesture = None;
        Ok(id)
    }

    pub fn rename_checkpoint(&mut self, id: u32, name: &str) -> Result<(), CoreError> {
        let name = name.trim();
        if name.is_empty() {
            return invalid(tr!("The name can't be empty"));
        }
        let c = self.history.checkpoints.iter_mut().find(|c| c.id == id).ok_or_else(|| CoreError::Invalid(tr!("Checkpoint {id} doesn't exist", id = id)))?;
        c.name = name.to_string();
        Ok(())
    }

    /// Removes the name – the steps stay.
    pub fn remove_checkpoint(&mut self, id: u32) -> Result<(), CoreError> {
        let before = self.history.checkpoints.len();
        self.history.checkpoints.retain(|c| c.id != id);
        if self.history.checkpoints.len() == before {
            return invalid(tr!("Checkpoint {id} doesn't exist", id = id));
        }
        Ok(())
    }

    // ------------------------------------------------------------ Variants

    fn add_variant(&mut self, name: String, head: NodeId) -> u32 {
        let id = self.history.next_mark;
        self.history.next_mark += 1;
        self.history.variants.push(Variant { id, name, head, time: crate::history::now() });
        self.history.active = id;
        id
    }

    /// Starts a variant at step `at` (default: the current step): the project goes
    /// there, and from now on new steps go into the variant – the others stay as they are.
    pub fn create_variant(&mut self, name: Option<String>, at: Option<NodeId>) -> Result<u32, CoreError> {
        let name = match name.map(|n| n.trim().to_string()) {
            Some(n) if !n.is_empty() => n,
            Some(_) => return invalid(tr!("The name can't be empty")),
            None => self.history.free_variant_name(|n| tr!("Variant {n}", n = n)),
        };
        let at = at.unwrap_or(self.history.current);
        self.move_to(at)?;
        Ok(self.add_variant(name, at))
    }

    /// Switches to a variant: the project goes to its newest step.
    pub fn switch_variant(&mut self, id: u32) -> Result<(), CoreError> {
        let head = self.history.variant(id).map(|v| v.head).ok_or_else(|| CoreError::Invalid(tr!("Variant {id} doesn't exist", id = id)))?;
        self.move_to(head)?;
        self.history.active = id;
        Ok(())
    }

    pub fn rename_variant(&mut self, id: u32, name: &str) -> Result<(), CoreError> {
        let name = name.trim();
        if name.is_empty() {
            return invalid(tr!("The name can't be empty"));
        }
        let v = self.history.variants.iter_mut().find(|v| v.id == id).ok_or_else(|| CoreError::Invalid(tr!("Variant {id} doesn't exist", id = id)))?;
        v.name = name.to_string();
        Ok(())
    }

    /// Deletes a variant with the steps only it has. The last variant stays;
    /// deleting the active one switches to the one it branched from.
    pub fn delete_variant(&mut self, id: u32) -> Result<(), CoreError> {
        let Some(v) = self.history.variant(id).cloned() else {
            return invalid(tr!("Variant {id} doesn't exist", id = id));
        };
        if self.history.variants.len() <= 1 {
            return invalid(tr!("The only variant can't be deleted"));
        }
        if self.history.active == id {
            let fork = self.history.fork_of(id);
            let h = &self.history;
            let next = h.variants.iter().filter(|o| o.id != id).find(|o| h.is_ancestor(fork, o.head)).unwrap_or(&h.variants[0]).id;
            let next = if next == id { h.variants.iter().find(|o| o.id != id).map(|o| o.id).unwrap_or(next) } else { next };
            self.switch_variant(next)?;
        }
        self.history.variants.retain(|o| o.id != id);
        let alive = self.history.alive();
        let mut doomed: Vec<NodeId> = self.history.ancestors(v.head).into_iter().filter(|n| !alive.contains(n)).collect();
        // the topmost one takes the rest with it
        doomed.reverse();
        if let Some(&top) = doomed.first() {
            self.history.remove_subtree(top);
        }
        Ok(())
    }

    /// Forgets all steps: the current state becomes the first one, checkpoints and
    /// other variants are gone.
    pub fn clear_history(&mut self, label: String) {
        let name = self.history.active().name.clone();
        self.history = History::new(&self.project, label, name);
        self.gesture = None;
    }

    // ------------------------------------------------------------ Combining variants

    /// What combining `variants` needs: their common origin (the newest step all
    /// of them share), the project there and the project of each variant.
    pub fn merge_sources(&self, variants: &[u32]) -> Result<MergeSources, CoreError> {
        if variants.len() < 2 {
            return invalid(tr!("Choose at least two variants to combine"));
        }
        let mut heads = Vec::with_capacity(variants.len());
        for id in variants {
            let v = self.history.variant(*id).ok_or_else(|| CoreError::Invalid(tr!("Variant {id} doesn't exist", id = id)))?;
            heads.push(v.head);
        }
        let origin = heads.iter().skip(1).fold(heads[0], |acc, &h| self.history.common_ancestor(acc, h));
        let mut projects = Vec::with_capacity(variants.len());
        for (id, head) in variants.iter().zip(&heads) {
            projects.push((*id, self.project_at(*head)?));
        }
        Ok(MergeSources { origin, origin_project: self.project_at(origin)?, variants: projects })
    }

    /// Starts a variant at the newest step of `target` with one step `label` that
    /// turns its project into `result` – the combined variants. Returns its ID.
    pub fn add_combined(&mut self, target: u32, result: &Project, name: Option<String>, label: String) -> Result<u32, CoreError> {
        let head = self.history.variant(target).map(|v| v.head).ok_or_else(|| CoreError::Invalid(tr!("Variant {id} doesn't exist", id = target)))?;
        let id = self.create_variant(name, Some(head))?;
        let commands = crate::merge::commands_between(&self.project, result);
        if !commands.is_empty() {
            if let Err(e) = self.execute(Command::Batch { label, commands }, false) {
                let _ = self.delete_variant(id);
                return Err(e);
            }
        }
        Ok(id)
    }

    // ------------------------------------------------------------ Agent batches

    /// Mark in the history for [`Session::squash`] and [`Session::rollback`].
    pub fn mark(&self) -> UndoMark {
        let h = &self.history;
        UndoMark {
            node: h.current,
            next_node: h.next_node,
            redo: h.n(h.current).redo,
            variants: h.variants.clone(),
            active: h.active,
        }
    }

    /// The new steps from `mark` to the current one, top to bottom – `None` if
    /// the current step is not below the mark.
    fn chain_since(&self, mark: &UndoMark) -> Option<Vec<NodeId>> {
        let mut chain = Vec::new();
        let mut n = self.history.current;
        while n >= mark.next_node {
            chain.push(n);
            n = self.history.n(n).parent?;
        }
        if n != mark.node {
            return None;
        }
        chain.reverse();
        Some(chain)
    }

    /// Steps since `mark` (have there been changes?).
    pub fn steps_since(&self, mark: &UndoMark) -> usize {
        self.chain_since(mark).map(|c| c.len()).unwrap_or(0)
    }

    /// Merges all steps since `mark` into one undo step named `label`.
    pub fn squash(&mut self, mark: &UndoMark, label: String) {
        match self.chain_since(mark) {
            Some(chain) if !chain.is_empty() => {
                self.history.squash_chain(mark.node, &chain, label);
                self.gesture = None;
            }
            _ => {}
        }
    }

    /// Reverts all steps since `mark` – the history is as it was then,
    /// including what could be redone.
    pub fn rollback(&mut self, mark: UndoMark) -> Result<(), CoreError> {
        self.move_to(mark.node)?;
        let fresh: Vec<NodeId> = self.history.nodes.keys().copied().filter(|&n| n >= mark.next_node).collect();
        for n in fresh {
            self.history.remove_subtree(n);
        }
        if let Some(n) = self.history.nodes.get_mut(&mark.node) {
            n.redo = mark.redo;
        }
        self.history.variants = mark.variants;
        self.history.active = mark.active;
        self.gesture = None;
        self.revision = next_revision();
        Ok(())
    }

    // ------------------------------------------------------------ File

    /// The history for saving with the project.
    pub fn history_file(&self) -> Result<HistoryFile, CoreError> {
        let h = &self.history;
        let root = self.project_at(h.root)?;
        let steps = h
            .preorder()
            .into_iter()
            .map(|id| {
                let n = h.n(id);
                SavedStep { id, parent: n.parent, label: n.label.clone(), time: n.time, forward: n.forward.clone(), redo: n.redo }
            })
            .collect();
        Ok(HistoryFile {
            version: HISTORY_FILE_VERSION,
            root,
            steps,
            current: h.current,
            checkpoints: h.checkpoints.clone(),
            variants: h.variants.clone(),
            active: h.active,
        })
    }

    /// Opens `project` with its saved history. The steps are replayed from the
    /// first state; if they don't lead exactly to `project` (file from elsewhere,
    /// edited by hand), the history is refused.
    pub fn restore(project: Project, file: HistoryFile) -> Result<Session, CoreError> {
        let bad = |m: &str| CoreError::Invalid(format!("history: {m}"));
        if file.version > HISTORY_FILE_VERSION {
            return Err(bad("newer format"));
        }
        let root_step = file.steps.first().ok_or_else(|| bad("empty"))?;
        if root_step.parent.is_some() {
            return Err(bad("first step has a parent"));
        }
        let root = root_step.id;
        let mut nodes: BTreeMap<NodeId, crate::history::Node> = BTreeMap::new();
        for s in &file.steps {
            if nodes.contains_key(&s.id) {
                return Err(bad("step twice"));
            }
            if let Some(p) = s.parent {
                let parent = nodes.get_mut(&p).ok_or_else(|| bad("child before its parent"))?;
                parent.children.push(s.id);
                if s.forward.is_none() {
                    return Err(bad("step without command"));
                }
            } else if s.id != root {
                return Err(bad("two first steps"));
            }
            nodes.insert(s.id, crate::history::Node {
                id: s.id,
                parent: s.parent,
                children: Vec::new(),
                redo: s.redo,
                label: s.label.clone(),
                time: s.time,
                forward: s.forward.clone(),
                inverse: None,
                merge_key: None,
                metrics: Metrics::default(),
                scope: Default::default(),
            });
        }
        // replay depth first: forward into every child, back again afterwards
        let mut p = file.root.clone();
        nodes.get_mut(&root).expect("root").metrics = Metrics::of(&p);
        let mut stack: Vec<(NodeId, usize)> = vec![(root, 0)];
        while let Some((id, i)) = stack.pop() {
            let child = nodes[&id].children.get(i).copied();
            match child {
                Some(c) => {
                    stack.push((id, i + 1));
                    let fwd = nodes[&c].forward.clone().expect("checked");
                    let before = Before::of(&p, &fwd);
                    let inverse = apply(&mut p, fwd).map_err(|e| bad(&e.to_string()))?;
                    let n = nodes.get_mut(&c).expect("node");
                    n.inverse = Some(inverse);
                    n.metrics = Metrics::of(&p);
                    n.scope = before.scope(&p);
                    stack.push((c, 0));
                }
                None => {
                    if let Some(inv) = nodes[&id].inverse.clone() {
                        apply(&mut p, inv).map_err(|e| bad(&e.to_string()))?;
                    }
                }
            }
        }
        let next_mark = file.checkpoints.iter().map(|c| c.id).chain(file.variants.iter().map(|v| v.id)).max().unwrap_or(0) + 1;
        let next_node = nodes.keys().max().copied().unwrap_or(root) + 1;
        let known = |n: NodeId| nodes.contains_key(&n);
        if !known(file.current) || file.variants.iter().any(|v| !known(v.head)) || file.checkpoints.iter().any(|c| !known(c.node)) {
            return Err(bad("unknown step"));
        }
        let mut variants = file.variants;
        if variants.is_empty() {
            variants.push(Variant { id: next_mark, name: tk!("Main version").into(), head: file.current, time: 0 });
        }
        let active = if variants.iter().any(|v| v.id == file.active) { file.active } else { variants[0].id };
        let mut history = History {
            nodes,
            root,
            current: root,
            next_node,
            checkpoints: file.checkpoints,
            variants,
            active,
            next_mark: next_mark + 1,
            limit: crate::history::HISTORY_LIMIT,
        };
        // to the saved state – it has to be exactly the saved project
        let (_, down) = history.path(root, file.current);
        walk(&mut p, &history, &[], &down).map_err(|e| bad(&e.to_string()))?;
        history.current = file.current;
        p.next_id = project.next_id;
        if p != project {
            return Err(bad("doesn't lead to the saved project"));
        }
        history.prune();
        Ok(Session { project, history, revision: next_revision(), gesture: None })
    }
}

/// The versions for combining variants (see [`Session::merge_sources`]).
#[derive(Clone, Debug)]
pub struct MergeSources {
    /// The newest step all the variants share.
    pub origin: NodeId,
    pub origin_project: Project,
    /// Each variant's ID with its project, in the order asked for.
    pub variants: Vec<(u32, Project)>,
}

impl MergeSources {
    /// The versions as [`crate::merge`] takes them.
    pub fn versions(&self) -> crate::merge::Versions<'_> {
        crate::merge::Versions { origin: &self.origin_project, variants: self.variants.iter().map(|(id, p)| (*id, p)).collect() }
    }
}

/// A point in the history (see [`Session::mark`]).
#[derive(Clone, Debug)]
pub struct UndoMark {
    node: NodeId,
    next_node: NodeId,
    redo: Option<NodeId>,
    variants: Vec<Variant>,
    active: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ClipPatch, ClipSource, Clip, Id, Note, ProjectPatch, Track, TrackKind, TrackPatch, TrackView};

    fn audio_clip(start: f64) -> Clip {
        Clip {
            id: Id::NEW,
            name: "Loop".into(),
            start_beats: start,
            source: ClipSource::Audio { asset_id: "a1".into(), source_bpm: 100.0, warp: true, duration_secs: 2.4 },
        }
    }

    #[test]
    fn add_undo_redo_roundtrip() {
        let mut s = Session::default();
        let before = s.project.clone();
        s.execute(Command::AddTrack { track: Track::new(Id::NEW, "Beat", TrackKind::Audio), index: None }, false).unwrap();
        let tid = s.project.tracks[0].id;
        s.execute(Command::AddClip { track_id: tid, clip: audio_clip(4.0) }, false).unwrap();
        let after = s.project.clone();
        assert_eq!(s.undo_label(), Some("Add clip"));

        assert!(s.undo().unwrap());
        assert!(s.undo().unwrap());
        assert_eq!(s.project.tracks, before.tracks);
        assert!(!s.undo().unwrap());

        assert!(s.redo().unwrap());
        assert!(s.redo().unwrap());
        assert_eq!(s.project.tracks, after.tracks);
    }

    #[test]
    fn slider_gesture_merges_into_one_step() {
        let mut s = Session::default();
        s.execute(Command::AddTrack { track: Track::new(Id::NEW, "Bass", TrackKind::Audio), index: None }, false).unwrap();
        let tid = s.project.tracks[0].id;
        for (i, db) in [-1.0, -2.0, -3.0, -6.0].into_iter().enumerate() {
            let patch = TrackPatch { gain_db: Some(db), ..Default::default() };
            s.execute(Command::SetTrack { track_id: tid, patch }, i > 0).unwrap();
        }
        assert_eq!(s.project.tracks[0].gain_db, -6.0);
        s.undo().unwrap();
        assert_eq!(s.project.tracks[0].gain_db, 0.0);
        assert_eq!(s.undo_label(), Some("Add track"));
        // the same with its own label (batch of one command)
        for (i, db) in [-1.0, -4.0].into_iter().enumerate() {
            let patch = TrackPatch { gain_db: Some(db), ..Default::default() };
            let one = Command::Batch { label: "Pegel ziehen".into(), commands: vec![Command::SetTrack { track_id: tid, patch }] };
            s.execute(one, i > 0).unwrap();
        }
        assert_eq!(s.undo_label(), Some("Pegel ziehen"));
        s.undo().unwrap();
        assert_eq!(s.project.tracks[0].gain_db, 0.0, "ein Schritt");
    }

    #[test]
    fn invalid_values_leave_project_untouched() {
        let mut s = Session::default();
        let err = s.execute(Command::SetProject { patch: ProjectPatch { tempo_bpm: Some(1000.0), ..Default::default() } }, false);
        assert!(err.is_err());
        assert_eq!(s.project.tempo_bpm, 120.0);
        assert!(s.undo_label().is_none());
    }

    #[test]
    fn failing_batch_rolls_back() {
        let mut s = Session::default();
        let batch = Command::Batch {
            label: "Test".into(),
            commands: vec![
                Command::AddTrack { track: Track::new(Id::NEW, "A", TrackKind::Audio), index: None },
                Command::RemoveTrack { track_id: Id(999) },
            ],
        };
        assert!(s.execute(batch, false).is_err());
        assert!(s.project.tracks.is_empty());
    }

    #[test]
    fn clip_sync_settings_and_length() {
        let mut s = Session::default();
        s.execute(Command::AddTrack { track: Track::new(Id::NEW, "Loop", TrackKind::Audio), index: None }, false).unwrap();
        let tid = s.project.tracks[0].id;
        s.execute(Command::AddClip { track_id: tid, clip: audio_clip(0.0) }, false).unwrap();
        let cid = s.project.tracks[0].clips[0].id;
        // 2.4 s at 100 BPM = 4 beats – independent of the project tempo.
        assert!((s.project.tracks[0].clips[0].length_beats(140.0) - 4.0).abs() < 1e-9);
        s.execute(Command::SetClip { track_id: tid, clip_id: cid, patch: ClipPatch { warp: Some(false), ..Default::default() } }, false).unwrap();
        assert!((s.project.tracks[0].clips[0].length_beats(120.0) - 4.8).abs() < 1e-9);
    }

    fn notes_clip_session() -> (Session, Id, Id) {
        let mut s = Session::default();
        let mut t = Track::new(Id::NEW, "Lead", TrackKind::Instrument);
        t.clips.push(Clip {
            id: Id::NEW,
            name: "Thema".into(),
            start_beats: 0.0,
            source: ClipSource::Notes { length_beats: 4.0, notes: vec![] },
        });
        s.execute(Command::AddTrack { track: t, index: None }, false).unwrap();
        let (tid, cid) = (s.project.tracks[0].id, s.project.tracks[0].clips[0].id);
        (s, tid, cid)
    }

    fn note(pitch: u8, start: f64, len: f64) -> Note {
        Note::new(pitch, start, len, 100)
    }

    #[test]
    fn note_expression_is_compact_and_validated() {
        // neutral expression values are missing from the JSON, set ones are there
        let plain = serde_json::to_value(note(60, 0.0, 1.0)).unwrap();
        assert!(plain.get("vibrato").is_none() && plain.get("bend").is_none());
        let expressive = Note { vibrato: 0.3, bend: -2.0, ..note(60, 0.0, 1.0) };
        let v = serde_json::to_value(&expressive).unwrap();
        assert_eq!(v["bend"], -2.0);
        let back: Note = serde_json::from_value(serde_json::json!({ "pitch": 60, "startBeats": 0, "lengthBeats": 1, "velocity": 90 })).unwrap();
        assert_eq!(back.vibrato, 0.0);
        // outside the limits → rejected
        assert!(validate_test(&[Note { vibrato: 1.5, ..note(60, 0.0, 1.0) }]).is_err());
        assert!(validate_test(&[Note { bend: -13.0, ..note(60, 0.0, 1.0) }]).is_err());
        assert!(validate_test(&[Note { brightness: 0.5, ..note(60, 0.0, 1.0) }]).is_ok());
    }

    fn validate_test(notes: &[Note]) -> Result<(), crate::CoreError> {
        let (mut s, tid, cid) = notes_clip_session();
        s.execute(Command::SetClipNotes { track_id: tid, clip_id: cid, notes: notes.to_vec(), length_beats: None }, false).map(|_| ())
    }

    fn clip_notes(s: &Session) -> (Vec<Note>, f64) {
        match &s.project.tracks[0].clips[0].source {
            ClipSource::Notes { notes, length_beats } => (notes.clone(), *length_beats),
            _ => unreachable!(),
        }
    }

    #[test]
    fn set_clip_notes_sorts_validates_and_undoes() {
        let (mut s, tid, cid) = notes_clip_session();
        let cmd = Command::SetClipNotes { track_id: tid, clip_id: cid, notes: vec![note(67, 2.0, 1.0), note(60, 0.0, 1.0), note(64, 0.0, 0.5)], length_beats: Some(8.0) };
        s.execute(cmd, false).unwrap();
        let (notes, len) = clip_notes(&s);
        assert_eq!(notes.iter().map(|n| n.pitch).collect::<Vec<_>>(), vec![60, 64, 67]);
        assert_eq!(len, 8.0);
        assert_eq!(s.undo_label(), Some("Edit notes"));

        let bad = Command::SetClipNotes { track_id: tid, clip_id: cid, notes: vec![note(60, -1.0, 1.0)], length_beats: None };
        assert!(s.execute(bad, false).is_err());
        assert_eq!(clip_notes(&s).0.len(), 3);

        s.undo().unwrap();
        assert_eq!(clip_notes(&s), (vec![], 4.0));
        s.redo().unwrap();
        assert_eq!(clip_notes(&s).1, 8.0);
    }

    #[test]
    fn view_switch_is_not_an_undo_step() {
        let (mut s, tid, _) = notes_clip_session();
        assert_eq!(s.project.tracks[0].view, TrackView::Notes, "Instrumentspuren starten in der Notenansicht");
        let before = s.undo_label().map(str::to_string);
        let view = |v| Command::SetTrack { track_id: tid, patch: TrackPatch { view: Some(v), ..Default::default() } };
        s.execute(view(TrackView::Compact), false).unwrap();
        assert_eq!(s.project.tracks[0].view, TrackView::Compact);
        assert_eq!(s.undo_label().map(str::to_string), before);
        // A view together with another change, on the other hand, is a normal step.
        let mixed = Command::SetTrack { track_id: tid, patch: TrackPatch { view: Some(TrackView::Notes), gain_db: Some(-3.0), ..Default::default() } };
        s.execute(mixed, false).unwrap();
        assert_eq!(s.undo_label(), Some("Change volume"));
        let json: Command = serde_json::from_str(r#"{"type":"setTrack","trackId":1,"patch":{"view":"notes"}}"#).unwrap();
        assert!(json.is_view_only());

        // Dragging the row height: likewise no undo step, limits are checked.
        let height = Command::SetTrack { track_id: tid, patch: TrackPatch { notes_height: Some(420.0), ..Default::default() } };
        assert!(height.is_view_only());
        s.execute(height, false).unwrap();
        assert_eq!(s.project.tracks[0].notes_height, 420.0);
        assert_eq!(s.undo_label(), Some("Change volume"));
        let too_small = Command::SetTrack { track_id: tid, patch: TrackPatch { compact_height: Some(5.0), ..Default::default() } };
        assert!(s.execute(too_small, false).is_err());
        let reset = Command::SetTrack { track_id: tid, patch: TrackPatch { notes_height: Some(0.0), ..Default::default() } };
        s.execute(reset, false).unwrap();
        assert_eq!(s.project.tracks[0].notes_height, 0.0);
        assert!(!Command::SetTrack { track_id: tid, patch: TrackPatch::default() }.is_view_only());
    }

    #[test]
    fn revision_changes_with_content_and_steps_squash_into_one() {
        let mut s = Session::default();
        let r0 = s.revision();
        let add = |name: &str| Command::AddTrack { track: Track::new(Id::NEW, name, TrackKind::Instrument), index: None };
        s.execute(add("A"), false).unwrap();
        let r1 = s.revision();
        assert!(r1 > r0);
        let tid = s.project.tracks[0].id;
        // pure view change: same revision
        s.execute(Command::SetTrack { track_id: tid, patch: TrackPatch { notes_height: Some(300.0), ..Default::default() } }, false).unwrap();
        assert_eq!(s.revision(), r1);
        // another session never repeats a revision
        assert!(Session::default().revision() > r1);

        let mark = s.mark();
        s.execute(add("B"), false).unwrap();
        s.execute(Command::SetTrack { track_id: tid, patch: TrackPatch { gain_db: Some(-3.0), ..Default::default() } }, false).unwrap();
        assert_eq!(s.steps_since(&mark), 2);
        s.squash(&mark, "Agent · batch".into());
        assert_eq!(s.undo_label(), Some("Agent · batch"));
        assert_eq!(s.project.tracks.len(), 2);
        s.undo().unwrap();
        assert_eq!(s.project.tracks.len(), 1, "one step reverts both");
        assert_eq!(s.project.tracks[0].gain_db, 0.0);
        s.redo().unwrap();
        assert_eq!((s.project.tracks.len(), s.project.tracks[0].gain_db), (2, -3.0));

        // a rollback after an undo: what could be redone stays redoable
        s.undo().unwrap();
        let mark = s.mark();
        let before = s.revision();
        s.execute(add("C"), false).unwrap();
        assert!(s.redo_label().is_none());
        s.rollback(mark).unwrap();
        assert_eq!(s.project.tracks.len(), 1);
        assert_eq!(s.undo_label(), Some("Add track"));
        assert_eq!(s.redo_label(), Some("Agent · batch"));
        assert!(s.revision() > before);
        s.redo().unwrap();
        assert_eq!(s.project.tracks.len(), 2);
    }

    #[test]
    fn json_shape_for_ui_and_agent() {
        let json = r#"{"type":"setTrack","trackId":3,"patch":{"gainDb":-6.5,"mute":true}}"#;
        let cmd: Command = serde_json::from_str(json).unwrap();
        assert_eq!(
            cmd,
            Command::SetTrack { track_id: Id(3), patch: TrackPatch { gain_db: Some(-6.5), mute: Some(true), ..Default::default() } }
        );
        let add: Command = serde_json::from_str(r#"{"type":"addTrack","track":{"id":0,"name":"Neu","kind":"instrument"}}"#).unwrap();
        let mut s = Session::default();
        s.execute(add, false).unwrap();
        assert!(s.project.tracks[0].instrument.is_some());
        assert!(!s.project.tracks[0].color.is_empty());
        let round = Project::from_json(&s.project.to_json()).unwrap();
        assert_eq!(round, s.project);
    }
}

#[cfg(test)]
mod history_tests {
    use super::*;
    use crate::{Clip, ClipSource, Id, Note, ProjectPatch, Track, TrackKind, TrackPatch};

    fn add(name: &str) -> Command {
        Command::AddTrack { track: Track::new(Id::NEW, name, TrackKind::Instrument), index: None }
    }

    fn gain(id: Id, db: f32) -> Command {
        Command::SetTrack { track_id: id, patch: TrackPatch { gain_db: Some(db), ..Default::default() } }
    }

    fn names(s: &Session) -> Vec<String> {
        s.project.tracks.iter().map(|t| t.name.clone()).collect()
    }

    fn labels(s: &Session) -> Vec<String> {
        s.history().line().iter().map(|n| s.history().node(*n).unwrap().label.clone()).collect()
    }

    /// Instrument track "Lead" with one clip of notes, as one step.
    fn lead_with_notes(s: &mut Session, notes: Vec<Note>) -> (Id, Id) {
        let mut t = Track::new(Id::NEW, "Lead", TrackKind::Instrument);
        t.clips.push(Clip { id: Id::NEW, name: "A".into(), start_beats: 4.0, source: ClipSource::Notes { length_beats: 16.0, notes } });
        s.execute(Command::AddTrack { track: t, index: None }, false).unwrap();
        let t = s.project.tracks.last().unwrap();
        (t.id, t.clips[0].id)
    }

    #[test]
    fn every_step_has_metrics_and_scope() {
        let mut s = Session::with_label(Project::default(), "Neues Projekt".into());
        assert_eq!(labels(&s), vec!["Neues Projekt"]);
        let (tid, cid) = lead_with_notes(&mut s, vec![Note::new(60, 0.0, 1.0, 90), Note::new(64, 4.0, 1.0, 90)]);
        s.execute(Command::SetClipNotes { track_id: tid, clip_id: cid, notes: vec![Note::new(60, 0.0, 1.0, 90), Note::new(65, 4.0, 2.0, 90), Note::new(67, 8.0, 1.0, 90)], length_beats: None }, false).unwrap();
        s.execute(Command::SetProject { patch: ProjectPatch { tempo_bpm: Some(90.0), ..Default::default() } }, false).unwrap();
        let h = s.history();
        let line = h.line();
        assert_eq!(line.len(), 4);
        let n = |i: usize| h.node(line[i]).unwrap();
        assert_eq!((n(1).metrics.notes, n(1).metrics.tracks, n(1).metrics.clips), (2, 1, 1));
        assert_eq!(n(1).metrics.length_beats, 20.0);
        assert_eq!(n(1).scope.tracks[0].name, "Lead");
        assert_eq!(n(1).scope.range, Some((4.0, 9.0)));
        assert_eq!((n(1).scope.notes_added, n(1).scope.notes_removed), (2, 0));
        // the edit: one note changed, one added → beats 8..13 (clip starts at 4)
        assert_eq!(n(2).metrics.notes, 3);
        assert_eq!(n(2).scope.range, Some((8.0, 13.0)));
        assert_eq!((n(2).scope.notes_added, n(2).scope.notes_removed), (2, 1));
        assert!(n(3).scope.project && n(3).scope.tracks.is_empty());
        assert_eq!(n(3).metrics.tempo_bpm, 90.0);
        assert_eq!(n(2).metrics.tempo_bpm, 120.0);
    }

    #[test]
    fn editing_after_undo_keeps_the_old_steps_as_a_branch() {
        let mut s = Session::default();
        s.execute(add("A"), false).unwrap();
        s.execute(add("B"), false).unwrap();
        s.execute(add("C"), false).unwrap();
        let c = s.history().current();
        s.undo().unwrap();
        s.undo().unwrap();
        assert_eq!(s.redo_label(), Some("Add track"));
        s.execute(add("X"), false).unwrap();
        assert_eq!(names(&s), ["A", "X"]);
        assert!(s.redo_label().is_none(), "redo follows the new line");
        assert_eq!(s.history().len(), 5, "B and C are still there");
        // jumping to the old branch makes it a variant of its own
        s.goto(c).unwrap();
        assert_eq!(names(&s), ["A", "B", "C"]);
        assert_eq!(s.history().variants().len(), 2);
        assert_eq!(s.history().active().head, c);
        assert_eq!(s.history().active().name, "Variant 2");
        // and back to the main version
        let main = s.history().variants()[0].id;
        s.switch_variant(main).unwrap();
        assert_eq!(names(&s), ["A", "X"]);
    }

    #[test]
    fn goto_any_step_and_undo_redo_along_the_line() {
        let mut s = Session::default();
        for n in ["A", "B", "C", "D"] {
            s.execute(add(n), false).unwrap();
        }
        let line = s.history().line();
        let r0 = s.revision();
        s.goto(line[1]).unwrap();
        assert_eq!(names(&s), ["A"]);
        assert!(s.revision() > r0);
        assert_eq!(s.history().line(), line, "the steps after it can be redone");
        s.redo().unwrap();
        assert_eq!(names(&s), ["A", "B"]);
        s.goto(line[4]).unwrap();
        assert_eq!(names(&s), ["A", "B", "C", "D"]);
        s.goto(line[0]).unwrap();
        assert!(s.project.tracks.is_empty());
        assert!(!s.undo().unwrap());
        assert!(s.goto(999).is_err());
        assert_eq!(s.project_at(line[2]).unwrap().tracks.len(), 2);
        assert!(s.project.tracks.is_empty(), "project_at leaves the project alone");
    }

    #[test]
    fn checkpoints_name_states() {
        let mut s = Session::default();
        s.execute(add("A"), false).unwrap();
        let a = s.add_checkpoint(None).unwrap();
        s.execute(add("B"), false).unwrap();
        let b = s.add_checkpoint(Some("  Refrain fertig ".into())).unwrap();
        let cps = s.history().checkpoints();
        assert_eq!(cps.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["Checkpoint 1", "Refrain fertig"]);
        assert_eq!(cps[1].node, s.history().current());
        s.goto(cps[0].node).unwrap();
        assert_eq!(names(&s), ["A"]);
        s.rename_checkpoint(a, "Intro").unwrap();
        assert!(s.rename_checkpoint(a, " ").is_err());
        s.remove_checkpoint(b).unwrap();
        assert!(s.remove_checkpoint(b).is_err());
        assert_eq!(s.history().checkpoints()[0].name, "Intro");
    }

    #[test]
    fn a_slider_gesture_never_changes_a_checkpointed_step() {
        let mut s = Session::default();
        s.execute(add("A"), false).unwrap();
        let tid = s.project.tracks[0].id;
        s.execute(gain(tid, -1.0), false).unwrap();
        s.execute(gain(tid, -2.0), true).unwrap();
        assert_eq!(s.history().len(), 3, "one step for the gesture");
        s.add_checkpoint(None).unwrap();
        s.execute(gain(tid, -5.0), true).unwrap();
        assert_eq!(s.history().len(), 4, "the checkpoint keeps -2 dB");
        let cp = s.history().checkpoints()[0].node;
        assert_eq!(s.project_at(cp).unwrap().tracks[0].gain_db, -2.0);
        // after an undo a gesture value never merges into an older step
        s.undo().unwrap();
        s.execute(gain(tid, -7.0), true).unwrap();
        s.undo().unwrap();
        assert_eq!(s.project.tracks[0].gain_db, -2.0);
    }

    #[test]
    fn variants_branch_switch_rename_and_delete() {
        let mut s = Session::default();
        s.execute(add("Lead"), false).unwrap();
        let base = s.history().current();
        s.execute(add("Bass"), false).unwrap();
        let main = s.history().active().id;
        assert_eq!(s.history().active().name, "Main version");
        // a variant from the earlier state
        let v = s.create_variant(Some("Ohne Bass".into()), Some(base)).unwrap();
        assert_eq!(names(&s), ["Lead"]);
        s.execute(add("Pad"), false).unwrap();
        assert_eq!(names(&s), ["Lead", "Pad"]);
        assert_eq!(s.history().fork_of(v), base);
        assert_eq!(s.history().variants_through(base).len(), 2);
        s.switch_variant(main).unwrap();
        assert_eq!(names(&s), ["Lead", "Bass"]);
        // undo/redo stay on the active variant
        s.undo().unwrap();
        s.redo().unwrap();
        assert_eq!(names(&s), ["Lead", "Bass"]);
        s.switch_variant(v).unwrap();
        assert_eq!(names(&s), ["Lead", "Pad"]);
        s.rename_variant(v, "Mit Pad").unwrap();
        assert!(s.create_variant(Some("".into()), None).is_err());
        // deleting the active variant goes back to the one it came from; its own steps are gone
        let before = s.history().len();
        s.delete_variant(v).unwrap();
        assert_eq!(names(&s), ["Lead", "Bass"]);
        assert_eq!(s.history().len(), before - 1);
        assert!(s.delete_variant(main).is_err(), "the last variant stays");
    }

    #[test]
    fn agent_batch_inside_a_variant() {
        let mut s = Session::default();
        s.execute(add("A"), false).unwrap();
        let v = s.create_variant(None, None).unwrap();
        let mark = s.mark();
        s.execute(add("B"), false).unwrap();
        s.execute(add("C"), false).unwrap();
        s.squash(&mark, "Agent".into());
        assert_eq!(s.history().active().id, v);
        assert_eq!(s.history().active().head, s.history().current());
        assert_eq!(s.undo_label(), Some("Agent"));
        let mark = s.mark();
        s.execute(add("D"), false).unwrap();
        s.rollback(mark).unwrap();
        assert_eq!(names(&s), ["A", "B", "C"]);
        assert_eq!(s.history().active().head, s.history().current());
    }

    #[test]
    fn history_file_roundtrip_with_variants_and_checkpoints() {
        let mut s = Session::with_label(Project::default(), "Geöffnet".into());
        let (tid, cid) = lead_with_notes(&mut s, vec![Note::new(60, 0.0, 1.0, 90)]);
        s.add_checkpoint(Some("Thema".into())).unwrap();
        s.execute(Command::SetClipNotes { track_id: tid, clip_id: cid, notes: vec![Note::new(62, 0.0, 1.0, 90)], length_beats: None }, false).unwrap();
        let first = s.history().line()[1];
        s.create_variant(Some("B".into()), Some(first)).unwrap();
        s.execute(gain(tid, -6.0), false).unwrap();
        s.undo().unwrap();
        s.execute(add("Pad"), false).unwrap();
        let json = serde_json::to_string(&s.history_file().unwrap()).unwrap();
        let file: HistoryFile = serde_json::from_str(&json).unwrap();
        let back = Session::restore(s.project.clone(), file.clone()).unwrap();
        assert_eq!(back.project, s.project);
        assert_eq!(back.history().len(), s.history().len());
        assert_eq!(back.history().line(), s.history().line());
        assert_eq!(back.history().variants(), s.history().variants());
        assert_eq!(back.history().checkpoints(), s.history().checkpoints());
        for (a, b) in s.history().nodes().zip(back.history().nodes()) {
            assert_eq!((a.id, &a.label, &a.metrics, &a.scope, &a.children), (b.id, &b.label, &b.metrics, &b.scope, &b.children));
        }
        // the restored one works like the original
        let mut back = back;
        let main = back.history().variants()[0].id;
        back.switch_variant(main).unwrap();
        assert_eq!(back.project.tracks.len(), 1);
        back.undo().unwrap();
        // a project that does not fit the history is refused
        let mut other = s.project.clone();
        other.tempo_bpm = 99.0;
        assert!(Session::restore(other, file).is_err());
    }

    #[test]
    fn old_abandoned_branches_go_first_when_the_history_is_full() {
        let mut s = Session::default();
        s.history.limit = 6;
        s.execute(add("A"), false).unwrap();
        s.execute(add("B"), false).unwrap();
        s.undo().unwrap();
        s.execute(add("C"), false).unwrap(); // B is abandoned now
        s.add_checkpoint(None).unwrap();
        for n in ["D", "E", "F", "G"] {
            s.execute(add(n), false).unwrap();
        }
        let h = s.history();
        assert!(h.len() <= 6, "{}", h.len());
        assert!(!h.nodes().any(|n| n.scope.tracks.iter().any(|t| t.name == "B")), "the abandoned step went first");
        let cp = h.checkpoints()[0].node;
        assert!(h.node(cp).is_some(), "the checkpoint stays");
        assert_eq!(names(&s), ["A", "C", "D", "E", "F", "G"]);
        // undo works down to the oldest step that is left
        while s.undo().unwrap() {}
        assert!(s.project.tracks.len() <= 2);
    }

    #[test]
    fn clear_history_keeps_the_project() {
        let mut s = Session::default();
        s.execute(add("A"), false).unwrap();
        s.create_variant(Some("Zweite".into()), None).unwrap();
        s.add_checkpoint(None).unwrap();
        s.clear_history("Verlauf geleert".into());
        assert_eq!(s.history().len(), 1);
        assert_eq!(names(&s), ["A"]);
        assert_eq!(s.history().variants().len(), 1);
        assert_eq!(s.history().active().name, "Zweite");
        assert!(s.history().checkpoints().is_empty());
        assert!(!s.undo().unwrap());
    }
}

#[cfg(test)]
mod merge_tests {
    use super::*;
    use crate::merge::{self, Choices, SegmentChoice};
    use crate::{Clip, ClipSource, Id, Note, Track, TrackKind};

    fn notes_of(s: &Session) -> Vec<u8> {
        match &s.project.tracks[0].clips[0].source {
            ClipSource::Notes { notes, .. } => notes.iter().map(|n| n.pitch).collect(),
            _ => vec![],
        }
    }

    #[test]
    fn two_variants_combined_into_a_new_one_with_one_step() {
        let mut s = Session::default();
        let mut lead = Track::new(Id::NEW, "Lead", TrackKind::Instrument);
        lead.clips.push(Clip { id: Id::NEW, name: "A".into(), start_beats: 0.0, source: ClipSource::Notes { length_beats: 32.0, notes: (0..8).map(|i| Note::new(60, i as f64 * 4.0, 1.0, 90)).collect() } });
        s.execute(Command::AddTrack { track: lead, index: None }, false).unwrap();
        let (tid, cid) = (s.project.tracks[0].id, s.project.tracks[0].clips[0].id);
        let set = |s: &mut Session, f: &dyn Fn(&mut Vec<Note>)| {
            let ClipSource::Notes { notes, .. } = &s.project.tracks[0].clips[0].source else { unreachable!() };
            let mut n = notes.clone();
            f(&mut n);
            s.execute(Command::SetClipNotes { track_id: tid, clip_id: cid, notes: n, length_beats: None }, false).unwrap();
        };
        let base = s.history().current();
        let main = s.history().active().id;
        set(&mut s, &|n| n[0].pitch = 62); // main: bar 1
        let b = s.create_variant(Some("B".into()), Some(base)).unwrap();
        set(&mut s, &|n| n[7].pitch = 67); // B: bar 8
        s.switch_variant(main).unwrap();
        let src = s.merge_sources(&[main, b]).unwrap();
        assert_eq!(src.origin, base);
        let v = src.versions();
        let plan = merge::plan(&v, main);
        let segs: Vec<(f64, Vec<u32>)> = plan.tracks[0].segments.iter().map(|x| (x.start, x.by.clone())).collect();
        assert_eq!(segs, vec![(0.0, vec![main]), (28.0, vec![b])]);
        let result = merge::combine(&v, &plan, &Choices::default()).unwrap();
        let before = s.history().len();
        let id = s.add_combined(main, &result, Some("Main + B".into()), "Combined: B".into()).unwrap();
        assert_eq!(s.history().active().id, id);
        assert_eq!(s.history().len(), before + 1, "one step");
        assert_eq!(s.undo_label(), Some("Combined: B"));
        assert_eq!(notes_of(&s), vec![62, 60, 60, 60, 60, 60, 60, 67]);
        assert_eq!(s.history().variants().len(), 3);
        // undo goes back to the main version's state, the others are untouched
        s.undo().unwrap();
        assert_eq!(notes_of(&s)[7], 60);
        s.switch_variant(b).unwrap();
        assert_eq!(notes_of(&s), vec![60, 60, 60, 60, 60, 60, 60, 67]);
        // taking the origin for bar 1 drops the main version's change
        let only_b = Choices { segments: vec![SegmentChoice { key: plan.tracks[0].key.clone(), start: 0.0, source: merge::ORIGIN }], ..Default::default() };
        let r = merge::combine(&v, &plan, &only_b).unwrap();
        let id2 = s.add_combined(main, &r, None, "Combined".into()).unwrap();
        assert_eq!(s.history().active().name, "Variant 4");
        assert_eq!(s.history().active().id, id2);
        assert_eq!(notes_of(&s), vec![60, 60, 60, 60, 60, 60, 60, 67]);
        assert!(s.merge_sources(&[main]).is_err());
    }
}
