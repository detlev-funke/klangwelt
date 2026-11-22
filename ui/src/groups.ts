// Track groups in the track list: picking several tracks, grouping them, moving
// tracks into and out of groups (also by dragging), and the context menus of
// tracks, groups and the empty list. The core keeps the groups (project.groups,
// track.group); a group shows where its first track is, its tracks follow it.

import { addTrack } from "./actions.js";
import { type MenuEntry, openContextMenu } from "./ctxmenu.js";
import { t } from "./i18n.js";
import { layoutRows } from "./pianoroll.js";
import { signal } from "./signals.js";
import { project, run, selection } from "./store.js";
import { showPan, showPitch, toggleShowPan, toggleShowPitch } from "./trackview.js";
import type { Command, Project, Track, TrackGroup } from "./types.js";

export const GROUP_COLORS = ["#ffb454", "#6ee7d8", "#a78bfa", "#f472b6", "#60a5fa", "#4ade80", "#fb923c", "#facc15"];

/** Tracks picked with Ctrl/Shift+click in the track list (for "Group"); the selected track counts too. */
export const picked = signal<number[]>([]);

/** Track rows in the order of the list (folded groups included). */
function listOrder(p: Project): Track[] {
  return layoutRows({ tracks: p.tracks, groups: (p.groups ?? []).map((g) => ({ ...g, collapsed: false })) }).flatMap((r) => (r.kind === "track" ? [r.track] : []));
}

/** Click on a track header: alone, Ctrl = add/remove, Shift = range from the last pick. */
export function pickTrack(id: number, e: { ctrlKey: boolean; metaKey: boolean; shiftKey: boolean }): void {
  const p = project();
  const cur = picked.peek();
  if (e.ctrlKey || e.metaKey) {
    picked.value = cur.includes(id) ? cur.filter((x) => x !== id) : [...cur, id];
  } else if (e.shiftKey && p && cur.length) {
    const order = listOrder(p).map((t) => t.id);
    const a = order.indexOf(cur[cur.length - 1]!);
    const b = order.indexOf(id);
    const [lo, hi] = a < b ? [a, b] : [b, a];
    const range = order.slice(lo, hi + 1);
    picked.value = [...cur.filter((x) => !range.includes(x)), ...range];
  } else {
    picked.value = [id];
  }
}

/** The picked tracks in list order; without any, the selected track. */
export function pickedTracks(): Track[] {
  const p = project();
  if (!p) return [];
  const ids = new Set(picked.peek());
  const sel = selection.peek()?.trackId;
  if (!ids.size && sel) ids.add(sel);
  return listOrder(p).filter((t) => ids.has(t.id));
}

function nextGroupName(p: Project): string {
  const names = new Set((p.groups ?? []).map((g) => g.name));
  for (let n = 1; ; n++) {
    const name = t("Group {n}", { n });
    if (!names.has(name)) return name;
  }
}

/** Moves that put `ids` one after another, starting where the first one is (indices as the core applies them). */
function gatherMoves(order: number[], ids: number[]): Command[] {
  const cmds: Command[] = [];
  const cur = [...order];
  for (let k = 1; k < ids.length; k++) {
    const id = ids[k]!;
    const from = cur.indexOf(id);
    cur.splice(from, 1);
    const at = cur.indexOf(ids[k - 1]!) + 1;
    cur.splice(at, 0, id);
    if (at !== from) cmds.push({ type: "moveTrack", trackId: id, index: at });
  }
  return cmds;
}

/** Puts the tracks into a new group (they move together where the first one is); none: an empty group. */
export async function groupTracks(tracks: Track[] = pickedTracks()): Promise<number | null> {
  const p = project();
  if (!p) return null;
  const id = p.nextId;
  const color = GROUP_COLORS[(p.groups?.length ?? 0) % GROUP_COLORS.length]!;
  const ids = tracks.map((tr) => tr.id);
  const cmds: Command[] = [
    { type: "addGroup", group: { id, name: nextGroupName(p), color } },
    ...ids.map((trackId): Command => ({ type: "setTrack", trackId, patch: { group: id } })),
    ...gatherMoves(p.tracks.map((tr) => tr.id), ids),
  ];
  const ok = await run({ type: "batch", label: tracks.length ? t("Group tracks") : t("Add group"), commands: cmds });
  if (ok) picked.value = [];
  return ok ? id : null;
}

export function ungroup(g: TrackGroup): Promise<boolean> {
  return run({ type: "removeGroup", groupId: g.id });
}

export function deleteGroupWithTracks(g: TrackGroup): Promise<boolean> {
  const p = project();
  const members = p?.tracks.filter((tr) => tr.group === g.id) ?? [];
  return run({
    type: "batch",
    label: t("Delete group"),
    commands: [...members.map((tr): Command => ({ type: "removeTrack", trackId: tr.id })), { type: "removeGroup", groupId: g.id }],
  });
}

export function setGroup(g: TrackGroup, patch: Partial<Omit<TrackGroup, "id">>, merge = false): Promise<boolean> {
  return run({ type: "setGroup", groupId: g.id, patch }, merge);
}

/** Where a dragged track lands: before a track (or at the end) and in which group (0 = none). */
export interface DropPlace {
  /** Track it goes in front of; null = after `after` (or at the very end). */
  before: number | null;
  after: number | null;
  group: number;
}

/**
 * Moves a track to a place in the list, joining or leaving groups on the way.
 * One undo step; nothing happens if it stays where it is.
 */
export function moveTrackTo(trackId: number, place: DropPlace): Promise<boolean> | null {
  const p = project();
  const track = p?.tracks.find((tr) => tr.id === trackId);
  if (!p || !track) return null;
  const order = p.tracks.map((tr) => tr.id).filter((x) => x !== trackId);
  let index = order.length;
  if (place.before !== null && place.before !== trackId) index = order.indexOf(place.before);
  else if (place.after !== null && place.after !== trackId) index = order.indexOf(place.after) + 1;
  else if (place.before === trackId || place.after === trackId) index = p.tracks.findIndex((tr) => tr.id === trackId);
  const cmds: Command[] = [];
  if ((track.group ?? 0) !== place.group) cmds.push({ type: "setTrack", trackId, patch: { group: place.group } });
  if (index !== p.tracks.findIndex((tr) => tr.id === trackId)) cmds.push({ type: "moveTrack", trackId, index });
  if (!cmds.length) return null;
  return run({ type: "batch", label: t("Move track"), commands: cmds });
}

/** Into a group (at its end) or out of it (right after it). */
export function moveTrackToGroup(track: Track, groupId: number): Promise<boolean> | null {
  const p = project();
  if (!p) return null;
  if (groupId) {
    const members = p.tracks.filter((tr) => tr.group === groupId && tr.id !== track.id);
    return moveTrackTo(track.id, { before: null, after: members.at(-1)?.id ?? null, group: groupId });
  }
  const members = p.tracks.filter((tr) => tr.group === track.group);
  return moveTrackTo(track.id, { before: null, after: members.at(-1)?.id ?? null, group: 0 });
}

/** Color chips of a context menu. */
function colorChips(current: string, set: (c: string) => void): MenuEntry {
  return {
    label: t("Color"),
    act: "farbe",
    chips: GROUP_COLORS.map((c, i) => ({ label: "●", act: `farbe-${i}`, title: c, on: c === current, action: () => set(c) })),
  };
}

function viewEntries(): MenuEntry[] {
  return [
    { heading: t("Show in the track headers") },
    { label: t("Pan"), act: "zeige-pan", checked: showPan.peek(), action: toggleShowPan },
    { label: t("Pitch"), act: "zeige-pitch", checked: showPitch.peek(), action: toggleShowPitch },
  ];
}

/** Right-click on a track header. */
export function trackMenu(x: number, y: number, track: Track, rename: () => void): void {
  const p = project();
  if (!p) return;
  if (!picked.peek().includes(track.id)) picked.value = [track.id];
  const many = pickedTracks();
  const groups = p.groups ?? [];
  const entries: MenuEntry[] = [
    { heading: many.length > 1 ? t("{n} tracks", { n: many.length }) : track.name },
    { label: t("Rename"), act: "umbenennen", action: rename },
    colorChips(track.color, (color) => void run({ type: "setTrack", trackId: track.id, patch: { color } })),
    "sep",
    { label: many.length > 1 ? t("Group {n} tracks", { n: many.length }) : t("Put in a new group"), hint: "Ctrl+G", act: "gruppieren", action: () => void groupTracks(many) },
    ...groups.filter((g) => g.id !== track.group).map((g): MenuEntry => ({ label: t("Move to “{name}”", { name: g.name }), act: `in-gruppe-${g.id}`, action: () => void moveTrackToGroup(track, g.id) })),
  ];
  if (track.group) entries.push({ label: t("Take out of the group"), act: "aus-gruppe", action: () => void moveTrackToGroup(track, 0) });
  entries.push(
    "sep",
    { label: track.view === "notes" ? t("Compact view") : track.kind === "instrument" ? t("Piano roll (editable)") : t("Show pitches (from the analysis)"), act: "ansicht", action: () => void run({ type: "setTrack", trackId: track.id, patch: { view: track.view === "notes" ? "compact" : "notes" } }) },
    { label: track.mute ? t("Unmute") : t("Mute"), act: "stumm", action: () => void run({ type: "setTrack", trackId: track.id, patch: { mute: !track.mute } }) },
    { label: track.solo ? t("Solo off") : t("Solo"), act: "solo", action: () => void run({ type: "setTrack", trackId: track.id, patch: { solo: !track.solo } }) },
    "sep",
    ...viewEntries(),
    "sep",
    {
      label: many.length > 1 ? t("Delete {n} tracks", { n: many.length }) : t("Delete track"),
      act: "loeschen",
      action: () => void (many.length > 1
        ? run({ type: "batch", label: t("Delete tracks"), commands: many.map((tr): Command => ({ type: "removeTrack", trackId: tr.id })) })
        : run({ type: "removeTrack", trackId: track.id })),
    },
  );
  openContextMenu(x, y, entries);
}

/** Right-click on a group's row. */
export function groupMenu(x: number, y: number, g: TrackGroup, rename: () => void): void {
  openContextMenu(x, y, [
    { heading: g.name },
    { label: t("Rename"), act: "umbenennen", action: rename },
    colorChips(g.color, (color) => void setGroup(g, { color })),
    { label: g.collapsed ? t("Unfold") : t("Fold"), act: "falten", action: () => void setGroup(g, { collapsed: !g.collapsed }) },
    "sep",
    { label: g.mute ? t("Unmute") : t("Mute"), act: "stumm", action: () => void setGroup(g, { mute: !g.mute }) },
    { label: g.solo ? t("Solo off") : t("Solo"), act: "solo", action: () => void setGroup(g, { solo: !g.solo }) },
    { label: t("Volume to 0 dB"), act: "pegel-0", disabled: g.gainDb === 0, action: () => void setGroup(g, { gainDb: 0 }) },
    "sep",
    { label: t("Ungroup"), title: t("The tracks stay, without the group"), act: "aufloesen", action: () => void ungroup(g) },
    { label: t("Delete group with its tracks"), act: "loeschen", action: () => void deleteGroupWithTracks(g) },
  ]);
}

/** Right-click on the empty part of the track list. */
export function listMenu(x: number, y: number): void {
  const n = pickedTracks().length;
  openContextMenu(x, y, [
    { label: t("Add audio track"), act: "neue-audiospur", action: () => addTrack("audio") },
    { label: t("Add instrument track"), act: "neue-instrumentspur", action: () => addTrack("instrument") },
    { label: n > 1 ? t("Group {n} tracks", { n }) : n ? t("Put in a new group") : t("Add group"), hint: "Ctrl+G", act: "gruppieren", action: () => void groupTracks() },
    "sep",
    ...viewEntries(),
  ]);
}
