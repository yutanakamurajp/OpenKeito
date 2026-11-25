// The graphical symbols of the diagram: one for every component type and
// every control block type of `openkeito components`.
//
// The circuit symbols follow the forms of IEC 60617 where it has one
// (resistor, inductor, capacitor, ideal sources, make contact, diode,
// two-winding transformer, machine, converter, earth) and are composed of
// its elements otherwise (see web/README.md for the table). A control block
// is a rectangle with a qualifying mark.
//
// A symbol is drawn once, lying, in a frame of SYMBOL_WIDTH: x from 0 to
// SYMBOL_WIDTH, the line of its first ports at y = 0, further ports TRACK
// below. A terminal on the other side of the diagram is the same symbol
// mirrored (`flip`); text is moved, not mirrored.
//
// A new component type gets an entry in CIRCUIT (a new block type in
// CONTROL): a test counts the types of the catalog without one, which are
// drawn as a box with their name.

export const SYMBOL_WIDTH = 44;
/// Distance between the ports of one side.
export const TRACK = 16;

const W = SYMBOL_WIDTH;

/// A number of the SVG: at most two decimals, no trailing zeros.
export function n(v) {
  const r = Math.round(v * 100) / 100;
  return Object.is(r, -0) ? "0" : String(r);
}

export function escapeXml(text) {
  return String(text).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
}

// ---- primitives: ["kind", ...numbers] ----
//
// l  x1 y1 x2 y2            line
// d  x1 y1 x2 y2            dashed line (a link to an operating device)
// r  x y w h                rectangle
// c  cx cy r                circle
// p  [x y ...]              open polyline
// g  [x y ...]              closed outline
// f  [x y ...]              closed and filled (an arrowhead)
// h  x y count radius       half circles above the line (a winding)
// w  cx cy halfwidth        one period of a sine
// t  x y text [size]        text, centred on x

const lead = (x1, x2, y = 0) => ["l", x1, y, x2, y];
const circle = (r, cx = W / 2, cy = 0) => ["c", cx, cy, r];
/// The link from a symbol to its operating device: what a control signal
/// acts on.
const controlled = (fromY) => [["d", W / 2, fromY, W / 2, 10], ["r", W / 2 - 3.5, 10, 7, 4]];
const arrowOnLead = ["f", [36, -3, 41.5, 0, 36, 3]];

function draw(prims, flip) {
  const fx = (x) => (flip ? W - x : x);
  const points = (list) => {
    const out = [];
    for (let i = 0; i < list.length; i += 2) out.push(`${n(fx(list[i]))},${n(list[i + 1])}`);
    return out.join(" ");
  };
  let lines = "";
  let texts = "";
  for (const prim of prims) {
    const [kind, a, b, c, d] = prim;
    if (kind === "l" || kind === "d") {
      const dashed = kind === "d" ? ' class="link-line"' : "";
      lines += `<line${dashed} x1="${n(fx(a))}" y1="${n(b)}" x2="${n(fx(c))}" y2="${n(d)}"/>`;
    } else if (kind === "r") {
      lines += `<rect x="${n(flip ? W - a - c : a)}" y="${n(b)}" width="${n(c)}" height="${n(d)}"/>`;
    } else if (kind === "c") {
      lines += `<circle cx="${n(fx(a))}" cy="${n(b)}" r="${n(c)}"/>`;
    } else if (kind === "p") {
      lines += `<polyline points="${points(a)}"/>`;
    } else if (kind === "g") {
      lines += `<polygon points="${points(a)}"/>`;
    } else if (kind === "f") {
      lines += `<polygon class="solid" points="${points(a)}"/>`;
    } else if (kind === "h") {
      // Symmetric: mirrored, the half circles start at the other end.
      const x0 = flip ? W - a - 2 * d * c : a;
      let path = `M${n(x0)},${n(b)}`;
      for (let k = 0; k < c; k += 1) path += `a${n(d)},${n(d)} 0 0 1 ${n(2 * d)},0`;
      lines += `<path d="${path}"/>`;
    } else if (kind === "w") {
      const x0 = fx(a) - c;
      lines += `<path d="M${n(x0)},${n(b)}q${n(c / 2)},${n(-c)} ${n(c)},0t${n(c)},0"/>`;
    } else if (kind === "t") {
      const size = d === undefined ? "" : ` font-size="${n(d)}"`;
      texts += `<text class="sym-text" x="${n(fx(a))}" y="${n(b)}"${size}>${escapeXml(c)}</text>`;
    }
  }
  return `<g class="sym">${lines}</g>${texts}`;
}

// ---- circuit components ----
//
// body: the primitives, or a function of the names of the terminals drawn.
// left, right: where the body begins and ends on the line of the first
// ports; the leads from the ports to it are drawn when the port is used.
// off, on: further primitives for a component with a state (a contact):
// `off` is drawn until a run says that it conducts, `on` while it does.
// tracks: the number of ports of a side (default 1).
// ports: the side and track of every terminal, by its name; default: the
// first terminal on the left, the second on the right.

const twoCircles = (r) => [["c", 17, 0, r], ["c", 27, 0, r]];
/// A make contact: open, and closed (the blade on its contact, with the two
/// ends of the contact marked so that it is not taken for a wire).
const contactOpen = ["l", 12, 0, 30, -9];
const contactClosed = [["l", 12, 0, 32, 0], ["c", 12, 0, 1.8], ["c", 32, 0, 1.8]];

export const CIRCUIT = {
  resistor: { left: 8, right: 36, body: [["r", 8, -5, 28, 10]] },
  inductor: { left: 8, right: 36, body: [["h", 8, 0, 4, 3.5]] },
  capacitor: { left: 19.5, right: 24.5, body: [["l", 19.5, -9, 19.5, 9], ["l", 24.5, -9, 24.5, 9]] },
  // Ideal voltage source: a circle with the line of the connection through
  // it; "+" at nodes[0].
  voltage_source: { left: 11, right: 33, body: [circle(11), lead(11, 33), ["t", 5, -3, "+", 9]] },
  // Ideal current source: a circle with a line across; the arrow is the
  // direction of a positive current, from nodes[0] to nodes[1].
  current_source: { left: 11, right: 33, body: [circle(11), ["l", 22, -11, 22, 11], arrowOnLead] },
  // Make contact.
  switch: { left: 12, right: 32, body: [], off: [contactOpen], on: contactClosed },
  // Anode at nodes[0].
  diode: { left: 15, right: 28, body: [["g", [15, -7, 15, 7, 28, 0]], ["l", 28, -7, 28, 7]] },
  controlled_switch: {
    left: 12,
    right: 32,
    body: [["r", W / 2 - 3.5, 10, 7, 4]],
    off: [contactOpen, ["d", W / 2, -4.5, W / 2, 10]],
    on: [...contactClosed, ["d", W / 2, 0, W / 2, 10]],
  },
  three_phase_source: { left: 11, right: 33, body: [circle(11), ["w", 22, -2, 3.5], ["t", 22, 8, "3", 7]] },
  // A series impedance.
  coupled_rl: { left: 8, right: 36, body: [["r", 8, -6, 28, 12], ["t", 22, 3, "Z", 9]] },
  // A series impedance with half the shunt capacitance at each end.
  pi_line: {
    left: 12,
    right: 32,
    body: [
      ["r", 12, -4, 20, 8],
      ...[6, 38].flatMap((x) => [
        ["l", x, 0, x, 5],
        ["l", x - 3.5, 5, x + 3.5, 5],
        ["l", x - 3.5, 8, x + 3.5, 8],
        ["l", x, 8, x, 11],
        ["l", x - 2.5, 11, x + 2.5, 11],
      ]),
    ],
  },
  // Two windings: the ends of winding 0 on one side, those of winding 1 on
  // the other.
  transformer: {
    tracks: 2,
    ports: { "windings[0][0]": ["L", 0], "windings[0][1]": ["L", 1], "windings[1][0]": ["R", 0], "windings[1][1]": ["R", 1] },
    body: [
      ["c", 17, 8, 10],
      ["c", 27, 8, 10],
      lead(0, 11),
      lead(0, 11, TRACK),
      lead(33, W),
      lead(33, W, TRACK),
    ],
  },
  // The star points that are brought out leave below their winding.
  three_phase_transformer: {
    tracks: (names) => (names.has("hv_neutral") || names.has("lv_neutral") ? 2 : 1),
    ports: { "buses[0]": ["L", 0], "buses[1]": ["R", 0], hv_neutral: ["L", 1], lv_neutral: ["R", 1] },
    left: 8,
    right: 36,
    body: (names) => [
      ...twoCircles(9),
      ...(names.has("hv_neutral") ? [["p", [0, TRACK, 17, TRACK, 17, 9]]] : []),
      ...(names.has("lv_neutral") ? [["p", [W, TRACK, 27, TRACK, 27, 9]]] : []),
    ],
  },
  // A flashover; with a second terminal, to ground.
  fault: {
    left: 9,
    right: 33,
    body: [["p", [9, 0, 19, -8, 16, 1, 27, -4]], ["f", [33, 0, 26, -7, 24.5, -0.5]]],
  },
  // An arrow away from the bus, towards the star point.
  three_phase_load: { left: 14, right: 28, body: [["f", [14, -7, 14, 7, 28, 0]]] },
  three_phase_current_source: {
    left: 11,
    right: 33,
    body: [circle(11), ["l", 22, -11, 22, 11], ["t", 28, 3, "3", 7], arrowOnLead],
  },
  // Ideal ratio with a phase shift: the arrow of something adjustable.
  three_phase_ideal_transformer: {
    left: 8,
    right: 36,
    body: [...twoCircles(9), ["l", 9, 11, 34, -11], ["f", [35, -12, 29.5, -11, 33, -6.5]]],
  },
  // The line of a characteristic that is not linear.
  nonlinear_resistor: { left: 8, right: 36, body: [["r", 8, -5, 28, 10], ["p", [11, 10, 31, -10, 36, -10]]] },
  nonlinear_inductor: {
    left: 8,
    right: 36,
    body: [["h", 8, 0, 4, 3.5], ["l", 8, -7, 36, -7], ["p", [11, 10, 31, -11, 36, -11]]],
  },
  // A line with a travel time.
  bergeron_line: {
    left: 6,
    right: 38,
    body: [["r", 6, -5, 32, 10], ["l", 10, -5, 10, 5], ["l", 34, -5, 34, 5], ["t", 22, 3, "τ", 9]],
  },
  controlled_voltage_source: { left: 13, right: 31, body: [circle(9), lead(13, 31), ["t", 6, -3, "+", 9], ...controlled(9)] },
  controlled_current_source: {
    left: 13,
    right: 31,
    body: [circle(9), ["l", 22, -9, 22, 9], arrowOnLead, ...controlled(9)],
  },
  three_phase_controlled_source: {
    left: 13,
    right: 31,
    body: [circle(9), ["w", 22, -2, 3], ["t", 22, 7, "3", 6.5], ...controlled(9)],
  },
  three_phase_controlled_current_source: {
    left: 13,
    right: 31,
    body: [circle(9), ["l", 22, -9, 22, 9], ["t", 27, 3, "3", 6.5], arrowOnLead, ...controlled(9)],
  },
  // A synchronous machine (GS), three-phase.
  synchronous_machine: { left: 10, right: 34, body: [circle(12), ["t", 22, 1, "GS", 8.5], ["t", 22, 9, "3~", 7]] },
  // A converter: alternating current towards the bus, direct current behind.
  grid_following_converter: {
    left: 9,
    right: 35,
    body: [["r", 9, -11, 26, 22], ["l", 9, 11, 35, -11], ["t", 16, -2, "~", 10], ["t", 28.5, 9, "=", 9]],
  },
};

// ---- control blocks: the mark inside the rectangle ----

const mark = (text, size) => [["t", W / 2, 3, text, size]];

export const CONTROL = {
  constant: mark("c"),
  gain: mark("K"),
  sum: mark("Σ", 12),
  triangle_wave: [["p", [10, 5, 16, -5, 22, 5, 28, -5, 34, 5]]],
  comparator: mark("a > b", 8.5),
  pi: mark("PI"),
  measurement: mark("meas", 8.5),
  transfer_function: mark("G(s)", 9),
  integrator: mark("∫", 13),
  lag: mark("1/(1+sT)", 8),
  lead_lag: mark("lead/lag", 8),
  delay: mark("delay", 8.5),
  limiter: [["p", [10, 6, 16, 6, 28, -6, 34, -6]]],
  rate_limiter: mark("±d/dt", 8.5),
  dead_band: [["p", [9, 6, 17, 0, 27, 0, 35, -6]]],
  product: mark("×", 12),
  quotient: mark("÷", 12),
  function: mark("f(u)", 9),
  selector: mark("select", 8.5),
  initial_value: mark("u(0)", 9),
  step: [["p", [10, 5, 22, 5, 22, -5, 34, -5]]],
  ramp: [["p", [10, 5, 17, 5, 29, -5, 34, -5]]],
  sine_wave: [["w", 22, 0, 6]],
  time_series: [["p", [10, 5, 16, 5, 16, 0, 24, 0, 24, -5, 34, -5]]],
  piecewise_linear: [["p", [10, 5, 16, 5, 22, -3, 28, -5, 34, -5]]],
  abc_to_dq0: mark("abc/dq0", 8),
  dq0_to_abc: mark("dq0/abc", 8),
  pll: mark("PLL", 9),
  three_phase_measurement: mark("meas 3~", 8),
  dq_limiter: mark("|dq| lim", 8),
  pi_dq: mark("PI dq", 8.5),
};

/// The types of the catalog (`openkeito components`) that have no symbol:
/// they are drawn as a box with their name.
export function typesWithoutSymbol(catalog) {
  return catalog
    .filter((entry) => !Object.hasOwn(entry.category === "control" ? CONTROL : CIRCUIT, entry.type))
    .map((entry) => entry.type);
}

/// The symbols that no type of the catalog uses.
export function symbolsWithoutType(catalog) {
  const types = (category) => new Set(catalog.filter((e) => e.category === category).map((e) => e.type));
  const circuit = types("circuit");
  const control = types("control");
  return [
    ...Object.keys(CIRCUIT).filter((t) => !circuit.has(t)),
    ...Object.keys(CONTROL).filter((t) => !control.has(t)),
  ];
}

/// The number of ports of a side of a circuit component with the terminals
/// `names` (a set).
export function circuitTracks(type, names) {
  const tracks = CIRCUIT[type]?.tracks ?? 1;
  return typeof tracks === "function" ? tracks(names) : tracks;
}

/// The side ("L" or "R") and the track of every terminal of a component, in
/// the frame of the symbol. A terminal that has no port (a type without a
/// symbol and with more than two terminals) gets a track of its own.
export function circuitPorts(type, terminalNames) {
  const fixed = CIRCUIT[type]?.ports;
  const used = { L: 0, R: 0 };
  return terminalNames.map((name, k) => {
    let [side, track] = fixed?.[name] ?? (k === 0 ? ["L", 0] : k === 1 ? ["R", 0] : ["R", used.R]);
    if (!fixed && k > 1) track = used.R;
    used[side] = Math.max(used[side], track + 1);
    return { side, track };
  });
}

/// The frame of a box: of a control block, of a type without a symbol.
function box(tracks, cls) {
  const height = 24 + (tracks - 1) * TRACK;
  return `<rect class="${cls}" x="0" y="-12" width="${W}" height="${n(height)}"/>`;
}

/// The SVG of a circuit symbol in its frame. `names`: the terminals drawn;
/// `ports`: which ports of the first track are used, { L, R }; `flip`: the
/// symbol mirrored.
export function circuitSymbol(type, { names, ports, flip = false, tracks = 1 }) {
  const def = CIRCUIT[type];
  if (!def) {
    // No symbol: a box with the name of the type.
    const text = type.length > 9 ? `${type.slice(0, 8)}…` : type;
    return (
      `<g class="sym">${box(tracks, "unknown-box")}</g>` +
      `<text class="sym-text" x="${W / 2}" y="${n(3 + ((tracks - 1) * TRACK) / 2)}" font-size="7">${escapeXml(text)}</text>`
    );
  }
  const body = typeof def.body === "function" ? def.body(names) : def.body;
  const leads = [];
  if (def.left !== undefined && ports.L) leads.push(lead(0, def.left));
  if (def.right !== undefined && ports.R) leads.push(lead(def.right, W));
  let s = draw([...leads, ...body], flip);
  if (def.off) s += `<g class="when-off">${draw(def.off, flip)}</g>`;
  if (def.on) s += `<g class="when-on">${draw(def.on, flip)}</g>`;
  return s;
}

/// The SVG of a control block: the rectangle and its mark.
export function controlSymbol(type, tracks) {
  const shift = ((tracks - 1) * TRACK) / 2;
  const prims = CONTROL[type];
  let inner;
  if (prims) {
    inner = `<g transform="translate(0 ${n(shift)})">${draw(prims, false)}</g>`;
  } else {
    const text = type.length > 9 ? `${type.slice(0, 8)}…` : type;
    inner = `<text class="sym-text" x="${W / 2}" y="${n(3 + shift)}" font-size="7">${escapeXml(text)}</text>`;
  }
  return `<g class="sym">${box(tracks, prims ? "block-box" : "unknown-box")}</g>${inner}`;
}
