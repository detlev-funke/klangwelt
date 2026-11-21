//! The change history as a tree.
//!
//! Every step is a node: the command that led there from its parent
//! (`forward`), the command that leads back (`inverse`), when it happened, a
//! label, the project's key figures afterwards ([`Metrics`]) and where it
//! changed something ([`Scope`]). Undo goes to the parent, redo to a child;
//! editing after an undo starts a new branch – the old steps stay in the tree.
//!
//! * **Checkpoints** are named nodes.
//! * **Variants** are named branches: a variant points to its newest step
//!   (`head`); the active variant's head moves along with every new step. The
//!   line of a variant is the path from the first state to its head.
//!
//! [`Session`](crate::Session) applies the commands; this module holds the
//! tree, the navigation along it and the file format.

use crate::command::Command;
use crate::diff::{diff_track, span_of, Range};
use crate::metrics::Metrics;
use crate::model::{Id, Project, Track};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::RwLock;

pub type NodeId = u32;

static CLOCK: RwLock<Option<fn() -> u64>> = RwLock::new(None);

/// Sets the clock for the time of the steps (Unix ms). The core has none of its
/// own (in the browser `std::time` doesn't exist) – the app sets it at start.
pub fn set_clock(now_ms: fn() -> u64) {
    *CLOCK.write().unwrap_or_else(|e| e.into_inner()) = Some(now_ms);
}

pub(crate) fn now() -> u64 {
    CLOCK.read().unwrap_or_else(|e| e.into_inner()).map(|f| f()).unwrap_or(0)
}

/// A track a step changed, with its name at that time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopeTrack {
    pub id: Id,
    pub name: String,
}

/// Where a step changed something.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Scope {
    pub tracks: Vec<ScopeTrack>,
    /// Changed time range in beats (all tracks together).
    pub range: Option<Range>,
    /// Name, tempo or time signature of the project changed.
    pub project: bool,
    /// The sound of a track changed.
    pub sound: bool,
    pub notes_added: u32,
    pub notes_removed: u32,
}

impl Scope {
    /// Both together (steps squashed into one).
    fn join(mut self, other: &Scope) -> Scope {
        for t in &other.tracks {
            if !self.tracks.iter().any(|x| x.id == t.id) {
                self.tracks.push(t.clone());
            }
        }
        self.range = span_of(self.range.into_iter().chain(other.range));
        self.project |= other.project;
        self.sound |= other.sound;
        self.notes_added += other.notes_added;
        self.notes_removed += other.notes_removed;
        self
    }
}

/// What a command will touch, taken before it runs – afterwards
/// [`Before::scope`] tells what changed.
#[derive(Clone, Debug)]
pub(crate) struct Before {
    tracks: Vec<(Id, Option<Track>)>,
    header: (String, f64, u32, u32),
}

impl Before {
    pub(crate) fn of(p: &Project, cmd: &Command) -> Self {
        let mut ids = Vec::new();
        cmd.touched_tracks(&mut ids);
        Before {
            tracks: ids.into_iter().map(|id| (id, p.track(id).cloned())).collect(),
            header: header(p),
        }
    }

    pub(crate) fn scope(&self, after: &Project) -> Scope {
        let mut s = Scope { project: self.header != header(after), ..Default::default() };
        let mut ranges = Vec::new();
        for (id, old) in &self.tracks {
            let new = after.track(*id);
            let Some(name) = new.or(old.as_ref()).map(|t| t.name.clone()) else { continue };
            if let Some(d) = diff_track(old.as_ref(), new, self.header.1, after.tempo_bpm) {
                ranges.extend(d.ranges.iter().copied());
                s.sound |= d.sound;
                s.notes_added += d.notes_added;
                s.notes_removed += d.notes_removed;
            }
            s.tracks.push(ScopeTrack { id: *id, name });
        }
        s.range = span_of(ranges);
        s
    }
}

fn header(p: &Project) -> (String, f64, u32, u32) {
    (p.name.clone(), p.tempo_bpm, p.beats_per_bar, p.beat_unit)
}

/// One step in the history.
#[derive(Clone, Debug)]
pub struct Node {
    pub id: NodeId,
    pub parent: Option<NodeId>,
    /// In the order they were created.
    pub children: Vec<NodeId>,
    /// Child that redo goes to when no variant says otherwise (the one visited last).
    pub redo: Option<NodeId>,
    pub label: String,
    /// Unix ms (0 without a clock).
    pub time: u64,
    pub(crate) forward: Option<Command>,
    pub(crate) inverse: Option<Command>,
    pub(crate) merge_key: Option<String>,
    pub metrics: Metrics,
    pub scope: Scope,
}

/// A named state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Checkpoint {
    pub id: u32,
    pub name: String,
    pub node: NodeId,
    pub time: u64,
}

/// A named branch of the history; `head` is its newest step.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Variant {
    pub id: u32,
    pub name: String,
    pub head: NodeId,
    pub time: u64,
}

/// The tree of all steps with checkpoints and variants.
#[derive(Clone, Debug)]
pub struct History {
    pub(crate) nodes: BTreeMap<NodeId, Node>,
    pub(crate) root: NodeId,
    pub(crate) current: NodeId,
    pub(crate) next_node: NodeId,
    pub(crate) checkpoints: Vec<Checkpoint>,
    pub(crate) variants: Vec<Variant>,
    pub(crate) active: u32,
    /// Next ID for checkpoints and variants.
    pub(crate) next_mark: u32,
    /// More nodes than this: old, abandoned steps are dropped.
    pub(crate) limit: usize,
}

/// Steps kept at most (old abandoned branches go first, then the oldest steps).
pub const HISTORY_LIMIT: usize = 1000;

impl History {
    pub(crate) fn new(project: &Project, label: String, variant: String) -> Self {
        let time = now();
        let root = Node {
            id: 1,
            parent: None,
            children: Vec::new(),
            redo: None,
            label,
            time,
            forward: None,
            inverse: None,
            merge_key: None,
            metrics: Metrics::of(project),
            scope: Scope::default(),
        };
        History {
            nodes: BTreeMap::from([(1, root)]),
            root: 1,
            current: 1,
            next_node: 2,
            checkpoints: Vec::new(),
            variants: vec![Variant { id: 1, name: variant, head: 1, time }],
            active: 1,
            next_mark: 2,
            limit: HISTORY_LIMIT,
        }
    }

    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(&id)
    }

    pub(crate) fn n(&self, id: NodeId) -> &Node {
        &self.nodes[&id]
    }

    pub(crate) fn n_mut(&mut self, id: NodeId) -> &mut Node {
        self.nodes.get_mut(&id).expect("node exists")
    }

    pub fn current(&self) -> NodeId {
        self.current
    }

    pub fn root(&self) -> NodeId {
        self.root
    }

    /// Number of steps (nodes) in the tree, the first state included.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.len() <= 1
    }

    pub fn nodes(&self) -> impl Iterator<Item = &Node> {
        self.nodes.values()
    }

    pub fn checkpoints(&self) -> &[Checkpoint] {
        &self.checkpoints
    }

    pub fn variants(&self) -> &[Variant] {
        &self.variants
    }

    pub fn active(&self) -> &Variant {
        self.variants.iter().find(|v| v.id == self.active).unwrap_or(&self.variants[0])
    }

    pub(crate) fn active_mut(&mut self) -> &mut Variant {
        let id = self.active;
        let i = self.variants.iter().position(|v| v.id == id).unwrap_or(0);
        &mut self.variants[i]
    }

    pub fn variant(&self, id: u32) -> Option<&Variant> {
        self.variants.iter().find(|v| v.id == id)
    }

    /// `id`, its parent, … up to the first state.
    pub fn ancestors(&self, id: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut n = Some(id);
        while let Some(i) = n {
            out.push(i);
            n = self.nodes.get(&i).and_then(|x| x.parent);
        }
        out
    }

    pub fn depth(&self, id: NodeId) -> usize {
        self.ancestors(id).len() - 1
    }

    /// Is `a` the node `b` or one of its ancestors?
    pub fn is_ancestor(&self, a: NodeId, b: NodeId) -> bool {
        let mut n = Some(b);
        while let Some(i) = n {
            if i == a {
                return true;
            }
            n = self.nodes.get(&i).and_then(|x| x.parent);
        }
        false
    }

    /// Latest common ancestor of `a` and `b`.
    pub fn common_ancestor(&self, a: NodeId, b: NodeId) -> NodeId {
        let up: BTreeSet<NodeId> = self.ancestors(a).into_iter().collect();
        self.ancestors(b).into_iter().find(|n| up.contains(n)).unwrap_or(self.root)
    }

    /// The way from `from` to `to`: nodes to go back from (undo, starting at
    /// `from`) and nodes to go forward to (redo, ending at `to`).
    pub(crate) fn path(&self, from: NodeId, to: NodeId) -> (Vec<NodeId>, Vec<NodeId>) {
        let lca = self.common_ancestor(from, to);
        let up: Vec<NodeId> = self.ancestors(from).into_iter().take_while(|&n| n != lca).collect();
        let mut down: Vec<NodeId> = self.ancestors(to).into_iter().take_while(|&n| n != lca).collect();
        down.reverse();
        (up, down)
    }

    /// Where redo goes from the current step: along the active variant, otherwise
    /// to the child visited last.
    pub fn redo_target(&self) -> Option<NodeId> {
        let head = self.active().head;
        if head != self.current && self.is_ancestor(self.current, head) {
            let a = self.ancestors(head);
            let i = a.iter().position(|&n| n == self.current)?;
            return Some(a[i - 1]);
        }
        let n = self.n(self.current);
        n.redo.or_else(|| n.children.last().copied())
    }

    /// The active variant's line: from the first state through the current one
    /// up to the variant's newest step (the steps after the current one can be redone).
    pub fn line(&self) -> Vec<NodeId> {
        let head = self.active().head;
        let end = if self.is_ancestor(self.current, head) { head } else { self.current };
        let mut line = self.ancestors(end);
        line.reverse();
        line
    }

    /// The newest step of the branch that `id` is on (following redo).
    pub fn tip(&self, id: NodeId) -> NodeId {
        let mut n = id;
        loop {
            let node = self.n(n);
            match node.redo.or_else(|| node.children.last().copied()) {
                Some(c) => n = c,
                None => return n,
            }
        }
    }

    /// Where a variant leaves the others: the newest of its steps that is also
    /// on another variant's line (the first state if none).
    pub fn fork_of(&self, variant: u32) -> NodeId {
        let Some(v) = self.variant(variant) else { return self.root };
        let others: Vec<NodeId> = self.variants.iter().filter(|o| o.id != variant).map(|o| o.head).collect();
        self.ancestors(v.head)
            .into_iter()
            .find(|&n| others.iter().any(|&h| self.is_ancestor(n, h)))
            .unwrap_or(self.root)
    }

    /// Variants whose line passes through `id`.
    pub fn variants_through(&self, id: NodeId) -> Vec<&Variant> {
        self.variants.iter().filter(|v| self.is_ancestor(id, v.head)).collect()
    }

    /// Nodes that must stay: the lines to the current step, to every variant
    /// and to every checkpoint.
    pub(crate) fn alive(&self) -> BTreeSet<NodeId> {
        let mut keep = BTreeSet::new();
        let tips = std::iter::once(self.current)
            .chain(self.variants.iter().map(|v| v.head))
            .chain(self.checkpoints.iter().map(|c| c.node));
        for t in tips {
            for n in self.ancestors(t) {
                if !keep.insert(n) {
                    break;
                }
            }
        }
        keep
    }

    /// Appends a step below the current one; the active variant moves along.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn push(&mut self, label: String, forward: Command, inverse: Command, merge_key: Option<String>, metrics: Metrics, scope: Scope) -> NodeId {
        let id = self.next_node;
        self.next_node += 1;
        let parent = self.current;
        self.nodes.insert(id, Node {
            id,
            parent: Some(parent),
            children: Vec::new(),
            redo: None,
            label,
            time: now(),
            forward: Some(forward),
            inverse: Some(inverse),
            merge_key,
            metrics,
            scope,
        });
        let p = self.n_mut(parent);
        p.children.push(id);
        p.redo = Some(id);
        self.current = id;
        self.active_mut().head = id;
        id
    }

    /// Removes `id` with everything below it.
    pub(crate) fn remove_subtree(&mut self, id: NodeId) {
        let Some(node) = self.nodes.remove(&id) else { return };
        if let Some(p) = node.parent.and_then(|p| self.nodes.get_mut(&p)) {
            p.children.retain(|&c| c != id);
            if p.redo == Some(id) {
                p.redo = p.children.last().copied();
            }
        }
        for c in node.children {
            self.remove_subtree(c);
        }
        self.checkpoints.retain(|c| c.node != id);
    }

    /// Keeps the tree below the limit: first abandoned branches (oldest
    /// first), then the oldest steps – as long as nothing named depends on them.
    pub(crate) fn prune(&mut self) {
        if self.nodes.len() <= self.limit {
            return;
        }
        let alive = self.alive();
        while self.nodes.len() > self.limit {
            let leaf = self.nodes.values().filter(|n| n.children.is_empty() && !alive.contains(&n.id)).min_by_key(|n| (n.time, n.id)).map(|n| n.id);
            match leaf {
                Some(id) => self.remove_subtree(id),
                None => break,
            }
        }
        while self.nodes.len() > self.limit {
            let r = self.root;
            let n = self.n(r);
            let named = self.checkpoints.iter().any(|c| c.node == r) || self.variants.iter().any(|v| v.head == r);
            if n.children.len() != 1 || r == self.current || named {
                break;
            }
            let c = n.children[0];
            self.nodes.remove(&r);
            let cn = self.n_mut(c);
            cn.parent = None;
            cn.forward = None;
            cn.inverse = None;
            cn.merge_key = None;
            self.root = c;
        }
    }

    /// Squashes the chain `chain` (top to bottom, the first one's parent is
    /// `base`) into one new node; returns it.
    pub(crate) fn squash_chain(&mut self, base: NodeId, chain: &[NodeId], label: String) -> NodeId {
        let id = self.next_node;
        self.next_node += 1;
        let last = *chain.last().expect("chain not empty");
        let forwards: Vec<Command> = chain.iter().filter_map(|n| self.n(*n).forward.clone()).collect();
        let inverses: Vec<Command> = chain.iter().rev().filter_map(|n| self.n(*n).inverse.clone()).collect();
        let scope = chain.iter().fold(Scope::default(), |s, n| s.join(&self.n(*n).scope));
        let (children, redo, metrics) = {
            let l = self.n(last);
            (l.children.clone(), l.redo, l.metrics.clone())
        };
        // branches that started inside the chain go with it
        for w in chain.windows(2) {
            let side: Vec<NodeId> = self.n(w[0]).children.iter().copied().filter(|&c| c != w[1]).collect();
            for s in side {
                self.remove_subtree(s);
            }
        }
        for n in chain {
            self.nodes.remove(n);
        }
        for &c in &children {
            self.n_mut(c).parent = Some(id);
        }
        self.nodes.insert(id, Node {
            id,
            parent: Some(base),
            children,
            redo,
            label: label.clone(),
            time: now(),
            forward: Some(Command::Batch { label: label.clone(), commands: forwards }),
            inverse: Some(Command::Batch { label, commands: inverses }),
            merge_key: None,
            metrics,
            scope,
        });
        let first = chain[0];
        let b = self.n_mut(base);
        for c in &mut b.children {
            if *c == first {
                *c = id;
            }
        }
        if b.redo == Some(first) {
            b.redo = Some(id);
        }
        for v in &mut self.variants {
            if chain.contains(&v.head) {
                v.head = id;
            }
        }
        for c in &mut self.checkpoints {
            if chain.contains(&c.node) {
                c.node = id;
            }
        }
        if chain.contains(&self.current) {
            self.current = id;
        }
        id
    }

    /// A name that no variant has yet: "Variant 2", "Variant 3" …
    pub(crate) fn free_variant_name(&self, template: impl Fn(usize) -> String) -> String {
        (self.variants.len() + 1..).map(&template).find(|n| !self.variants.iter().any(|v| &v.name == n)).unwrap_or_default()
    }
}

// ---------------------------------------------------------------- File

/// The history as it is saved with the project: the first state, then for every
/// step only the command that led there. Key figures, scope and the way back
/// are rebuilt when it is loaded.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryFile {
    pub version: u32,
    /// The project in the oldest state that was kept.
    pub root: Project,
    /// Parents before their children; the first step is the root.
    pub steps: Vec<SavedStep>,
    pub current: NodeId,
    #[serde(default)]
    pub checkpoints: Vec<Checkpoint>,
    #[serde(default)]
    pub variants: Vec<Variant>,
    pub active: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedStep {
    pub id: NodeId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<NodeId>,
    pub label: String,
    #[serde(default)]
    pub time: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forward: Option<Command>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redo: Option<NodeId>,
}

pub const HISTORY_FILE_VERSION: u32 = 1;

impl History {
    /// All nodes, parents before children (depth first from the root).
    pub(crate) fn preorder(&self) -> Vec<NodeId> {
        let mut out = Vec::with_capacity(self.nodes.len());
        let mut stack = vec![self.root];
        while let Some(n) = stack.pop() {
            out.push(n);
            stack.extend(self.n(n).children.iter().rev().copied());
        }
        out
    }
}
