// History window: every edit step of the active variant as a table – when,
// what, where, and the project's key figures afterwards (notes, length,
// tracks, clips, tempo, key). A click on a row goes back (or forward) to that
// state; Ctrl+Z / Ctrl+Y walk the same line step by step. Checkpoints name a
// state, variants are branches: editing after going back starts a new one
// instead of throwing the later steps away; "Combine…" brings variants together
// (historymerge.ts). The logic lives in the core (klang-core history.rs,
// merge.rs, klang-app history.rs) – this window only shows it.

import { backend } from "../bridge.js";
import { dropdown, h, icon, modal, noteName, type ModalAction } from "../dom.js";
import { language, plural, t } from "../i18n.js";
import { effect } from "../signals.js";
import { act, errorText, notify, snapshot, visiblePanels } from "../store.js";
import type { HistoryRow, HistoryVariant, HistoryView } from "../types.js";
import { mergeView } from "./historymerge.js";

/** Time of a step: clock time today, with the date on other days. */
function timeText(ms: number): string {
  if (!ms) return "–";
  const d = new Date(ms);
  const today = new Date().toDateString() === d.toDateString();
  const opts: Intl.DateTimeFormatOptions = today
    ? { hour: "2-digit", minute: "2-digit", second: "2-digit" }
    : { day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" };
  try {
    return d.toLocaleString(language(), opts);
  } catch {
    return d.toLocaleString(undefined, opts);
  }
}

function durationText(secs: number): string {
  const s = Math.max(0, Math.round(secs));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

const num = (v: number) => String(Math.round(v * 100) / 100);

/** Asks for a name; returns the chosen action and the text. */
async function askName(title: string, value: string, placeholder: string, actions: ModalAction[], hint?: string): Promise<{ choice: string | null; name: string }> {
  const input = h("input", { class: "modal-input hist-name", type: "text", value, placeholder, spellcheck: false }) as HTMLInputElement;
  const body: Node[] = [input];
  if (hint) body.push(h("div", { class: "modal-hint" }, hint));
  queueMicrotask(() => input.select());
  const choice = await modal(title, body, actions, input);
  return { choice, name: input.value.trim() };
}

export function historyPanel(): { el: HTMLElement; update(): void } {
  let view: HistoryView | null = null;
  let shownFor: unknown = null;
  let loading = false;
  let again = false;
  let lastCurrent = -1;

  const fail = (e: unknown) => notify(errorText(e), "error");
  const goto = (node: number) => void act(backend.historyGoto(node));
  const adopt = (v: HistoryView) => {
    view = v;
    render();
    // combining variants that are gone (another project, deleted): back to the table
    if (merge && !merge.variants().every((id) => v.variants.some((x) => x.id === id))) closeMerge();
  };

  // ------------------------------------------------------------ Actions
  const addCheckpoint = async () => {
    try {
      const v = await backend.historyCheckpoint(null);
      adopt(v);
      const cp = v.checkpoints[v.checkpoints.length - 1];
      if (cp) notify(t("Checkpoint \"{name}\" set – click its flag to rename it", { name: cp.name }));
    } catch (e) {
      fail(e);
    }
  };

  const newVariant = async (node: number | null) => {
    const n = (view?.variants.length ?? 1) + 1;
    const { choice, name } = await askName(t("New variant"), "", t("Variant {n}", { n }), [
      { label: t("Start variant"), value: "ok", kind: "primary" },
      { label: t("Cancel"), value: "cancel", kind: "ghost" },
    ], node == null
      ? t("From the current state: the following changes go into the new variant, the others stay as they are.")
      : t("The project goes back to this step; the following changes go into the new variant, the others stay as they are."));
    if (choice !== "ok") return;
    if (await act(backend.historyVariantNew(name || null, node))) notify(t("Variant \"{name}\" started – switch between variants above the table", { name: name || t("Variant {n}", { n }) }));
  };

  const editVariant = async (v: HistoryVariant) => {
    const actions: ModalAction[] = [{ label: t("Rename"), value: "rename", kind: "primary" }];
    if ((view?.variants.length ?? 0) > 1) actions.push({ label: t("Delete variant"), value: "delete", kind: "danger" });
    actions.push({ label: t("Cancel"), value: "cancel", kind: "ghost" });
    const { choice, name } = await askName(t("Variant \"{name}\"", { name: v.name }), v.name, v.name, actions,
      plural(v.steps, "{n} own step since it branched off", "{n} own steps since it branched off"));
    if (choice === "rename" && name && name !== v.name) {
      backend.historyVariantRename(v.id, name).then(adopt, fail);
    } else if (choice === "delete") {
      const sure = await modal(t("Delete variant \"{name}\"?", { name: v.name }), [
        h("div", { class: "modal-hint" }, plural(v.steps, "Its {n} own step is lost (can't be undone).", "Its {n} own steps are lost (can't be undone).")),
      ], [{ label: t("Delete variant"), value: "yes", kind: "danger" }, { label: t("Cancel"), value: "no", kind: "ghost" }]);
      if (sure === "yes") void act(backend.historyVariantDelete(v.id));
    }
  };

  const editCheckpoint = async (id: number, current: string, node: number) => {
    const { choice, name } = await askName(t("Checkpoint \"{name}\"", { name: current }), current, current, [
      { label: t("Rename"), value: "rename", kind: "primary" },
      { label: t("Go there"), value: "goto", kind: "blue" },
      { label: t("Remove"), value: "delete", kind: "danger" },
      { label: t("Cancel"), value: "cancel", kind: "ghost" },
    ], t("Removing the checkpoint keeps its steps."));
    if (choice === "rename" && name && name !== current) backend.historyCheckpointRename(id, name).then(adopt, fail);
    else if (choice === "goto") goto(node);
    else if (choice === "delete") backend.historyCheckpointDelete(id).then(adopt, fail);
  };

  const clearHistory = async () => {
    const sure = await modal(t("Clear history?"), [
      h("div", { class: "modal-hint" }, t("All steps, checkpoints and the other variants are forgotten – the project stays as it is now. Undo then starts here.")),
    ], [{ label: t("Clear history"), value: "yes", kind: "danger" }, { label: t("Cancel"), value: "no", kind: "ghost" }]);
    if (sure === "yes") void act(backend.historyClear());
  };

  // ------------------------------------------------------------ Structure
  const checkpointBtn = h("button", {
    class: "btn small hist-checkpoint",
    title: t("Set a checkpoint: names the current state so you can find it and go back to it"),
    onclick: () => void addCheckpoint(),
  }, icon("flag"), h("span", { class: "lbl" }, t("Checkpoint")));
  const variantBtn = h("button", {
    class: "btn small hist-variant-new",
    title: t("New variant from the current state – the following changes go into it, the other variants stay as they are. To branch off an earlier state use ⑂ in its row."),
    onclick: () => void newVariant(null),
  }, icon("branch"), h("span", { class: "lbl" }, t("New variant")));
  const variantsBar = h("div", { class: "hist-variants" });
  const moreBtn = h("button", { class: "btn small ghost hist-more", title: t("Checkpoints, clear history") }, icon("more")) as HTMLButtonElement;
  const more = dropdown(moreBtn, "hist-menu", (menu, close) => {
    const cps = view?.checkpoints ?? [];
    menu.replaceChildren(
      h("div", { class: "menu-title" }, t("Checkpoints")),
      ...(cps.length
        ? cps.map((c) => h("button", {
            class: "menu-item hist-cp-item",
            title: c.onLine ? t("Go to this state") : t("Go to this state (switches to the variant it is on)"),
            onclick: () => { close(); goto(c.node); },
          }, h("b", { class: "check" }), icon("flag"), h("em", {}, c.name), h("small", {}, [c.variant, timeText(c.time)].filter(Boolean).join(" · "))))
        : [h("div", { class: "menu-hint" }, t("No checkpoints yet – \"Checkpoint\" names the current state."))]),
      h("div", { class: "menu-sep" }),
      h("button", { class: "menu-item hist-clear", onclick: () => { close(); void clearHistory(); } }, h("b", { class: "check" }), icon("trash"), h("em", {}, t("Clear history…"))),
    );
  });

  const head = h("tr", {},
    h("th", { class: "n" }, "#"),
    h("th", {}, t("Time")),
    h("th", {}, t("Step")),
    h("th", {}, t("Where")),
    h("th", { class: "n", title: t("Notes in the whole project after this step") }, t("Notes")),
    h("th", { class: "n", title: t("Length up to the end of the last clip") }, t("Bars")),
    h("th", { class: "n" }, t("Duration")),
    h("th", { class: "n" }, t("Tracks")),
    h("th", { class: "n" }, t("Clips")),
    h("th", { class: "n" }, t("Tempo")),
    h("th", { title: t("Estimated from the notes of the melodic tracks") }, t("Key")),
  );
  const body = h("tbody");
  const scroll = h("div", { class: "hist-scroll" }, h("table", { class: "hist-table" }, h("thead", {}, head), body));
  const foot = h("div", { class: "hist-foot" });
  const combineBtn = h("button", {
    class: "btn small hist-combine",
    title: t("Combine variants: choose per track and section which version goes into a new variant"),
  }, icon("merge"), h("span", { class: "lbl" }, t("Combine…"))) as HTMLButtonElement;
  const top = h("div", { class: "hist-top" }, checkpointBtn, variantBtn, variantsBar, combineBtn, more);
  const el = h("div", { class: "history" }, top, scroll, foot);

  // "Combine" replaces the table until it is done or cancelled
  let merge: ReturnType<typeof mergeView> | null = null;
  const closeMerge = () => {
    merge?.destroy();
    merge = null;
    el.replaceChildren(top, scroll, foot);
    lastCurrent = -1;
    void refresh();
  };
  combineBtn.addEventListener("click", () => {
    if (!view || view.variants.length < 2) return;
    merge = mergeView(view, closeMerge);
    el.replaceChildren(merge.el);
  });

  // ------------------------------------------------------------ Display
  const cell = (text: string, cls = "", title?: string) => h("td", { class: cls, title }, text);

  const renderRow = (r: HistoryRow, prev: HistoryRow | undefined): HTMLTableRowElement => {
    const m = r.metrics;
    const p = prev?.metrics;
    const chg = (a: unknown, b: unknown) => (p && a !== b ? " chg" : "");
    const change = h("td", { class: "hist-change" }, h("span", { class: "hist-label" }, r.label));
    for (const c of r.checkpoints) {
      change.append(h("button", {
        class: "hist-flag",
        title: t("Checkpoint \"{name}\" – click: rename, remove", { name: c.name }),
        onclick: (e: Event) => { e.stopPropagation(); void editCheckpoint(c.id, c.name, r.id); },
      }, icon("flag"), c.name));
    }
    for (const f of r.forks) {
      const label = f.variant ? f.variant.name : plural(f.steps, "{n} step", "{n} steps");
      change.append(h("button", {
        class: `hist-fork${f.variant ? "" : " abandoned"}`,
        title: f.variant
          ? plural(f.steps, "Variant \"{name}\" branches off here ({n} step) – click: switch to it", "Variant \"{name}\" branches off here ({n} steps) – click: switch to it", { name: f.variant.name })
          : plural(f.steps, "{n} step was set aside here after an undo – click: bring it back as a variant", "{n} steps were set aside here after an undo – click: bring them back as a variant"),
        onclick: (e: Event) => {
          e.stopPropagation();
          if (f.variant) void act(backend.historyVariantSwitch(f.variant.id));
          else goto(f.tip);
        },
      }, icon("branch"), label));
    }
    change.append(h("button", {
      class: "hist-branch-here",
      title: t("Start a variant from this step"),
      onclick: (e: Event) => { e.stopPropagation(); void newVariant(r.id); },
    }, icon("branch")));
    const dNotes = p ? m.notes - p.notes : 0;
    const notes = h("td", { class: `n${chg(m.notes, p?.notes)}` }, String(m.notes),
      dNotes ? h("small", { class: dNotes > 0 ? "up" : "down" }, dNotes > 0 ? `+${dNotes}` : `−${-dNotes}`) : null);
    const bars = m.lengthBeats / Math.max(1, m.beatsPerBar);
    const sig = m.beatsPerBar === 4 && m.beatUnit === 4 ? "" : ` · ${m.beatsPerBar}/${m.beatUnit}`;
    const range = m.pitchRange ? `${noteName(m.pitchRange[0])} – ${noteName(m.pitchRange[1])}` : "";
    const tr = h("tr", {
      class: `hist-row${r.current ? " current" : ""}${r.future ? " future" : ""}`,
      title: r.current
        ? t("Current state")
        : r.future ? t("Click: redo up to this step") : t("Click: go back to this state"),
    },
      cell(String(r.index), "n idx"),
      cell(timeText(r.time), "time", r.time ? new Date(r.time).toLocaleString(language()) : undefined),
      change,
      cell(r.detail, "hist-detail", r.detail),
      notes,
      cell(num(bars), `n${chg(m.lengthBeats, p?.lengthBeats)}`),
      cell(durationText(m.lengthSecs), `n${chg(Math.round(m.lengthSecs), p ? Math.round(p.lengthSecs) : 0)}`),
      cell(String(m.tracks), `n${chg(m.tracks, p?.tracks)}`),
      cell(String(m.clips), `n${chg(m.clips, p?.clips)}`, m.audioClips ? plural(m.audioClips, "{n} with a recording", "{n} with recordings") : undefined),
      cell(`${num(m.tempo)}${sig}`, `n${chg(`${m.tempo}${sig}`, p ? `${p.tempo}${p.beatsPerBar === 4 && p.beatUnit === 4 ? "" : ` · ${p.beatsPerBar}/${p.beatUnit}`}` : "")}`),
      cell(m.key ?? "–", `key${chg(m.key, p?.key)}`, range ? t("Pitch range {range}", { range }) : undefined),
    );
    tr.dataset.node = String(r.id);
    tr.addEventListener("click", () => {
      if (!r.current) goto(r.id);
    });
    return tr;
  };

  const renderVariants = (v: HistoryView) => {
    variantsBar.replaceChildren(
      h("span", { class: "muted hist-var-lbl" }, t("Variants:")),
      ...v.variants.map((x) => {
        const b = h("button", {
          class: `hist-var${x.active ? " on" : ""}`,
          title: `${plural(x.steps, "{n} own step since it branched off", "{n} own steps since it branched off", {})} · ${t("last: {label}", { label: x.last })}\n${x.active ? t("Active variant – new changes go into it. Double-click or right-click: rename, delete") : t("Click: switch to this variant · double-click or right-click: rename, delete")}`,
        }, h("i"), x.name, h("small", {}, String(x.steps)));
        b.dataset.variant = String(x.id);
        b.addEventListener("click", () => {
          if (!x.active) void act(backend.historyVariantSwitch(x.id));
        });
        b.addEventListener("dblclick", () => void editVariant(x));
        b.addEventListener("contextmenu", (e) => {
          e.preventDefault();
          void editVariant(x);
        });
        return b;
      }),
    );
  };

  const render = () => {
    const v = view;
    if (!v) return;
    renderVariants(v);
    combineBtn.disabled = v.variants.length < 2;
    combineBtn.title = v.variants.length < 2
      ? t("Combining needs at least two variants – start one with \"New variant\"")
      : t("Combine variants: choose per track and section which version goes into a new variant");
    const rows = v.rows.map((r, i) => renderRow(r, v.rows[i - 1]));
    body.replaceChildren(...rows);
    const active = v.variants.find((x) => x.active);
    const past = v.rows.filter((r) => !r.future).length - 1;
    const future = v.rows.length - 1 - past;
    foot.replaceChildren(
      h("span", {}, [
        plural(past, "{n} step", "{n} steps"),
        future ? plural(future, "{n} to redo", "{n} to redo") : "",
        v.total > v.rows.length ? plural(v.total, "{n} in all variants", "{n} in all variants") : "",
      ].filter(Boolean).join(" · ")),
      h("span", { class: "rh-gap" }),
      h("span", { class: "hint" }, active && v.variants.length > 1
        ? t("Variant \"{name}\" · click a row to go back to that state · Ctrl+Z / Ctrl+Y step by step", { name: active.name })
        : t("Click a row to go back to that state · Ctrl+Z / Ctrl+Y step by step")),
    );
    if (v.current !== lastCurrent) {
      lastCurrent = v.current;
      body.querySelector(".hist-row.current")?.scrollIntoView({ block: "nearest" });
    }
  };

  // ------------------------------------------------------------ Loading
  const refresh = async () => {
    if (loading) {
      again = true;
      return;
    }
    loading = true;
    try {
      adopt(await backend.historyView());
    } catch (e) {
      body.replaceChildren(h("tr", {}, h("td", { colSpan: 11, class: "muted" }, errorText(e))));
    } finally {
      loading = false;
      if (again) {
        again = false;
        void refresh();
      }
    }
  };

  const update = () => {
    // only while the window can be seen; every new snapshot can mean a new step
    if (!visiblePanels.peek().has("historie")) return;
    const s = snapshot.peek();
    if (s === shownFor && view) return;
    shownFor = s;
    void refresh();
    // the project changed while combining (agent, undo …): compare anew
    merge?.refresh();
  };
  effect(() => {
    if (visiblePanels.value.has("historie")) queueMicrotask(update);
  });

  return { el, update };
}
