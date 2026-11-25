// A placed face of the diagram (layout.js) as SVG text. Nothing here depends
// on a browser: the same placed face gives the same text, and with the
// results of a run the same face and the same frame (overlay.js) do.
//
// The SVG carries no styles and no scripts: its looks come from the page's
// style sheet (the page's content security policy allows nothing inline).
// What can be pressed says so itself: `data-item` and `data-path` on a
// component or block, `data-face` and the target on a link to the other
// face, `data-rail` on a node, a bus or a signal that has columns in the
// result. What a run fills in is marked too: `data-slot` on the text of a
// value, and on a component with a state the classes `state-on` or
// `state-off`, and `acting` while an event is about to change it.

import { SYMBOL_WIDTH, TRACK, circuitSymbol, controlSymbol, escapeXml, n } from "./symbols.js";

const W = SYMBOL_WIDTH;
/// From line to line of a name on several lines.
const LINE = 11;

function attributes(pairs) {
  return Object.entries(pairs)
    .filter(([, value]) => value !== undefined && value !== null && value !== "")
    .map(([key, value]) => ` ${key}="${escapeXml(value)}"`)
    .join("");
}

function points(list) {
  return list.map(([x, y]) => `${n(x)},${n(y)}`).join(" ");
}

/// A text on one or more lines: the lines of a name are `tspan`s of one
/// `text`, so that the name is whole in the document.
function textLines(cls, x, y, lines, more = "") {
  if (lines.length === 0) return "";
  const inner =
    lines.length === 1
      ? escapeXml(lines[0])
      : lines.map((line, k) => `<tspan x="${n(x)}"${k === 0 ? "" : ` dy="${LINE}"`}>${escapeXml(line)}</tspan>`).join("");
  return `<text class="${cls}" x="${n(x)}" y="${n(y)}"${more}>${inner}</text>`;
}

/// The mark of a diagnostic: a disc for an error, a triangle for a warning
/// (the shape says it, not only the colour).
function badge(severity, x, y) {
  if (!severity) return "";
  const shape =
    severity === "error"
      ? `<circle cx="${n(x)}" cy="${n(y)}" r="5.5"/>`
      : `<polygon points="${n(x)},${n(y - 6.5)} ${n(x + 6.5)},${n(y + 5)} ${n(x - 6.5)},${n(y + 5)}"/>`;
  return `<g class="badge ${severity}">${shape}<text x="${n(x)}" y="${n(y + 3.2)}">!</text></g>`;
}

function chipSvg(chip) {
  const target = chip.goto ?? {};
  return (
    `<g class="chip" tabindex="-1" role="link" focusable="true"${attributes({
      "data-face": target.face,
      "data-control": target.control,
      "data-signal": target.signal,
      "data-owner": target.owner,
      "data-component": target.component,
      "data-node": target.node,
      "data-bus": target.bus,
      "aria-label": chip.title ?? chip.text,
    })}>` +
    `<title>${escapeXml(chip.title ?? chip.text)}</title>` +
    textLines("chip-text", chip.x, chip.y, chip.lines ?? [chip.text]) +
    `</g>`
  );
}

/// Where a text is anchored, as a class: the style sheet of the page sets
/// `text-anchor` for every text, which an attribute would not override.
const anchor = (side) => (side === "start" || side === "end" ? ` at-${side}` : "");

/// The text of a value of a run: empty until a frame fills it.
function valueSvg(value, frame) {
  const text = frame?.texts.get(value.slot) ?? "";
  return (
    `<text class="value ${value.kind === "state" ? "state" : "quantity"}${anchor(value.anchor)}" x="${n(value.x)}" y="${n(value.y)}"` +
    ` data-slot="${escapeXml(value.slot)}">${escapeXml(text)}</text>`
  );
}

/// Returns the `data-heat` attribute for a rail or item in `frame`, or "".
export function heatAttribute(frame, id) {
  const level = frame?.levels?.get(id)?.level;
  return level ? ` data-heat="${escapeXml(level)}"` : "";
}

const KIND = { bus: "Bus", node: "Node", signal: "Signal" };

function railSvg(rail, frame) {
  const classes = ["rail", rail.kind, rail.severity, rail.missing ? "missing" : "", rail.columns ? "has-columns" : ""].filter(Boolean).join(" ");
  const title =
    `${KIND[rail.kind]} ${rail.name}` +
    (rail.kind === "bus" ? ` (phases ${rail.name}.a, ${rail.name}.b, ${rail.name}.c)` : "") +
    (rail.missing ? ": no block gives this signal" : "") +
    (rail.severity ? `: ${rail.severity}` : "") +
    (rail.columns ? ". Press to show its columns in the waveforms." : "");
  const lines = rail.labelLines ?? [rail.label];
  const longest = Math.max(0, ...lines.map((line) => line.length));
  // A rail with columns in the result can be pressed, like a component.
  const pressable = rail.columns ? ` tabindex="-1" role="button" focusable="true"${attributes({ "aria-label": `${KIND[rail.kind]} ${rail.name}` })}` : "";
  return (
    `<g class="${classes}" data-rail="${escapeXml(rail.id)}"${pressable}${heatAttribute(frame, rail.id)}><title>${escapeXml(title)}</title>` +
    `<line x1="${n(rail.x)}" y1="${n(rail.y1)}" x2="${n(rail.x)}" y2="${n(rail.y2)}"/>` +
    textLines("rail-label", rail.x, rail.labelY, lines) +
    (rail.value ? valueSvg(rail.value, frame) : "") +
    badge(rail.severity, rail.x + longest * 3.2 + 9, rail.labelY - 4) +
    `</g>` +
    rail.chips.map(chipSvg).join("")
  );
}

const WHAT = {
  circuit: (item) => item.type,
  control: (item) => `${item.type} block`,
  owner: (item) => `control blocks of the ${item.type}`,
  sink: (item) => `${item.type} in the circuit`,
  reduced: () => "components between two buses",
};

function symbolSvg(item) {
  const unflipped = (side) => (item.flip ? (side === "L" ? "R" : "L") : side);
  if (item.category === "circuit") {
    const used = { L: false, R: false };
    for (const port of item.ports) if (port.track === 0) used[unflipped(port.side)] = true;
    return circuitSymbol(item.type, { names: item.names, ports: used, flip: item.flip, tracks: item.tracks });
  }
  if (item.category === "control") return controlSymbol(item.type, item.tracks);
  if (item.category === "reduced") return `<g class="sym"><line x1="0" y1="0" x2="${W}" y2="0"/></g>`;
  // The blocks of a component, and a component that reads signals: a box
  // that stands for something of the circuit.
  const height = 24 + (item.tracks - 1) * TRACK;
  const text = item.category === "owner" ? "control" : "circuit";
  return (
    `<g class="sym"><rect class="link-box" x="0" y="-12" width="${W}" height="${n(height)}"/></g>` +
    `<text class="sym-text" x="${W / 2}" y="${n(3 + ((item.tracks - 1) * TRACK) / 2)}" font-size="8">${text}</text>`
  );
}

/// The classes of the state of a component in a frame.
export function stateClasses(state) {
  if (!state) return [];
  return [state.on ? "state-on" : "state-off", state.acting ? "acting" : ""].filter(Boolean);
}

function itemSvg(item, frame) {
  const state = item.stateful ? frame?.states.get(item.id) : undefined;
  const classes = ["item", item.category, item.severity, ...stateClasses(state)].filter(Boolean).join(" ");
  const what = WHAT[item.category](item);
  const lines = item.labelLines ?? (item.label === "" ? [] : [item.label]);
  let s =
    `<g class="${classes}" tabindex="-1" role="button" focusable="true"${attributes({
      "data-item": item.id,
      "data-path": item.path,
      "data-type": item.category === "circuit" ? item.type : undefined,
      "aria-label": `${item.name}, ${what}${item.severity ? `, ${item.severity}` : ""}`,
    })}${heatAttribute(frame, item.id)}>` +
    `<title>${escapeXml(item.title)}</title>` +
    `<rect class="hit" x="${n(item.x - 4)}" y="${n(item.y)}" width="${n(item.width + 8)}" height="${n(item.height)}" rx="4"/>`;
  for (const wire of item.wires) s += `<polyline class="wire" points="${points(wire)}"/>`;
  for (const dot of item.dots) s += `<circle class="dot" cx="${n(dot.x)}" cy="${n(dot.y)}" r="2.3"/>`;
  for (const g of item.grounds) {
    // Earth: the lead down to three bars.
    s += `<path class="earth" d="M${n(g.x)},${n(g.y)}v5m-5,0h10m-8,3h6m-4.5,3h3"/>`;
  }
  for (const a of item.arrows) {
    s += `<polygon class="arrow" points="${n(a.x)},${n(a.y)} ${n(a.x - 5.5)},${n(a.y - 3)} ${n(a.x - 5.5)},${n(a.y + 3)}"/>`;
  }
  for (const tag of item.tags) {
    s += `<text class="tag${anchor(tag.anchor)}" x="${n(tag.x)}" y="${n(tag.y)}">${escapeXml(tag.text)}</text>`;
  }
  for (const text of item.portLabels) {
    const whole = text.title && text.title !== text.text ? `<title>${escapeXml(text.title)}</title>` : "";
    s += `<text class="port-label${anchor(text.anchor)}" x="${n(text.x)}" y="${n(text.y)}">${whole}${escapeXml(text.text)}</text>`;
  }
  s += textLines("item-label", item.labelX, item.labelY, lines);
  s += `<g transform="translate(${n(item.symbol.x)} ${n(item.symbol.y)})">${symbolSvg(item)}</g>`;
  for (const value of item.values ?? []) s += valueSvg(value, frame);
  s += badge(item.severity, item.symbol.x + W + 4, item.symbol.top + 1);
  s += `</g>`;
  return s + item.chips.map(chipSvg).join("");
}

/// The SVG of a placed face. `title` names it for assistive technology;
/// `frame` (overlay.js, `frameAt`) fills in the values and states of a run
/// at one row.
export function toSvg(placed, title = "", frame = null) {
  return (
    `<svg xmlns="http://www.w3.org/2000/svg" class="diagram-svg ${placed.kind}" viewBox="0 0 ${placed.width} ${placed.height}"` +
    ` width="${placed.width}" height="${placed.height}" role="group"${attributes({ "aria-label": title })}>` +
    `<g class="rails">${placed.rails.map((rail) => railSvg(rail, frame)).join("")}</g>` +
    `<g class="items">${placed.items.map((item) => itemSvg(item, frame)).join("")}</g>` +
    `</svg>`
  );
}

/// The whole of a placed face in little, for an overview beside the diagram:
/// a line for every rail, a box for every symbol, in the colours of the
/// diagnostics. The frame of what the diagram shows (`.viewport`) is moved by
/// the page.
export function overviewSvg(placed) {
  let s =
    `<svg xmlns="http://www.w3.org/2000/svg" class="overview-svg ${placed.kind}" viewBox="0 0 ${placed.width} ${placed.height}"` +
    ` preserveAspectRatio="xMidYMid meet" role="img" aria-label="Overview of the whole diagram">`;
  for (const rail of placed.rails) {
    const classes = ["rail", rail.kind, rail.severity].filter(Boolean).join(" ");
    s += `<line class="${classes}" x1="${n(rail.x)}" y1="${n(rail.y1)}" x2="${n(rail.x)}" y2="${n(rail.y2)}"/>`;
  }
  for (const item of placed.items) {
    const classes = ["item", item.category, item.severity].filter(Boolean).join(" ");
    // The reach of the item: from its leftmost to its rightmost wire end.
    const xs = item.wires.flatMap((wire) => wire.map((p) => p[0]));
    const x1 = Math.min(item.symbol.x, ...xs);
    const x2 = Math.max(item.symbol.x + W, ...xs);
    s += `<line class="${classes} reach" x1="${n(x1)}" y1="${n(item.symbol.y)}" x2="${n(x2)}" y2="${n(item.symbol.y)}"/>`;
    s += `<rect class="${classes}" x="${n(item.symbol.x)}" y="${n(item.symbol.top)}" width="${W}" height="${n(item.symbol.height)}"/>`;
  }
  return `${s}<rect class="viewport" x="0" y="0" width="0" height="0"/></svg>`;
}
