// Docking system for the UI's windows – without libraries.
//
// The layout is a tree of splits (row/column with size shares) and
// stacks (tab groups), plus free-floating windows. A tab is dragged
//  · onto a tab bar            → inserted there as a tab,
//  · onto the target cross of a stack → docked beside / above / below it,
//  · onto the markers at the edge → docked across the full width/height,
//  · anywhere else             → floating window.
// The empty area of a tab bar drags the whole group (for floating
// windows: the window). The contents are created only once and moved
// on rebuild, so they keep their state. The layout ends up in localStorage.
//
// Small screens (phones, narrow tablets) where the layout doesn't fit get a
// compact view instead, without touching the saved layout: the main window on
// top and all other windows as tabs below it ("sheet", with a divider), or –
// when it is too low for that – all windows as tabs ("single"). The Windows
// menu can switch back to the free layout (setFreeLayout).

import { h, icon } from "./dom.js";
import { t } from "./i18n.js";

export interface PanelDef {
  id: string;
  title: string;
  icon: string;
  /** false: cannot be closed (e.g. the tracks). */
  closable: boolean;
  /** Minimum size in px (default 180 × 110). */
  minWidth?: number;
  minHeight?: number;
  /** Place if the window is still missing from a saved layout (e.g. newly added). */
  place?: { near: string; zone: Zone };
  /** Keep closed in a saved layout where it is still missing (opens only on request). */
  startClosed?: boolean;
  /** Without a remembered place, open as a floating window – position from the size of the workspace. */
  float?: (w: number, h: number) => Box;
  create(): HTMLElement;
}

export type Dir = "row" | "col";
/** "dock": the layout as arranged; "sheet"/"single": compact view on small screens. */
export type DockMode = "dock" | "sheet" | "single";
export type Zone = "center" | "left" | "right" | "top" | "bottom";
export interface StackNode { kind: "stack"; panels: string[]; active: string }
export interface SplitNode { kind: "split"; dir: Dir; children: DockNode[]; sizes: number[] }
export type DockNode = StackNode | SplitNode;
export interface Box { x: number; y: number; w: number; h: number }
export interface FloatWin extends Box { stack: StackNode }
/**
 * Where a closed window returns to: to its tab neighbors
 * (`with`), beside another window (`near` + `zone`), at an edge of the
 * workspace (`edge`) or as a floating window (`float`).
 */
export interface Home { with: string[]; near?: string; zone?: Zone; edge?: Zone; float?: Box }
export interface Layout {
  root: DockNode | null;
  floats: FloatWin[];
  closed: string[];
  homes: Record<string, Home>;
}

export function stack(panels: string[], active?: string): StackNode {
  return { kind: "stack", panels, active: active ?? panels[0] ?? "" };
}

export function split(dir: Dir, children: DockNode[], sizes?: number[]): SplitNode {
  return { kind: "split", dir, children, sizes: sizes ?? children.map(() => 1 / children.length) };
}

type Target =
  | { kind: "tab"; stack: StackNode; index: number }
  | { kind: "stack"; stack: StackNode; zone: Zone }
  | { kind: "root"; zone: Zone }
  | { kind: "float"; x: number; y: number };

interface Payload {
  panels: string[];
  active: string;
  source: StackNode;
  /** A whole floating window is moved. */
  win: FloatWin | null;
  /** The whole group (all tabs of the stack) is dragged. */
  whole: boolean;
}

interface StackView { el: HTMLElement; tabs: HTMLElement; stack: StackNode; win: FloatWin | null }

const MIN_W = 180;
const MIN_H = 110;
const FLOAT_MIN_W = 240;
const FLOAT_MIN_H = 150;
const ROOT_SHARE = 0.28;
const DRAG_START = 5;
const ZONES: Zone[] = ["center", "left", "right", "top", "bottom"];
/** Small screen: narrower or lower than this (workspace in px) – only then the compact view may replace the layout. */
const SMALL_W = 900;
const SMALL_H = 500;
/** Compact view: at least this high for "main window above, tabs below", otherwise one window at a time. */
const SHEET_MIN_H = 500;
/** Compact view: smallest height of the two parts. */
const COMPACT_MIN_H = 130;
const COMPASS: Record<Zone, [number, number]> = { center: [0, 0], left: [-40, 0], right: [40, 0], top: [0, -40], bottom: [0, 40] };

const clamp = (v: number, lo: number, hi: number) => Math.max(lo, Math.min(hi, v));
const total = (xs: number[]) => xs.reduce((a, b) => a + b, 0);
const inside = (r: DOMRect, x: number, y: number) => x >= r.left && x < r.right && y >= r.top && y < r.bottom;
const num = (v: unknown, fallback: number) => (typeof v === "number" && Number.isFinite(v) ? v : fallback);
const isZone = (v: unknown): v is Zone => typeof v === "string" && (ZONES as string[]).includes(v);

/** Drag gesture via window listeners; Esc cancels (`up(null)`). */
function track(move: (e: PointerEvent) => void, up: (e: PointerEvent | null) => void): void {
  const done = (e: PointerEvent | null) => {
    window.removeEventListener("pointermove", move);
    window.removeEventListener("pointerup", onUp);
    window.removeEventListener("pointercancel", onCancel);
    window.removeEventListener("keydown", onKey, true);
    up(e);
  };
  const onUp = (e: PointerEvent) => done(e);
  const onCancel = () => done(null);
  const onKey = (e: KeyboardEvent) => {
    if (e.key !== "Escape") return;
    e.stopPropagation();
    e.preventDefault();
    done(null);
  };
  window.addEventListener("pointermove", move);
  window.addEventListener("pointerup", onUp);
  window.addEventListener("pointercancel", onCancel);
  window.addEventListener("keydown", onKey, true);
}

export interface DockOptions {
  panels: PanelDef[];
  defaults: () => Layout;
  storageKey: string;
  /** Called after every change with the visible (active) windows. */
  onVisible?: (ids: Set<string>) => void;
}

export class Dock {
  readonly #host: HTMLElement;
  readonly #defs = new Map<string, PanelDef>();
  readonly #order: string[];
  readonly #content = new Map<string, HTMLElement>();
  readonly #defaults: () => Layout;
  readonly #key: string;
  readonly #onVisible: (ids: Set<string>) => void;
  /** Neighbors in the default layout – return place if nothing else is known. */
  readonly #companions = new Map<string, string[]>();
  readonly #area = h("div", { class: "dk-area" });
  readonly #floatLayer = h("div", { class: "dk-floats" });
  readonly #overlay = h("div", { class: "dk-overlay" });
  readonly #preview = h("div", { class: "dk-preview" });
  readonly #insertMark = h("div", { class: "dk-insert" });
  readonly #compass = new Map<Zone, HTMLElement>();
  /** Stack under the target cross: its content fades so that the cross stands out. */
  #under: HTMLElement | null = null;
  readonly #rootMarks = new Map<Zone, HTMLElement>();
  #layout: Layout;
  #views: StackView[] = [];
  #floatEls = new Map<FloatWin, HTMLElement>();
  /** Temporarily enlarged splits and their actual sizes. */
  #emph: { split: SplitNode; sizes: number[] }[] = [];
  #badges = new Set<string>();
  #ghost: HTMLElement | null = null;
  #mode: DockMode = "dock";
  /** Gutter width and margin around the layout in px (--dk-gutter, --dk-inset; density). */
  #gap = 5;
  #inset = 5;
  /** Small screen, but the free layout anyway (Windows menu). */
  #free = false;
  /** Stacks of the compact view – kept, so that the chosen tabs and the divider stay. */
  readonly #compact = { main: stack([]), sheet: stack([]), single: stack([]), split: split("col", [], [0.5, 0.5]) };

  constructor(host: HTMLElement, o: DockOptions) {
    this.#host = host;
    for (const p of o.panels) this.#defs.set(p.id, p);
    this.#order = o.panels.map((p) => p.id);
    this.#defaults = o.defaults;
    this.#key = o.storageKey;
    this.#onVisible = o.onVisible ?? (() => {});

    const d = o.defaults();
    const walk = (n: DockNode | null): void => {
      if (!n) return;
      if (n.kind === "split") return n.children.forEach(walk);
      for (const p of n.panels) this.#companions.set(p, n.panels.filter((q) => q !== p));
    };
    walk(d.root);
    for (const f of d.floats) walk(f.stack);

    for (const z of ZONES) {
      const c = h("div", { class: "dk-target", title: "" });
      c.dataset.zone = z;
      this.#compass.set(z, c);
      const r = h("div", { class: "dk-target root" });
      r.dataset.zone = z;
      this.#rootMarks.set(z, r);
    }
    this.#overlay.append(this.#preview, this.#insertMark, ...this.#compass.values(), ...this.#rootMarks.values());
    document.body.append(this.#overlay);

    host.classList.add("dk-host");
    this.#readSpacing();
    host.replaceChildren(this.#area, this.#floatLayer);
    this.#layout = this.#load() ?? o.defaults();
    this.#loadCompact();
    this.#ensureAll();
    this.#normalize();
    this.#render();
    window.addEventListener("resize", () => {
      for (const [w, el] of this.#floatEls) this.#place(el, w);
    });
    // phone turned, window resized: compact view on or off
    new ResizeObserver(() => {
      if (this.#wantMode() !== this.#mode) this.#render();
    }).observe(host);
  }

  // ------------------------------------------------------------ public

  /** The spacing changed (gear menu: tight, normal, airy): measure gutters and margins anew,
   *  then the compact view on or off and the floating windows back inside. */
  refit(): void {
    this.#readSpacing();
    if (this.#wantMode() !== this.#mode) this.#render();
    for (const [w, el] of this.#floatEls) this.#place(el, w);
  }

  /** Gutter width and margin around the layout from the stylesheet (--dk-gutter, --dk-inset). */
  #readSpacing(): void {
    const cs = getComputedStyle(this.#host);
    const px = (name: string, fallback: number) => {
      const v = parseFloat(cs.getPropertyValue(name));
      return Number.isFinite(v) ? v : fallback;
    };
    this.#gap = px("--dk-gutter", 5);
    this.#inset = px("--dk-inset", 5);
  }

  /** Show window: open (if closed), activate tab, bring to front. */
  show(id: string): void {
    if (!this.#defs.has(id)) return;
    if (!this.#where(id)) {
      this.#reopen(id);
      this.#normalize();
      this.#render();
    }
    const w = this.#where(id);
    if (!w) return;
    const shown = this.#mode === "dock" ? null : this.#views.find((v) => v.stack.panels.includes(id));
    if (shown) this.#activate(shown.stack, id);
    else {
      this.#activate(w.stack, id);
      if (w.win) this.#raise(w.win);
    }
    for (const t of this.#tabEls(id)) {
      t.classList.remove("flash");
      void t.offsetWidth;
      t.classList.add("flash");
    }
  }

  close(id: string): void {
    if (!this.#defs.get(id)?.closable) return;
    const w = this.#where(id);
    if (!w) return;
    const L = this.#layout;
    const home: Home = { with: w.stack.panels.filter((p) => p !== id) };
    if (!home.with.length) {
      if (w.win) home.float = { x: w.win.x, y: w.win.y, w: w.win.w, h: w.win.h };
      else Object.assign(home, this.#placeOf(w.stack));
    }
    L.homes[id] = home;
    if (!L.closed.includes(id)) L.closed.push(id);
    this.#detach([id]);
    this.#normalize();
    this.#render();
  }

  toggle(id: string): void {
    if (this.isOpen(id)) this.close(id);
    else this.show(id);
  }

  isOpen(id: string): boolean {
    return !!this.#where(id);
  }

  reset(): void {
    this.#layout = this.#defaults();
    this.#compact.split.sizes = [0.5, 0.5];
    this.#ensureAll();
    this.#normalize();
    this.#render();
  }

  list(): Array<{ id: string; title: string; icon: string; open: boolean; closable: boolean }> {
    return this.#order.map((id) => {
      const d = this.#defs.get(id)!;
      return { id, title: d.title, icon: d.icon, open: this.isOpen(id), closable: d.closable };
    });
  }

  /**
   * Temporarily show a window larger (e.g. in the intro tour). Height: its
   * space in the nearest vertical split grows to at least `share` –
   * for a small window more, so that it becomes about `minHeight` px tall (at most
   * 80 %). Width (only with `minWidth`): likewise in the nearest horizontal
   * split (at most 65 %). null restores the old sizes.
   * The actual sizes are always what gets saved.
   */
  emphasize(id: string | null, o: { share?: number; minHeight?: number; minWidth?: number } = {}): void {
    const old = this.#emph;
    this.#emph = [];
    for (const e of [...old].reverse()) if (this.#contains(e.split)) e.split.sizes = e.sizes;
    if (this.#mode !== "dock") {
      // compact view: every window gets the full width anyway
      if (old.length) this.#render();
      return;
    }
    const w = id ? this.#where(id) : null;
    const grow = (dir: Dir, share: number) => {
      // nearest split of this direction above the stack – it may also sit in a
      // split of the other direction (then that one grows as a whole)
      for (let node: DockNode | null = w && !w.win ? w.stack : null; node; ) {
        const up = this.#parentOf(node);
        if (!up) return;
        if (up.parent.dir === dir) {
          const { parent, index } = up;
          const t = total(parent.sizes) || 1;
          const own = parent.sizes[index] ?? 0;
          if (own / t >= share) return;
          this.#emph.push({ split: parent, sizes: [...parent.sizes] });
          const rest = t - own || 1;
          parent.sizes = parent.sizes.map((s, i) => (i === index ? share : (s / rest) * (1 - share)));
          return;
        }
        node = up.parent;
      }
    };
    if (w && !w.win) {
      const H = Math.max(1, this.#host.clientHeight);
      const W = Math.max(1, this.#host.clientWidth);
      grow("col", Math.min(0.8, Math.max(o.share ?? 0.62, (o.minHeight ?? 640) / H)));
      if (o.minWidth) grow("row", Math.min(0.65, o.minWidth / W));
    }
    if (old.length || this.#emph.length) this.#render();
  }

  #contains(node: DockNode): boolean {
    const walk = (n: DockNode | null): boolean => !!n && (n === node || (n.kind === "split" && n.children.some(walk)));
    return walk(this.#layout.root);
  }

  /** How the windows are shown right now; `small`: the screen is small enough for the compact view. */
  mode(): { mode: DockMode; small: boolean; free: boolean } {
    return { mode: this.#mode, small: this.#small(), free: this.#free };
  }

  /** Small screen: free layout (true) instead of the compact view – remembered. */
  setFreeLayout(on: boolean): void {
    this.#free = on;
    this.#saveCompact();
    this.#render();
  }

  /** Marker (dot) on the tab, e.g. unread agent replies. */
  setBadge(id: string, on: boolean): void {
    if (on) this.#badges.add(id);
    else this.#badges.delete(id);
    for (const t of this.#tabEls(id)) t.classList.toggle("unread", on);
  }

  // ------------------------------------------------------------ Tree

  #stacks(): Array<{ stack: StackNode; win: FloatWin | null }> {
    const out: Array<{ stack: StackNode; win: FloatWin | null }> = [];
    const walk = (n: DockNode): void => {
      if (n.kind === "stack") out.push({ stack: n, win: null });
      else n.children.forEach(walk);
    };
    if (this.#layout.root) walk(this.#layout.root);
    for (const f of this.#layout.floats) out.push({ stack: f.stack, win: f });
    return out;
  }

  #where(id: string): { stack: StackNode; win: FloatWin | null } | null {
    return this.#stacks().find((s) => s.stack.panels.includes(id)) ?? null;
  }

  #parentOf(target: DockNode): { parent: SplitNode; index: number } | null {
    const walk = (n: DockNode): { parent: SplitNode; index: number } | null => {
      if (n.kind !== "split") return null;
      for (let i = 0; i < n.children.length; i++) {
        const c = n.children[i]!;
        if (c === target) return { parent: n, index: i };
        const r = walk(c);
        if (r) return r;
      }
      return null;
    };
    return this.#layout.root ? walk(this.#layout.root) : null;
  }

  #detach(ids: string[]): void {
    for (const { stack: s } of this.#stacks()) {
      if (!ids.some((id) => s.panels.includes(id))) continue;
      const at = s.panels.indexOf(s.active);
      s.panels = s.panels.filter((p) => !ids.includes(p));
      if (!s.panels.includes(s.active)) s.active = s.panels[Math.max(0, at - 1)] ?? s.panels[0] ?? "";
    }
  }

  #insert(panels: string[], active: string, t: Target, size: { w: number; h: number }): void {
    const L = this.#layout;
    const fresh = stack(panels, active);
    if (t.kind === "tab" || (t.kind === "stack" && t.zone === "center")) {
      const s = t.stack;
      s.panels.splice(t.kind === "tab" ? clamp(t.index, 0, s.panels.length) : s.panels.length, 0, ...panels);
      s.active = active;
      return;
    }
    if (t.kind === "float") {
      L.floats.push({ stack: fresh, x: t.x, y: t.y, w: size.w, h: size.h });
      return;
    }
    if (t.kind === "root" && (t.zone === "center" || !L.root)) {
      if (L.root) L.floats.push({ stack: fresh, x: 40, y: 40, w: size.w, h: size.h });
      else L.root = fresh;
      return;
    }
    const dir: Dir = t.zone === "left" || t.zone === "right" ? "row" : "col";
    const before = t.zone === "left" || t.zone === "top";
    if (t.kind === "root") {
      const root = L.root!;
      if (root.kind === "split" && root.dir === dir) {
        root.sizes = root.sizes.map((s) => s * (1 - ROOT_SHARE));
        if (before) {
          root.children.unshift(fresh);
          root.sizes.unshift(ROOT_SHARE);
        } else {
          root.children.push(fresh);
          root.sizes.push(ROOT_SHARE);
        }
      } else {
        L.root = before ? split(dir, [fresh, root], [ROOT_SHARE, 1 - ROOT_SHARE]) : split(dir, [root, fresh], [1 - ROOT_SHARE, ROOT_SHARE]);
      }
      return;
    }
    // Dock beside a stack.
    const p = this.#parentOf(t.stack);
    if (p && p.parent.dir === dir) {
      const s = p.parent.sizes[p.index] ?? 0.5;
      const at = before ? p.index : p.index + 1;
      p.parent.sizes[p.index] = s / 2;
      p.parent.children.splice(at, 0, fresh);
      p.parent.sizes.splice(at, 0, s / 2);
    } else {
      const sp = split(dir, before ? [fresh, t.stack] : [t.stack, fresh]);
      if (p) p.parent.children[p.index] = sp;
      else if (L.root === t.stack) L.root = sp;
      else {
        // floating stack: there is no split there – insert as a tab
        t.stack.panels.push(...panels);
        t.stack.active = active;
      }
    }
  }

  #norm(n: DockNode | null): DockNode | null {
    if (!n) return null;
    if (n.kind === "stack") {
      if (!n.panels.length) return null;
      if (!n.panels.includes(n.active)) n.active = n.panels[0]!;
      return n;
    }
    const kids: DockNode[] = [];
    const sizes: number[] = [];
    n.children.forEach((c, i) => {
      const k = this.#norm(c);
      if (!k) return;
      const s = n.sizes[i] && n.sizes[i]! > 0 ? n.sizes[i]! : 1 / n.children.length;
      if (k.kind === "split" && k.dir === n.dir) {
        const t = total(k.sizes) || 1;
        k.children.forEach((cc, j) => {
          kids.push(cc);
          sizes.push((s * (k.sizes[j] ?? 0)) / t);
        });
      } else {
        kids.push(k);
        sizes.push(s);
      }
    });
    if (!kids.length) return null;
    if (kids.length === 1) return kids[0]!;
    const t = total(sizes) || 1;
    n.children = kids;
    n.sizes = sizes.map((s) => s / t);
    return n;
  }

  #normalize(): void {
    const L = this.#layout;
    L.root = this.#norm(L.root);
    L.floats = L.floats.filter((f) => this.#norm(f.stack));
  }

  #ensureAll(): void {
    for (const id of this.#order) {
      const def = this.#defs.get(id)!;
      if (this.#where(id)) continue;
      if (def.closable && this.#layout.closed.includes(id)) continue;
      if (def.closable && def.startClosed) {
        this.#layout.closed.push(id);
        continue;
      }
      this.#reopen(id);
    }
  }

  #reopen(id: string): void {
    const L = this.#layout;
    L.closed = L.closed.filter((c) => c !== id);
    const home = L.homes[id];
    delete L.homes[id];
    const size = { w: 420, h: 300 };
    const tabWith = (ids: string[]) => {
      for (const c of ids) {
        const w = this.#where(c);
        if (!w) continue;
        w.stack.panels.push(id);
        w.stack.active = id;
        return true;
      }
      return false;
    };
    if (tabWith(home?.with ?? [])) return;
    if (home?.float) {
      L.floats.push({ stack: stack([id]), ...home.float });
      return;
    }
    const near = home?.near ? this.#where(home.near) : null;
    if (near && !near.win && home?.zone) return this.#insert([id], id, { kind: "stack", stack: near.stack, zone: home.zone }, size);
    if (home?.edge) return this.#insert([id], id, { kind: "root", zone: home.edge }, size);
    if (tabWith(this.#companions.get(id) ?? [])) return;
    const float = this.#defs.get(id)?.float;
    if (float) {
      L.floats.push({ stack: stack([id]), ...float(this.#host.clientWidth || 1200, this.#host.clientHeight || 800) });
      return;
    }
    // New window in a saved layout: to its intended place
    const place = this.#defs.get(id)?.place;
    const anchor = place ? this.#where(place.near) : null;
    if (place && anchor && !anchor.win) return this.#insert([id], id, { kind: "stack", stack: anchor.stack, zone: place.zone }, size);
    this.#insert([id], id, { kind: "root", zone: "bottom" }, size);
  }

  /** Place of a singly docked stack: edge of the workspace or side beside a neighbor. */
  #placeOf(s: StackNode): Pick<Home, "near" | "zone" | "edge"> {
    const p = this.#parentOf(s);
    if (!p) return {};
    const { parent, index } = p;
    const last = index === parent.children.length - 1;
    const after = index > 0 && (last || index >= parent.children.length / 2);
    if (parent === this.#layout.root) {
      return { edge: parent.dir === "row" ? (after ? "right" : "left") : after ? "bottom" : "top" };
    }
    // Neighbor on the facing side; its nearest stack serves as the anchor.
    const sib = parent.children[after ? index - 1 : index + 1];
    if (!sib) return {};
    const edgeStack = (n: DockNode): StackNode => (n.kind === "stack" ? n : edgeStack(after ? n.children[n.children.length - 1]! : n.children[0]!));
    const anchor = edgeStack(sib);
    const zone: Zone = parent.dir === "row" ? (after ? "right" : "left") : after ? "bottom" : "top";
    return { near: anchor.active || anchor.panels[0], zone };
  }

  // ------------------------------------------------------------ Load/Save

  #load(): Layout | null {
    let raw: unknown;
    try {
      raw = JSON.parse(localStorage.getItem(this.#key) ?? "null");
    } catch {
      return null;
    }
    if (!raw || typeof raw !== "object") return null;
    const o = raw as Record<string, unknown>;
    const seen = new Set<string>();
    const node = (v: unknown): DockNode | null => {
      if (!v || typeof v !== "object") return null;
      const n = v as Record<string, unknown>;
      if (n.kind === "stack" && Array.isArray(n.panels)) {
        const panels = n.panels.filter((p): p is string => typeof p === "string" && this.#defs.has(p) && !seen.has(p));
        for (const p of panels) seen.add(p);
        if (!panels.length) return null;
        return stack(panels, typeof n.active === "string" && panels.includes(n.active) ? n.active : panels[0]);
      }
      if (n.kind === "split" && (n.dir === "row" || n.dir === "col") && Array.isArray(n.children)) {
        const sizes = Array.isArray(n.sizes) ? n.sizes : [];
        const kids: DockNode[] = [];
        const ks: number[] = [];
        n.children.forEach((c, i) => {
          const k = node(c);
          if (!k) return;
          kids.push(k);
          ks.push(Math.max(0.01, num(sizes[i], 1)));
        });
        return kids.length ? { kind: "split", dir: n.dir, children: kids, sizes: ks } : null;
      }
      return null;
    };
    const root = node(o.root);
    const floats: FloatWin[] = [];
    for (const f of Array.isArray(o.floats) ? o.floats : []) {
      if (!f || typeof f !== "object") continue;
      const r = f as Record<string, unknown>;
      const s = node(r.stack);
      if (s?.kind === "stack") floats.push({ stack: s, x: num(r.x, 40), y: num(r.y, 40), w: num(r.w, 420), h: num(r.h, 300) });
    }
    const closed = (Array.isArray(o.closed) ? o.closed : []).filter((c): c is string => typeof c === "string" && this.#defs.has(c) && !seen.has(c));
    const homes: Record<string, Home> = {};
    if (o.homes && typeof o.homes === "object") {
      for (const [id, v] of Object.entries(o.homes as Record<string, unknown>)) {
        if (!this.#defs.has(id) || !v || typeof v !== "object") continue;
        const x = v as Record<string, unknown>;
        const home: Home = { with: Array.isArray(x.with) ? x.with.filter((p): p is string => typeof p === "string") : [] };
        if (isZone(x.edge)) home.edge = x.edge;
        if (isZone(x.zone)) home.zone = x.zone;
        if (typeof x.near === "string" && this.#defs.has(x.near)) home.near = x.near;
        if (x.float && typeof x.float === "object") {
          const b = x.float as Record<string, unknown>;
          home.float = { x: num(b.x, 40), y: num(b.y, 40), w: num(b.w, 420), h: num(b.h, 300) };
        }
        homes[id] = home;
      }
    }
    if (!root && !floats.length) return null;
    return { root, floats, closed, homes };
  }

  #loadCompact(): void {
    try {
      const raw = JSON.parse(localStorage.getItem(`${this.#key}.compact`) ?? "null") as Record<string, unknown> | null;
      if (!raw || typeof raw !== "object") return;
      this.#free = raw.free === true;
      const sz = Array.isArray(raw.sizes) ? raw.sizes.map((v) => num(v, 0.5)) : [];
      if (sz.length === 2 && sz.every((v) => v > 0.05)) this.#compact.split.sizes = sz;
    } catch {
      /* without storage */
    }
  }

  #saveCompact(): void {
    try {
      localStorage.setItem(`${this.#key}.compact`, JSON.stringify({ free: this.#free, sizes: this.#compact.split.sizes }));
    } catch {
      /* without storage */
    }
  }

  #save(): void {
    if (this.#mode !== "dock") this.#saveCompact();
    // temporarily enlarged (emphasize): save the actual sizes
    const shown = this.#emph.map((e) => e.split.sizes);
    for (const e of [...this.#emph].reverse()) e.split.sizes = e.sizes;
    try {
      localStorage.setItem(this.#key, JSON.stringify(this.#layout));
    } catch {
      /* without storage */
    } finally {
      this.#emph.forEach((e, i) => (e.split.sizes = shown[i]!));
    }
  }

  // ------------------------------------------------------------ Rendering

  #render(): void {
    this.#mode = this.#wantMode();
    this.#host.dataset.mode = this.#mode;
    this.#views = [];
    this.#floatEls.clear();
    const L = this.#layout;
    if (this.#mode !== "dock") {
      this.#area.replaceChildren(this.#compactView());
      this.#floatLayer.replaceChildren();
    } else {
      this.#area.replaceChildren(
        L.root ? this.#node(L.root) : h("div", { class: "dk-empty" }, t("All windows are floating – drag a tab onto the marker in the middle to dock it here.")),
      );
      this.#floatLayer.replaceChildren(...L.floats.map((w, i) => this.#float(w, i)));
    }
    this.#publish();
    this.#save();
  }

  #small(): boolean {
    const W = this.#host.clientWidth;
    const H = this.#host.clientHeight;
    return !!W && !!H && (W < SMALL_W || H < SMALL_H);
  }

  /** Compact view only on a small screen where the layout doesn't fit (its minimum sizes). */
  #wantMode(): DockMode {
    const W = this.#host.clientWidth;
    const H = this.#host.clientHeight;
    if (!W || !H) return this.#mode;
    if (this.#free || !this.#small()) return "dock";
    const root = this.#layout.root;
    const inset = 2 * this.#inset;
    if (!root || (W >= this.#minSize(root, true) + inset && H >= this.#minSize(root, false) + inset)) return "dock";
    return H >= SHEET_MIN_H ? "sheet" : "single";
  }

  /** Compact view: the main window above and the other windows as tabs below – or all as tabs. */
  #compactView(): HTMLElement {
    const C = this.#compact;
    const open = this.#order.filter((id) => this.isOpen(id));
    const main = open.filter((id) => !this.#defs.get(id)!.closable);
    const rest = open.filter((id) => this.#defs.get(id)!.closable);
    // the tool that was in front in the layout stays in front
    const front = this.#stacks().map((s) => s.stack.active).find((id) => rest.includes(id));
    const fill = (s: StackNode, panels: string[], prefer?: string) => {
      s.panels = panels;
      if (!panels.includes(s.active)) s.active = (prefer && panels.includes(prefer) ? prefer : panels[0]) ?? "";
    };
    if (this.#mode === "sheet" && main.length && rest.length) {
      fill(C.main, main);
      fill(C.sheet, rest, front);
      C.split.children = [C.main, C.sheet];
      return this.#node(C.split);
    }
    fill(C.single, open, main[0]);
    return this.#stack(C.single, null);
  }

  #publish(): void {
    const ids = new Set<string>();
    for (const v of this.#views) if (v.stack.active) ids.add(v.stack.active);
    this.#onVisible(ids);
  }

  #tabEls(id: string): HTMLElement[] {
    return [...this.#host.querySelectorAll<HTMLElement>(".dk-tab")].filter((t) => t.dataset.panel === id);
  }

  #panel(id: string): HTMLElement {
    let c = this.#content.get(id);
    if (!c) {
      c = h("div", { class: "dk-panel" }, this.#defs.get(id)!.create());
      c.dataset.panel = id;
      this.#content.set(id, c);
    }
    return c;
  }

  #node(n: DockNode): HTMLElement {
    if (n.kind === "stack") return this.#stack(n, null);
    const el = h("div", { class: `dk-split ${n.dir}` });
    n.children.forEach((c, i) => {
      if (i > 0) el.append(this.#gutter(n, i - 1, el));
      const child = this.#node(c);
      child.style.flex = `${n.sizes[i] ?? 1} 1 0px`;
      el.append(child);
    });
    return el;
  }

  #stack(s: StackNode, win: FloatWin | null): HTMLElement {
    const compact = this.#mode !== "dock";
    const tabs = h("div", { class: "dk-tabs", role: "tablist" });
    const body = h("div", { class: "dk-body" });
    for (const id of s.panels) {
      const def = this.#defs.get(id);
      if (!def) continue;
      const tab = h(
        "div",
        { class: `dk-tab${id === s.active ? " active" : ""}${this.#badges.has(id) ? " unread" : ""}`, role: "tab", title: t("{title} – drag to move or dock", { title: def.title }) },
        icon(def.icon),
        h("span", {}, def.title),
        h("i", { class: "badge" }),
      );
      tab.dataset.panel = id;
      if (def.closable) {
        const x = h("button", { class: "dk-close", title: t("Close {title} (reopen via \"Windows\")", { title: def.title }) }, icon("close"));
        x.addEventListener("pointerdown", (e) => e.stopPropagation());
        x.addEventListener("click", (e) => {
          e.stopPropagation();
          this.close(id);
        });
        tab.append(x);
        tab.addEventListener("auxclick", (e) => {
          if (e.button === 1) this.close(id);
        });
      }
      if (compact) tab.addEventListener("click", () => this.#activate(s, id));
      else tab.addEventListener("pointerdown", (e) => this.#pressTab(e, s, id, win));
      tabs.append(tab);
      const c = this.#panel(id);
      c.hidden = id !== s.active;
      body.append(c);
    }
    if (compact) {
      tabs.append(h("div", { class: "dk-fill" }));
    } else {
      tabs.append(h("div", { class: "dk-fill", title: win ? t("Drag: move or dock the window") : t("Drag: move the whole group") }));
      tabs.addEventListener("pointerdown", (e) => {
        if (!(e.target as Element).closest(".dk-tab")) this.#pressGroup(e, s, win);
      });
    }
    const el = h("div", { class: "dk-stack" }, tabs, body);
    if (!win) {
      el.style.minWidth = compact ? "0px" : `${this.#stackMin(s, true)}px`;
      el.style.minHeight = `${compact ? Math.min(COMPACT_MIN_H, this.#stackMin(s, false)) : this.#stackMin(s, false)}px`;
    }
    this.#views.push({ el, tabs, stack: s, win });
    if (compact) requestAnimationFrame(() => this.#revealTab(tabs, s.active));
    return el;
  }

  /** Compact view: scroll the tab bar so that the active tab can be seen. */
  #revealTab(tabs: HTMLElement, id: string): void {
    const tab = [...tabs.querySelectorAll<HTMLElement>(".dk-tab")].find((x) => x.dataset.panel === id);
    if (!tab) return;
    const l = tab.offsetLeft - tabs.offsetLeft;
    if (l < tabs.scrollLeft) tabs.scrollLeft = Math.max(0, l - 24);
    else if (l + tab.offsetWidth > tabs.scrollLeft + tabs.clientWidth) tabs.scrollLeft = l + tab.offsetWidth - tabs.clientWidth + 24;
  }

  #activate(s: StackNode, id: string): void {
    if (!s.panels.includes(id)) return;
    s.active = id;
    const v = this.#views.find((x) => x.stack === s);
    if (v) for (const t of v.tabs.querySelectorAll<HTMLElement>(".dk-tab")) t.classList.toggle("active", t.dataset.panel === id);
    if (v && this.#mode !== "dock") this.#revealTab(v.tabs, id);
    for (const p of s.panels) {
      const c = this.#content.get(p);
      if (c) c.hidden = p !== id;
    }
    this.#publish();
    this.#save();
  }

  // ------------------------------------------------------------ Dividers

  #stackMin(s: StackNode, horiz: boolean): number {
    let m = horiz ? MIN_W : MIN_H;
    for (const id of s.panels) {
      const d = this.#defs.get(id);
      m = Math.max(m, (horiz ? d?.minWidth : d?.minHeight) ?? 0);
    }
    return m;
  }

  #minSize(n: DockNode, horiz: boolean): number {
    if (n.kind === "stack") return this.#stackMin(n, horiz);
    const mins = n.children.map((c) => this.#minSize(c, horiz));
    return (n.dir === "row") === horiz ? total(mins) + this.#gap * (mins.length - 1) : Math.max(...mins);
  }

  #gutter(n: SplitNode, i: number, container: HTMLElement): HTMLElement {
    const horiz = n.dir === "row";
    const g = h("div", { class: `dk-gutter ${n.dir}`, title: t("Drag: resize · Double-click: equal sizes") });
    g.addEventListener("pointerdown", (e) => {
      if (e.button !== 0) return;
      e.preventDefault();
      const a = container.children[2 * i] as HTMLElement;
      const b = container.children[2 * i + 2] as HTMLElement;
      const ra = a.getBoundingClientRect();
      const rb = b.getBoundingClientRect();
      const pa = horiz ? ra.width : ra.height;
      const sum = pa + (horiz ? rb.width : rb.height);
      const share = (n.sizes[i] ?? 0.5) + (n.sizes[i + 1] ?? 0.5);
      const compact = this.#mode !== "dock";
      const minA = compact ? COMPACT_MIN_H : this.#minSize(n.children[i]!, horiz);
      const minB = compact ? COMPACT_MIN_H : this.#minSize(n.children[i + 1]!, horiz);
      const start = horiz ? e.clientX : e.clientY;
      g.classList.add("on");
      document.body.classList.add(horiz ? "dk-resize-col" : "dk-resize-row");
      track(
        (ev) => {
          const na = clamp(pa + (horiz ? ev.clientX : ev.clientY) - start, minA, Math.max(minA, sum - minB));
          n.sizes[i] = (share * na) / sum;
          n.sizes[i + 1] = share - n.sizes[i]!;
          a.style.flexGrow = String(n.sizes[i]);
          b.style.flexGrow = String(n.sizes[i + 1]);
        },
        () => {
          g.classList.remove("on");
          document.body.classList.remove("dk-resize-col", "dk-resize-row");
          this.#save();
        },
      );
    });
    g.addEventListener("dblclick", () => {
      const s = ((n.sizes[i] ?? 0.5) + (n.sizes[i + 1] ?? 0.5)) / 2;
      n.sizes[i] = s;
      n.sizes[i + 1] = s;
      (container.children[2 * i] as HTMLElement).style.flexGrow = String(s);
      (container.children[2 * i + 2] as HTMLElement).style.flexGrow = String(s);
      this.#save();
    });
    return g;
  }

  // ------------------------------------------------------------ floating windows

  #float(w: FloatWin, z: number): HTMLElement {
    const el = h("div", { class: "dk-float" }, this.#stack(w.stack, w));
    for (const g of ["l", "r", "b", "lb", "rb"]) el.append(this.#grip(w, el, g));
    el.style.zIndex = String(z + 1);
    el.addEventListener("pointerdown", () => this.#raise(w), true);
    this.#floatEls.set(w, el);
    this.#place(el, w);
    return el;
  }

  #place(el: HTMLElement, w: FloatWin): void {
    const W = this.#host.clientWidth;
    const H = this.#host.clientHeight;
    if (W > 200 && H > 120) {
      w.w = clamp(w.w, FLOAT_MIN_W, Math.max(FLOAT_MIN_W, W));
      w.h = clamp(w.h, FLOAT_MIN_H, Math.max(FLOAT_MIN_H, H));
      w.x = clamp(w.x, 100 - w.w, W - 100);
      w.y = clamp(w.y, 0, Math.max(0, H - 34));
    }
    el.style.left = `${Math.round(w.x)}px`;
    el.style.top = `${Math.round(w.y)}px`;
    el.style.width = `${Math.round(w.w)}px`;
    el.style.height = `${Math.round(w.h)}px`;
  }

  #raise(w: FloatWin): void {
    const L = this.#layout;
    const i = L.floats.indexOf(w);
    if (i < 0 || i === L.floats.length - 1) return;
    L.floats.splice(i, 1);
    L.floats.push(w);
    L.floats.forEach((f, j) => {
      const el = this.#floatEls.get(f);
      if (el) el.style.zIndex = String(j + 1);
    });
    this.#save();
  }

  #grip(w: FloatWin, el: HTMLElement, kind: string): HTMLElement {
    const g = h("div", { class: `dk-grip ${kind}` });
    g.addEventListener("pointerdown", (e) => {
      if (e.button !== 0) return;
      e.preventDefault();
      e.stopPropagation();
      this.#raise(w);
      const sx = e.clientX;
      const sy = e.clientY;
      const s = { x: w.x, y: w.y, w: w.w, h: w.h };
      track(
        (ev) => {
          const dx = ev.clientX - sx;
          const dy = ev.clientY - sy;
          if (kind.includes("r")) w.w = Math.max(FLOAT_MIN_W, s.w + dx);
          if (kind.includes("l")) {
            w.w = Math.max(FLOAT_MIN_W, s.w - dx);
            w.x = s.x + s.w - w.w;
          }
          if (kind.includes("b")) w.h = Math.max(FLOAT_MIN_H, s.h + dy);
          this.#place(el, w);
        },
        (ev) => {
          if (!ev) {
            Object.assign(w, s);
            this.#place(el, w);
          }
          this.#save();
        },
      );
    });
    return g;
  }

  // ------------------------------------------------------------ Dragging

  #pressTab(e: PointerEvent, s: StackNode, id: string, win: FloatWin | null): void {
    if (e.button !== 0) return;
    e.preventDefault();
    // Only tab of a floating window: drags the window.
    const whole = !!win && s.panels.length === 1;
    this.#press(e, { panels: whole ? [...s.panels] : [id], active: id, source: s, win: whole ? win : null, whole }, () => this.#activate(s, id));
  }

  #pressGroup(e: PointerEvent, s: StackNode, win: FloatWin | null): void {
    if (e.button !== 0) return;
    e.preventDefault();
    this.#press(e, { panels: [...s.panels], active: s.active, source: s, win, whole: true }, () => {});
  }

  #press(e: PointerEvent, p: Payload, click: () => void): void {
    const sx = e.clientX;
    const sy = e.clientY;
    let started = false;
    let target: Target | null = null;
    const winEl = p.win ? this.#floatEls.get(p.win) : undefined;
    const origin = p.win ? { x: p.win.x, y: p.win.y } : null;
    const src = this.#views.find((v) => v.stack === p.source)?.el.getBoundingClientRect();
    const size = p.win
      ? { w: p.win.w, h: p.win.h }
      : { w: clamp(src?.width ?? 420, 300, 640), h: clamp(src?.height ?? 300, 200, 480) };
    track(
      (ev) => {
        if (!started) {
          if (Math.hypot(ev.clientX - sx, ev.clientY - sy) < DRAG_START) return;
          started = true;
          this.#beginDrag(p);
        }
        if (p.win && winEl && origin) {
          p.win.x = origin.x + ev.clientX - sx;
          p.win.y = origin.y + ev.clientY - sy;
          this.#place(winEl, p.win);
        }
        if (this.#ghost) {
          this.#ghost.style.left = `${ev.clientX + 14}px`;
          this.#ghost.style.top = `${ev.clientY + 12}px`;
        }
        target = this.#hover(ev.clientX, ev.clientY, p, size);
      },
      (ev) => {
        if (!started) {
          if (ev) click();
          return;
        }
        this.#endDrag();
        if (!ev) {
          if (p.win && winEl && origin) {
            p.win.x = origin.x;
            p.win.y = origin.y;
            this.#place(winEl, p.win);
          }
          return;
        }
        if (target) this.#drop(p, target, size);
        else this.#save();
      },
    );
  }

  #beginDrag(p: Payload): void {
    document.body.classList.add("dk-dragging");
    this.#overlay.classList.add("on");
    if (!p.win) {
      const def = this.#defs.get(p.active);
      this.#ghost = h("div", { class: "dk-ghost" }, def ? icon(def.icon) : null, p.panels.map((id) => this.#defs.get(id)?.title ?? id).join(" · "));
      document.body.append(this.#ghost);
    }
  }

  #endDrag(): void {
    document.body.classList.remove("dk-dragging");
    this.#overlay.classList.remove("on");
    this.#preview.style.display = "none";
    this.#insertMark.style.display = "none";
    for (const m of [...this.#compass.values(), ...this.#rootMarks.values()]) m.classList.remove("show", "hot");
    this.#ghost?.remove();
    this.#ghost = null;
    this.#under?.classList.remove("dk-under");
    this.#under = null;
  }

  #viewAt(x: number, y: number, p: Payload): StackView | null {
    const order = this.#layout.floats;
    const floats = this.#views
      .filter((v) => v.win && v.win !== p.win)
      .sort((a, b) => order.indexOf(b.win!) - order.indexOf(a.win!));
    for (const v of floats) if (inside(v.el.getBoundingClientRect(), x, y)) return v;
    for (const v of this.#views) if (!v.win && inside(v.el.getBoundingClientRect(), x, y)) return v;
    return null;
  }

  #placeMark(m: HTMLElement, show: boolean, at: [number, number]): void {
    m.classList.toggle("show", show);
    m.classList.remove("hot");
    if (!show) return;
    m.style.left = `${Math.round(at[0])}px`;
    m.style.top = `${Math.round(at[1])}px`;
  }

  #hover(x: number, y: number, p: Payload, size: { w: number; h: number }): Target | null {
    const host = this.#host.getBoundingClientRect();
    const hasRoot = !!this.#layout.root;
    let target: Target | null = null;
    let box: Box | null = null;
    let mark: Box | null = null;

    // Markers at the edges of the workspace (or the center if nothing is docked)
    const cx = host.left + host.width / 2;
    const cy = host.top + host.height / 2;
    const rootAt: Record<Zone, [number, number]> = {
      center: [cx, cy], left: [host.left + 26, cy], right: [host.right - 26, cy], top: [cx, host.top + 26], bottom: [cx, host.bottom - 26],
    };
    for (const z of ZONES) this.#placeMark(this.#rootMarks.get(z)!, hasRoot ? z !== "center" : z === "center", rootAt[z]);

    // Target cross in the stack under the pointer
    const v = this.#viewAt(x, y, p);
    const self = !!v && v.stack === p.source && (p.whole || p.source.panels.length === 1);
    const r = v ? v.el.getBoundingClientRect() : null;
    const under = v && !self ? v.el : null;
    if (under !== this.#under) {
      this.#under?.classList.remove("dk-under");
      under?.classList.add("dk-under");
      this.#under = under;
    }
    for (const z of ZONES) {
      const show = !!v && !self && (z === "center" || !v.win);
      this.#placeMark(this.#compass.get(z)!, show, r ? [r.left + r.width / 2 + COMPASS[z][0], r.top + r.height / 2 + COMPASS[z][1]] : [0, 0]);
    }

    const hit = (m: HTMLElement) => m.classList.contains("show") && inside(m.getBoundingClientRect(), x, y);
    for (const z of ZONES) {
      const m = this.#rootMarks.get(z)!;
      if (!target && hit(m)) {
        m.classList.add("hot");
        target = { kind: "root", zone: z };
        box = this.#rootBox(host, z);
      }
    }
    if (!target && v && r) {
      const tr = v.tabs.getBoundingClientRect();
      if (self) {
        // releasing over its own tab bar = do nothing
        if (inside(tr, x, y)) return this.#showPreview(null, null, false);
      } else {
        for (const z of ZONES) {
          const m = this.#compass.get(z)!;
          if (!target && hit(m)) {
            m.classList.add("hot");
            target = { kind: "stack", stack: v.stack, zone: z };
            box = this.#zoneBox(r, z);
          }
        }
        if (!target && inside(tr, x, y)) {
          const tabs = [...v.tabs.querySelectorAll<HTMLElement>(".dk-tab")];
          let index = tabs.length;
          let mx = tabs.length ? tabs[tabs.length - 1]!.getBoundingClientRect().right : tr.left + 6;
          for (let i = 0; i < tabs.length; i++) {
            const b = tabs[i]!.getBoundingClientRect();
            if (x < b.left + b.width / 2) {
              index = i;
              mx = b.left;
              break;
            }
          }
          target = { kind: "tab", stack: v.stack, index };
          box = { x: r.left, y: r.top, w: r.width, h: r.height };
          mark = { x: mx - 1, y: tr.top + 5, w: 2, h: tr.height - 10 };
        }
      }
    }
    if (!target && !p.win) {
      const fx = x - host.left - 60;
      const fy = y - host.top - 14;
      target = { kind: "float", x: fx, y: fy };
      box = { x: host.left + fx, y: host.top + fy, w: size.w, h: size.h };
    }
    this.#showPreview(box, mark, target?.kind === "float");
    return target;
  }

  #showPreview(box: Box | null, mark: Box | null, floating: boolean): null {
    const put = (el: HTMLElement, b: Box | null) => {
      el.style.display = b ? "block" : "none";
      if (!b) return;
      el.style.left = `${Math.round(b.x)}px`;
      el.style.top = `${Math.round(b.y)}px`;
      el.style.width = `${Math.round(b.w)}px`;
      el.style.height = `${Math.round(b.h)}px`;
    };
    this.#preview.classList.toggle("float", floating);
    put(this.#preview, box);
    put(this.#insertMark, mark);
    return null;
  }

  #rootBox(o: DOMRect, z: Zone): Box {
    const w = Math.max(200, o.width * ROOT_SHARE);
    const hh = Math.max(140, o.height * ROOT_SHARE);
    switch (z) {
      case "left": return { x: o.left, y: o.top, w, h: o.height };
      case "right": return { x: o.right - w, y: o.top, w, h: o.height };
      case "top": return { x: o.left, y: o.top, w: o.width, h: hh };
      case "bottom": return { x: o.left, y: o.bottom - hh, w: o.width, h: hh };
      default: return { x: o.left, y: o.top, w: o.width, h: o.height };
    }
  }

  #zoneBox(r: DOMRect, z: Zone): Box {
    switch (z) {
      case "left": return { x: r.left, y: r.top, w: r.width / 2, h: r.height };
      case "right": return { x: r.left + r.width / 2, y: r.top, w: r.width / 2, h: r.height };
      case "top": return { x: r.left, y: r.top, w: r.width, h: r.height / 2 };
      case "bottom": return { x: r.left, y: r.top + r.height / 2, w: r.width, h: r.height / 2 };
      default: return { x: r.left, y: r.top, w: r.width, h: r.height };
    }
  }

  #drop(p: Payload, t: Target, size: { w: number; h: number }): void {
    if ((t.kind === "tab" || t.kind === "stack") && t.stack === p.source) {
      if (p.whole || p.source.panels.length === 1) return;
      if (t.kind === "tab" && p.source.panels.indexOf(p.panels[0]!) < t.index) t = { ...t, index: t.index - 1 };
    }
    this.#detach(p.panels);
    this.#insert(p.panels, p.active, t, size);
    this.#normalize();
    this.#render();
  }
}
