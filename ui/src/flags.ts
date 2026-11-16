// Small flags for the language choice, as inline SVG: emoji flags (🇩🇪) show
// up as plain letters ("DE") on Windows, in WebView2 too.
//
// All flags share one shape (3:2, viewBox 18×12) so the list lines up; the
// stripes sit on whole units, which keeps them sharp at 18×12 px. Simplified
// where detail would be lost at that size (Spain without the coat of arms).

/** Union Jack. The red saltire is counterchanged (clipped to one side of each
 *  diagonal); the clip path needs an id that is unique in the document. */
let ukClips = 0;
function unionJack(): string {
  const id = `flag-uk-clip-${++ukClips}`;
  return (
    `<rect width="18" height="12" fill="#012169"/>` +
    `<path d="M0 0L18 12M18 0L0 12" stroke="#fff" stroke-width="2.4"/>` +
    `<clipPath id="${id}"><path d="M9 6h9v6zv6h-9zh-9v-6zv-6h9z"/></clipPath>` +
    `<path d="M0 0L18 12M18 0L0 12" stroke="#C8102E" stroke-width="1.6" clip-path="url(#${id})"/>` +
    `<path d="M9 0v12M0 6h18" stroke="#fff" stroke-width="3.6"/>` +
    `<path d="M9 0v12M0 6h18" stroke="#C8102E" stroke-width="2.4"/>`
  );
}

/** Horizontal stripes, top to bottom: [color, height]. */
function rows(...stripes: [string, number][]): string {
  let y = 0;
  return stripes
    .map(([fill, height]) => {
      const r = `<rect y="${y}" width="18" height="${height}" fill="${fill}"/>`;
      y += height;
      return r;
    })
    .join("");
}

/** Three vertical stripes, left to right. */
function columns(a: string, b: string, c: string): string {
  return [a, b, c].map((fill, i) => `<rect x="${i * 6}" width="6" height="12" fill="${fill}"/>`).join("");
}

const FLAGS: Record<string, () => string> = {
  // English: the United Kingdom's flag
  en: unionJack,
  de: () => rows(["#000", 4], ["#DD0000", 4], ["#FFCE00", 4]),
  fr: () => columns("#0055A4", "#fff", "#EF4135"),
  it: () => columns("#009246", "#fff", "#CE2B37"),
  es: () => rows(["#AA151B", 3], ["#F1BF00", 6], ["#AA151B", 3]),
  nl: () => rows(["#AE1C28", 4], ["#fff", 4], ["#21468B", 4]),
  pl: () => rows(["#fff", 6], ["#DC143C", 6]),
};

/** Whether there is a flag for the language. */
export function hasFlag(code: string): boolean {
  return Object.hasOwn(FLAGS, code);
}

/** Flag of a language (code from `LANGUAGES`); decoration only, the name is
 *  always shown beside it. Without a flag: an empty frame of the same size. */
export function flag(code: string): SVGSVGElement {
  const tpl = document.createElement("template");
  tpl.innerHTML = `<svg class="flag" viewBox="0 0 18 12" aria-hidden="true" focusable="false">${hasFlag(code) ? FLAGS[code]!() : ""}</svg>`;
  const svg = tpl.content.firstElementChild as SVGSVGElement;
  svg.dataset.flag = code;
  return svg;
}
