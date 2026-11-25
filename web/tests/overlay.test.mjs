// The results of a run on the diagram, without a browser: the values against
// result.csv, the states against the events, the columns of a component, and
// the names that are whole in the picture.
//
// Run through modules.test.mjs. The runs come from the WebAssembly module,
// which must be built first (crates/openkeito-wasm/README.md, "Build").

import assert from "node:assert/strict";
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { test } from "node:test";

import { parseNumericCsv } from "../src/csv.js";
import { circuitFace, controlsFace, place, wiresThroughSymbols, wrapLabel } from "../src/layout.js";
import {
  bindColumns,
  columnsOf,
  describeValues,
  formatQuantity,
  frameAt,
  itemOwners,
  neighbouringEvent,
  rowAtOrBefore,
  slotsOf,
  stateTimeline,
} from "../src/overlay.js";
import { overviewSvg, toSvg } from "../src/render.js";

const root = new URL("../../", import.meta.url);
const pkg = new URL("crates/openkeito-wasm/pkg/", root);

let loaded = null;
async function module() {
  assert.ok(
    existsSync(new URL("openkeito_wasm_bg.wasm", pkg)),
    "the WebAssembly module is not built: see crates/openkeito-wasm/README.md, \"Build\"",
  );
  if (!loaded) {
    loaded = await import(new URL("openkeito_wasm.js", pkg).href);
    loaded.initSync({ module: readFileSync(new URL("openkeito_wasm_bg.wasm", pkg)) });
  }
  return loaded;
}

const exampleFiles = readdirSync(new URL("examples/", root))
  .filter((f) => f.endsWith(".json"))
  .sort();
const textOf = (file) => readFileSync(new URL(`examples/${file}`, root), "utf8");

/// A run of a model as the page has it, and the diagram with room for it.
const runs = new Map();
async function ran(file, text = textOf(file)) {
  if (runs.has(file)) return runs.get(file);
  const wasm = await module();
  const topology = JSON.parse(wasm.topology(text));
  const output = wasm.run(text, file);
  const summary = JSON.parse(output.summary);
  const csv = output.file("result.csv");
  output.free();
  assert.equal(summary.status, "completed", file);
  const table = parseNumericCsv(csv);
  const events = summary.all_events ?? summary.events;
  const binding = bindColumns(topology, summary.result.columns, table.header);
  const timeline = stateTimeline(topology, JSON.parse(text), events);
  const slots = slotsOf(binding, timeline);
  const faces = { circuit: circuitFace(topology), controls: controlsFace(topology) };
  const placed = { circuit: place(faces.circuit, { slots }), controls: place(faces.controls, { slots }) };
  const all = { topology, summary, csv, table, events, binding, timeline, slots, faces, placed };
  runs.set(file, all);
  return all;
}

const frameOf = (run, row, phase = "a") => frameAt({ binding: run.binding, timeline: run.timeline, table: run.table, row, phase });
const svgOf = (run, frame) => toSvg(run.placed.circuit, "Circuit", frame) + toSvg(run.placed.controls, "Controls", frame);

/// The text of every `text` element of an SVG, the lines of a name joined.
function texts(svg) {
  const unescape = (s) => s.replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&quot;/g, '"').replace(/&amp;/g, "&");
  return [...svg.matchAll(/<text\b[^>]*>(.*?)<\/text>/g)].map((m) => unescape(m[1].replace(/<title>.*?<\/title>/g, "").replace(/<[^>]+>/g, "")));
}

/// The text of the slot `slot` in an SVG.
function slotText(svg, slot) {
  const escaped = slot.replace(/&/g, "&amp;").replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const found = new RegExp(`data-slot="${escaped}">([^<]*)</text>`).exec(svg);
  return found ? found[1] : null;
}

/// The cell of a column at a row, read from the text of result.csv itself.
function cell(csv, name, row) {
  const lines = csv.split("\n");
  const k = lines[0].split(",").indexOf(name);
  assert.ok(k >= 0, `no column ${name}`);
  return Number(lines[row + 1].split(",")[k]);
}

test("a quantity is written with four digits, a prefix and its unit", () => {
  assert.equal(formatQuantity(0, "V"), "0 V");
  assert.equal(formatQuantity(-12345.6, "A"), "−12.35 kA");
  assert.equal(formatQuantity(0.0015, "V"), "1.5 mV");
  assert.equal(formatQuantity(999.96, "V"), "1 kV");
  assert.equal(formatQuantity(66000, "V"), "66 kV");
  assert.equal(formatQuantity(3.14159e-7, "A"), "314.2 nA");
  assert.equal(formatQuantity(1.000123, ""), "1.0001");
  assert.equal(formatQuantity(-0.5, ""), "−0.5");
  assert.equal(formatQuantity(NaN, "V"), "–");
});

test("the values in the diagram are the cells of result.csv at the row of the cursor", async () => {
  const run = await ran("rlc.json");
  for (const row of [0, 1, 137, 5000, run.table.rows - 1]) {
    const frame = frameOf(run, row);
    const svg = svgOf(run, frame);
    assert.equal(frame.time, cell(run.csv, "time_s", row));
    // V(n3) at the node n3, I(L1) below the inductor L1.
    assert.equal(slotText(svg, "node:n3|V"), formatQuantity(cell(run.csv, "V(n3)", row), "V"));
    assert.equal(slotText(svg, "component:L1|I"), formatQuantity(cell(run.csv, "I(L1)", row), "A"));
  }
  // Written out once, so that the comparison does not rest on the formatter alone.
  const frame = frameOf(run, 137);
  assert.match(frame.texts.get("node:n3|V"), /^−?\d+(\.\d+)? [mµn]?V$/);
  assert.ok(Math.abs(cell(run.csv, "V(n3)", 137)) > 0);
  // What the model does not record is not in the picture: no slot, no text.
  assert.equal(slotText(svg(run), "component:R1|I"), null);
  assert.equal(slotText(svg(run), "node:n1|V"), null);
  function svg(r) {
    return svgOf(r, frameOf(r, 137));
  }
});

test("every example: every column has its place, and every slot the value of its column", async () => {
  let slots = 0;
  for (const file of exampleFiles) {
    const run = await ran(file);
    assert.deepEqual(run.binding.unbound, [], file);
    assert.equal(run.binding.entries.length, run.table.header.length - 1, file);
    // The diagram with room for the values holds what the layout promises.
    assert.deepEqual(wiresThroughSymbols(run.placed.circuit), [], file);
    assert.deepEqual(wiresThroughSymbols(run.placed.controls), [], file);
    const row = Math.floor(run.table.rows / 3);
    for (const phase of ["a", "c"]) {
      const frame = frameOf(run, row, phase);
      const svg = svgOf(run, frame);
      assert.ok(!/NaN|undefined|Infinity/.test(svg), file);
      for (const entry of run.binding.entries) {
        if (entry.slot === null || (entry.phase !== null && entry.phase !== phase)) continue;
        const want = formatQuantity(cell(run.csv, entry.name, row), entry.unit);
        assert.equal(slotText(svg, `${entry.owner}|${entry.slot}`), want, `${file}: ${entry.name} at row ${row}`);
        slots += 1;
      }
    }
  }
  assert.ok(slots > 200, `${slots} values compared`);
});

test("of a three-phase quantity the chosen phase is shown, at the side of its terminal", async () => {
  const run = await ran("three_phase_fault.json");
  const row = 12000;
  for (const phase of ["a", "b", "c"]) {
    const frame = frameOf(run, row, phase);
    assert.equal(frame.texts.get("bus:b2|V"), formatQuantity(cell(run.csv, `V(b2.${phase})`, row), "V"));
    assert.equal(frame.texts.get("component:T1|I0"), formatQuantity(cell(run.csv, `I(T1.0.${phase})`, row), "A"));
    assert.equal(frame.texts.get("component:T1|I1"), formatQuantity(cell(run.csv, `I(T1.1.${phase})`, row), "A"));
  }
  // The fault is in phase a only: no value at the other phases.
  assert.ok(frameOf(run, row, "a").texts.has("component:F1|I"));
  assert.ok(!frameOf(run, row, "b").texts.has("component:F1|I"));
  // Terminal 0 of T1 is at b2, to its left: the value of that side is there.
  const t1 = run.placed.circuit.items.find((i) => i.id === "component:T1");
  const [i0, i1] = ["I0", "I1"].map((slot) => t1.values.find((v) => v.slot === `component:T1|${slot}`));
  assert.ok(i0.x < t1.symbol.x && i1.x > t1.symbol.x + t1.symbol.width);
  assert.deepEqual([i0.anchor, i1.anchor], ["start", "end"]);
  // The line below the diagram lists every column, also the ground current.
  assert.match(describeValues(run.binding, run.table, row, "component:F1"), /^I\(F1\.a\) = .* A, I\(F1\.g\) = .* A$/);
});

test("the state of a switch changes at the time of its event", async () => {
  const run = await ran("rl_breaker.json");
  const time = run.table.columns[0];
  const [close, open] = run.events;
  assert.deepEqual([close.component, close.action, open.action], ["CB1", "close", "open"]);
  const closing = rowAtOrBefore(time, close.time_s);
  assert.equal(time[closing], 0.005);
  const state = (row) => frameOf(run, row).states.get("component:CB1");
  // Before: open. At the row of the event: still open, about to close (the
  // row at that time was computed with the contact open). After: closed.
  assert.deepEqual(state(closing - 1), { on: false, acting: false, word: "open" });
  assert.deepEqual(state(closing), { on: false, acting: true, word: "open ▸ closed" });
  assert.deepEqual(state(closing + 1), { on: true, acting: false, word: "closed" });
  const opening = rowAtOrBefore(time, open.time_s);
  assert.deepEqual(state(opening), { on: true, acting: true, word: "closed ▸ open" });
  assert.deepEqual(state(opening + 1), { on: false, acting: false, word: "open" });
  assert.deepEqual(state(run.table.rows - 1), { on: false, acting: false, word: "open" });
  // In the picture: the classes of the component and the word below it.
  const at = (row) => svgOf(run, frameOf(run, row));
  assert.ok(at(closing - 1).includes('class="item circuit state-off"'));
  assert.ok(at(closing).includes('class="item circuit state-off acting"'));
  assert.ok(at(closing + 1).includes('class="item circuit state-on"'));
  assert.equal(slotText(at(closing + 1), "component:CB1|state"), "closed");
  // Without a run nothing says a state.
  assert.ok(!/state-on|state-off/.test(toSvg(run.placed.circuit, "Circuit")));
  // The current agrees: none while open, some while closed.
  assert.equal(cell(run.csv, "I(CB1)", closing), 0);
  assert.notEqual(cell(run.csv, "I(CB1)", closing + 5), 0);
  // From event to event.
  assert.equal(neighbouringEvent(run.timeline, time, 0, 1), close.time_s);
  assert.equal(neighbouringEvent(run.timeline, time, closing, 1), open.time_s);
  assert.equal(neighbouringEvent(run.timeline, time, opening, 1), null);
  assert.equal(neighbouringEvent(run.timeline, time, opening, -1), close.time_s);
  assert.equal(neighbouringEvent(run.timeline, time, closing, -1), null);
});

test("a diode conducts in the rows that the events say, and its current agrees", async () => {
  for (const [file, diode] of [["rectifier.json", "D1"], ["rectifier.json", "D2"], ["buck.json", "D1"], ["buck_interpolated.json", "D1"]]) {
    const run = await ran(file);
    const current = run.table.columns[run.table.header.indexOf(`I(${diode})`)];
    const unit = run.timeline.units.get(`component:${diode}`).list[0];
    assert.ok(unit.times.length >= 10, file);
    let peak = 0;
    for (const v of current) peak = Math.max(peak, Math.abs(v));
    // Every 7th row: where the current is clearly there, the diode
    // conducts; where it blocks, there is no more than its leakage.
    let on = 0;
    let off = 0;
    for (let row = 0; row < run.table.rows; row += 7) {
      const state = frameOf(run, row).states.get(`component:${diode}`);
      const i = Math.abs(current[row]);
      if (i > 0.01 * peak) assert.equal(state.on, true, `${file} ${diode} row ${row}`);
      if (!state.on) assert.ok(i < 1e-3 * peak, `${file} ${diode} row ${row}: ${i} A while blocking`);
      if (state.on) on += 1;
      else off += 1;
    }
    assert.ok(on > 20 && off > 20, `${file} ${diode}: ${on} rows on, ${off} off`);
  }
});

test("a fault is on in its phases between its events", async () => {
  const run = await ran("three_phase_fault.json");
  const time = run.table.columns[0];
  const start = rowAtOrBefore(time, 0.1);
  const word = (row) => frameOf(run, row).states.get("component:F1").word;
  assert.equal(word(0), "no fault");
  assert.equal(word(start), "no fault ▸ fault a");
  assert.equal(word(start + 1), "fault a");
  assert.equal(word(run.table.rows - 1), "no fault");
  // All three phases, cleared one by one at their current zeros.
  const areas = await ran("line_areas.json");
  const words = new Set();
  for (let row = 0; row < areas.table.rows; row += 1) words.add(frameOf(areas, row).states.get("component:F1").word.split(" ▸ ")[0]);
  assert.ok(words.has("no fault") && words.has("fault abc"), [...words].join(", "));
  assert.ok([...words].some((w) => /^fault [abc]{1,2}$/.test(w)), [...words].join(", "));
});

test("a switch that starts closed is closed until its event, one without events keeps its state", () => {
  const terminal = (name, node) => ({ name, path: `x.${name}`, nodes: [node] });
  const part = (name, k) => ({ name, type: "switch", path: `components[${k}]`, terminals: [terminal("nodes[0]", "a"), terminal("nodes[1]", "b")] });
  const topology = { components: [part("S1", 0), part("S2", 1), part("S3", 2)], nodes: [{ name: "a" }, { name: "b" }], buses: [], controls: [], signals: [] };
  const model = { components: [{ initially_closed: true }, { initially_closed: true }, {}] };
  const timeline = stateTimeline(topology, model, [
    { time_s: 2, component: "S1", action: "open", interrupted_current_a: 0 },
    { time_s: 2, component: "E1", action: "step" },
  ]);
  const table = { header: ["time_s"], columns: [Float64Array.of(0, 1, 2, 3)], rows: 4 };
  const words = (row) => [...frameAt({ binding: null, timeline, table, row }).states.values()].map((s) => s.word);
  assert.deepEqual(words(0), ["closed", "closed", "open"]);
  assert.deepEqual(words(2), ["closed ▸ open", "closed", "open"]);
  assert.deepEqual(words(3), ["open", "closed", "open"]);
  // The step of a source is an instant to go to, not a state.
  assert.deepEqual([...timeline.times], [2]);
});

test("a press on a component gives its columns, in the order of the result", async () => {
  const fault = await ran("three_phase_fault.json");
  assert.deepEqual(columnsOf(fault.binding, "component:T1"), ["I(T1.0.a)", "I(T1.0.b)", "I(T1.0.c)", "I(T1.1.a)", "I(T1.1.b)", "I(T1.1.c)"]);
  assert.deepEqual(columnsOf(fault.binding, "component:F1"), ["I(F1.a)", "I(F1.g)"]);
  assert.deepEqual(columnsOf(fault.binding, "bus:b2"), ["V(b2.a)", "V(b2.b)", "V(b2.c)"]);
  assert.deepEqual(columnsOf(fault.binding, "component:G1"), []);
  const buck = await ran("buck.json");
  assert.deepEqual(columnsOf(buck.binding, "component:L1"), ["I(L1)"]);
  assert.deepEqual(columnsOf(buck.binding, "node:out"), ["V(out)"]);
  assert.deepEqual(columnsOf(buck.binding, "signal:pwm"), ["S(pwm)"]);
  // The columns are those of the result, each owned once: switching them on
  // one by one is what the check boxes of the waveforms do.
  for (const file of exampleFiles) {
    const run = await ran(file);
    const owned = [...run.binding.byOwner.keys()].flatMap((owner) => columnsOf(run.binding, owner));
    assert.deepEqual([...owned].sort(), run.table.header.slice(1).sort(), file);
  }
  // What can be pressed for its columns says so in the picture.
  const svg = svgOf(buck, frameOf(buck, 0));
  assert.match(svg, /class="rail node has-columns" data-rail="node:out" tabindex="-1" role="button"/);
  assert.match(svg, /class="rail signal has-columns" data-rail="signal:pwm"/);
  assert.ok(!/data-rail="node:n1" tabindex/.test(svg));
});

test("the same model, the same run and the same row give the same picture", async () => {
  for (const file of exampleFiles) {
    const first = await ran(file);
    runs.delete(file);
    const again = await ran(file);
    assert.notEqual(first, again);
    const row = Math.floor(first.table.rows / 2);
    const one = svgOf(first, frameOf(first, row)) + overviewSvg(first.placed.circuit);
    const two = svgOf(again, frameOf(again, row)) + overviewSvg(again.placed.circuit);
    assert.equal(one, two, file);
  }
  // Another row is another picture; the positions stay.
  const run = await ran("rlc.json");
  const [a, b] = [svgOf(run, frameOf(run, 100)), svgOf(run, frameOf(run, 101))];
  assert.notEqual(a, b);
  assert.equal(a.replace(/data-slot="[^"]*">[^<]*</g, ""), b.replace(/data-slot="[^"]*">[^<]*</g, ""));
});

test("a long name goes on over lines, it is not cut", async () => {
  assert.deepEqual(wrapLabel("R1", 14), ["R1"]);
  assert.deepEqual(wrapLabel("gen0_efd0_over_gain", 16), ["gen0_efd0_over_", "gain"]);
  assert.deepEqual(wrapLabel("← G1.terminal_voltage_pu", 21, 2), ["← G1.terminal_", "voltage_pu"]);
  assert.deepEqual(wrapLabel("=0.1(fdm_bus_voltage_reference)", 14), ["=0.1(fdm_bus_", "voltage_", "reference)"]);
  // Only what does not fit on three lines ends with "…".
  assert.deepEqual(wrapLabel("x".repeat(50), 14), ["x".repeat(14), "x".repeat(14), `${"x".repeat(13)}…`]);
  assert.deepEqual(wrapLabel("abcdefghijklmnopqrstuvwxyz", 10), ["abcdefghij", "klmnopqrst", "uvwxyz"]);
  // Whatever fits on the lines there are is whole.
  for (const name of ["load0_out0_sw_a", "transition_theta", "error_outside_band", "x".repeat(42)]) {
    assert.equal(wrapLabel(name, 16).join(""), name);
  }
  // Every name of every example is whole in the picture: components,
  // blocks, nodes, buses, signals, and the links to what a block measures.
  let names = 0;
  let long = 0;
  for (const file of exampleFiles) {
    const run = await ran(file);
    const found = new Set(texts(svgOf(run, null)));
    const wanted = [
      ...run.topology.components.map((c) => c.name),
      ...run.topology.controls.filter((c) => c.owner === undefined).map((c) => c.name),
      ...run.topology.buses.map((b) => b.name),
      ...run.topology.nodes.filter((node) => !node.ground && node.bus === undefined).map((node) => node.name),
      ...run.placed.controls.rails.map((rail) => rail.name),
      ...[...run.faces.circuit.items, ...run.faces.controls.items, ...run.faces.circuit.rails, ...run.faces.controls.rails].flatMap((x) => x.chips.map((chip) => chip.text)),
    ];
    for (const name of wanted) {
      assert.ok(found.has(name), `${file}: "${name}" is not whole in the diagram`);
      names += 1;
      if (name.length > 16) long += 1;
    }
    // Nothing but the name of a port (the field of a block, which is as
    // long as its wire; the signal has its whole name above its rail) ends
    // with "…".
    const cut = texts(svgOf(run, null).replace(/<text class="port-label.*?<\/text>/g, "")).filter((t) => t.endsWith("…"));
    assert.deepEqual(cut, [], file);
  }
  assert.ok(names > 500 && long > 15, `${names} names, ${long} of them longer than 16 characters`);
});

test("the overview has every rail and every symbol, and a frame for what is shown", async () => {
  const run = await ran("transition_three_times.json");
  for (const placed of [run.placed.circuit, run.placed.controls]) {
    const svg = overviewSvg(placed);
    assert.equal((svg.match(/<line class="rail/g) ?? []).length, placed.rails.length);
    assert.equal((svg.match(/<rect class="item/g) ?? []).length, placed.items.length);
    assert.equal((svg.match(/<rect class="viewport"/g) ?? []).length, 1);
    assert.ok(svg.includes(`viewBox="0 0 ${placed.width} ${placed.height}"`));
    assert.ok(!/NaN|undefined|Infinity/.test(svg));
  }
});

test("itemOwners returns member components for a reduced item", () => {
  const placed = { rails: [{ id: "bus:b0" }, { id: "bus:b1" }] };
  const reduced = {
    id: "between:b0:b1",
    category: "reduced",
    name: "2 components",
    members: ["L0", "P0"],
    ports: [
      { side: "L", rails: [{ rail: 0 }] },
      { side: "R", rails: [{ rail: 1 }] },
    ],
  };
  assert.deepEqual(itemOwners(placed, reduced), ["component:L0", "component:P0"]);
  assert.deepEqual(itemOwners(placed, { id: "component:R1", category: "circuit", ports: [] }), ["component:R1"]);
});
