// The diagram without a browser: the symbols against the catalog, and the
// placed faces of every example against its topology.
//
// Run through modules.test.mjs. The topology of the examples and the catalog
// come from the WebAssembly module, which must be built first
// (crates/openkeito-wasm/README.md, "Build").

import assert from "node:assert/strict";
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import { MAX_COMPONENTS, circuitFace, compareWithTopology, controlsFace, place, wiresThroughSymbols } from "../src/layout.js";
import { toSvg } from "../src/render.js";
import { CIRCUIT, CONTROL, symbolsWithoutType, typesWithoutSymbol } from "../src/symbols.js";

const root = new URL("../../", import.meta.url);
const pkg = new URL("crates/openkeito-wasm/pkg/", root);

async function module() {
  assert.ok(
    existsSync(new URL("openkeito_wasm_bg.wasm", pkg)),
    "the WebAssembly module is not built: see crates/openkeito-wasm/README.md, \"Build\"",
  );
  const wasm = await import(new URL("openkeito_wasm.js", pkg).href);
  wasm.initSync({ module: readFileSync(new URL("openkeito_wasm_bg.wasm", pkg)) });
  return wasm;
}

const examples = readdirSync(new URL("examples/", root))
  .filter((f) => f.endsWith(".json"))
  .sort()
  .map((file) => [file, readFileSync(new URL(`examples/${file}`, root), "utf8")]);

function drawn(topology, options) {
  const faces = { circuit: circuitFace(topology, options), controls: controlsFace(topology) };
  const placed = { circuit: place(faces.circuit), controls: place(faces.controls) };
  return { faces, placed, svg: toSvg(placed.circuit, "Circuit") + toSvg(placed.controls, "Controls") };
}

test("every type of the catalog has a symbol, and every symbol a type", async () => {
  const wasm = await module();
  const catalog = JSON.parse(wasm.components());
  assert.equal(catalog.length, 59);
  assert.deepEqual(typesWithoutSymbol(catalog), []);
  assert.deepEqual(symbolsWithoutType(catalog), []);
  assert.equal(Object.keys(CIRCUIT).length, catalog.filter((e) => e.category === "circuit").length);
  assert.equal(Object.keys(CONTROL).length, catalog.filter((e) => e.category === "control").length);
  // The check itself: a type that is not in the tables is found.
  assert.deepEqual(typesWithoutSymbol([...catalog, { type: "new_part", category: "circuit" }]), ["new_part"]);
});

test("the diagram of every example shows what its topology has", async () => {
  const wasm = await module();
  assert.ok(examples.length >= 21);
  let components = 0;
  let blocks = 0;
  for (const [file, text] of examples) {
    const topology = JSON.parse(wasm.topology(text));
    assert.equal(topology.valid, true, file);
    const { faces, placed, svg } = drawn(topology);
    assert.deepEqual(compareWithTopology(topology, faces.circuit, faces.controls), [], file);
    // The numbers, counted in the SVG itself.
    const count = (pattern) => (svg.match(pattern) ?? []).length;
    assert.equal(count(/class="item circuit[ "]/g), topology.components.length, file);
    assert.equal(count(/class="item control[ "]/g), topology.controls.filter((c) => c.owner === undefined).length, file);
    const rails = topology.buses.length + topology.nodes.filter((node) => !node.ground && node.bus === undefined).length;
    assert.equal(count(/class="rail (bus|node)[ "]/g), rails, file);
    // No wire and no rail runs through the symbol of a component.
    assert.deepEqual(wiresThroughSymbols(placed.circuit), [], file);
    assert.deepEqual(wiresThroughSymbols(placed.controls), [], file);
    assert.ok(!/NaN|undefined|Infinity/.test(svg), file);
    assert.ok(!svg.includes("unknown-box"), file);
    components += topology.components.length;
    blocks += placed.controls.items.length;
  }
  assert.ok(components > 100 && blocks > 100, `${components} components, ${blocks} boxes`);
});

test("the same model gives the same SVG", async () => {
  const wasm = await module();
  for (const [file, text] of examples) {
    const first = drawn(JSON.parse(wasm.topology(text))).svg;
    const again = drawn(JSON.parse(wasm.topology(text))).svg;
    assert.equal(first, again, file);
    assert.ok(first.length > 1000, file);
  }
});

// ---- small documents written out, without the module ----

const terminal = (name, nodes, more = {}) => ({ name, path: `x.${name}`, nodes, ...more });
const phases = ["a", "b", "c"];
const bus = (field, name) => terminal(field, phases.map((p) => `${name}.${p}`), { bus: name, phases });

function document(components, more = {}) {
  const nodes = [];
  const buses = [];
  for (const c of components) {
    for (const t of c.terminals) {
      if (t.bus !== undefined && !buses.some((b) => b.name === t.bus)) buses.push({ name: t.bus, nodes: phases.map((p) => `${t.bus}.${p}`), components: [] });
      for (const name of t.nodes) if (!nodes.some((node) => node.name === name)) nodes.push({ name, components: [] });
    }
  }
  for (const node of nodes) {
    if (node.name === "gnd") node.ground = true;
    const [of, phase] = node.name.split(".");
    if (buses.some((b) => b.name === of) && phases.includes(phase)) Object.assign(node, { bus: of, phase });
  }
  return {
    components: components.map((c, k) => ({ path: `components[${k}]`, ...c })),
    nodes,
    buses,
    controls: [],
    signals: [],
    diagnostics: [],
    ...more,
  };
}

test("a symbol is mirrored when its first terminal is on the right", () => {
  const topology = document([
    { name: "E1", type: "voltage_source", terminals: [terminal("nodes[0]", ["a"]), terminal("nodes[1]", ["gnd"])] },
    { name: "D1", type: "diode", terminals: [terminal("nodes[0]", ["gnd"]), terminal("nodes[1]", ["a"])] },
    { name: "D2", type: "diode", terminals: [terminal("nodes[0]", ["b"]), terminal("nodes[1]", ["a"])] },
    { name: "D3", type: "diode", terminals: [terminal("nodes[0]", ["a"]), terminal("nodes[1]", ["b"])] },
  ]);
  const { faces, placed } = drawn(topology);
  const flip = Object.fromEntries(faces.circuit.items.map((i) => [i.name, i.flip]));
  assert.deepEqual(flip, { E1: false, D1: true, D2: true, D3: false });
  // The earth symbol of D1 is on the side of its anode: to the right of
  // the mirrored symbol, away from the rail of "a".
  const d1 = placed.circuit.items.find((i) => i.name === "D1");
  assert.equal(d1.grounds.length, 1);
  assert.ok(d1.grounds[0].x > d1.symbol.x + d1.symbol.width);
  assert.deepEqual(compareWithTopology(topology, faces.circuit, faces.controls), []);
});

test("a phase of a bus is a tag at the bus, and both ends at one bus step around the symbol", () => {
  const topology = document([
    { name: "G1", type: "three_phase_source", terminals: [bus("bus", "b1"), terminal("neutral", ["gnd"])] },
    { name: "R1", type: "resistor", terminals: [terminal("nodes[0]", ["b1.a"]), terminal("nodes[1]", ["b1.b"])] },
    { name: "LD", type: "three_phase_load", terminals: [bus("bus", "b1"), terminal("neutral", ["n"])] },
    { name: "F1", type: "fault", terminals: [terminal("bus", ["b1.c"], { bus: "b1", phases: ["c"] })] },
  ]);
  const { faces, placed } = drawn(topology);
  assert.equal(placed.circuit.rails.length, 2); // b1 and the star point n
  const tags = (name) => placed.circuit.items.find((i) => i.name === name).tags.map((t) => t.text);
  assert.deepEqual(tags("G1"), []);
  assert.deepEqual(tags("R1"), ["a", "b"]);
  assert.deepEqual(tags("F1"), ["c"]);
  // R1: one wire straight to the bus, one down and back under the symbol.
  const r1 = placed.circuit.items.find((i) => i.name === "R1");
  assert.equal(r1.wires.filter((w) => w.length === 3).length, 1);
  assert.equal(r1.dots.filter((d) => d.rail === 0).length, 2);
  assert.deepEqual(wiresThroughSymbols(placed.circuit), []);
  assert.deepEqual(compareWithTopology(topology, faces.circuit, faces.controls), []);
});

test("a type without a symbol is a box with its name, a diagnostic a mark", () => {
  const topology = document(
    [
      { name: "X1", type: "new_part", terminals: [terminal("nodes[0]", ["a"]), terminal("nodes[1]", ["b"]), terminal("nodes[2]", ["c"])] },
      { name: 'R"<1>&', type: "resistor", terminals: [terminal("nodes[0]", ["a"]), terminal("nodes[1]", ["gnd"])] },
    ],
    {
      diagnostics: [
        { severity: "warning", code: "W1", path: "components[0]", components: ["X1"], controls: [], nodes: ["b"], buses: [] },
        { severity: "error", code: "E1", path: "components[0]", components: ["X1"], controls: [], nodes: [], buses: [] },
      ],
    },
  );
  const { faces, placed, svg } = drawn(topology);
  assert.equal((svg.match(/unknown-box/g) ?? []).length, 1);
  assert.ok(svg.includes('class="item circuit error"'));
  assert.ok(svg.includes('class="rail node warning"'));
  assert.equal((svg.match(/class="badge (error|warning)"/g) ?? []).length, 2);
  // What the model names is text, never markup.
  assert.ok(svg.includes("R&quot;&lt;1&gt;&amp;") && !svg.includes("<1>"));
  assert.deepEqual(wiresThroughSymbols(placed.circuit), []);
  assert.deepEqual(compareWithTopology(topology, faces.circuit, faces.controls), []);
});

test("blocks, the signals between them, a loop, and the links between the faces", () => {
  const input = (field, signal) => ({ field, path: `controls.${field}`, signal });
  const topology = document(
    [
      { name: "E1", type: "voltage_source", terminals: [terminal("nodes[0]", ["n1"]), terminal("nodes[1]", ["gnd"])] },
      {
        name: "S1",
        type: "controlled_switch",
        terminals: [terminal("nodes[0]", ["n1"]), terminal("nodes[1]", ["gnd"])],
        signals: [{ field: "gate", path: "components[1].gate", signal: "pwm" }],
      },
    ],
    {
      controls: [
        { name: "e", type: "sum", path: "controls[0]", inputs: [input("inputs[0]", "v"), input("inputs[1]", "x")], outputs: ["e"] },
        { name: "x", type: "integrator", path: "controls[1]", inputs: [input("input", "e")], outputs: ["x"] },
        { name: "v", type: "measurement", path: "controls[2]", inputs: [], outputs: ["v"], measures: { type: "node_voltage", node: "n1" } },
        { name: "pwm", type: "comparator", path: "controls[3]", inputs: [input("inputs[0]", "x"), input("inputs[1]", "lost")], outputs: ["pwm"] },
      ],
      signals: [
        { name: "e", from: "e", to: [{ control: "x", field: "input" }] },
        { name: "x", from: "x", to: [{ control: "e", field: "inputs[1]" }, { control: "pwm", field: "inputs[0]" }] },
        { name: "v", from: "v", to: [{ control: "e", field: "inputs[0]" }] },
        { name: "pwm", from: "pwm", to: [{ component: "S1", field: "gate" }] },
        { name: "lost", from: null, to: [{ control: "pwm", field: "inputs[1]" }] },
      ],
    },
  );
  const { faces, placed, svg } = drawn(topology);
  assert.deepEqual(compareWithTopology(topology, faces.circuit, faces.controls), []);
  assert.deepEqual(wiresThroughSymbols(placed.controls), []);
  // A rail for every signal; the one that no block gives comes first.
  assert.deepEqual(placed.controls.rails.map((r) => r.name), ["lost", "v", "x", "e", "pwm"]);
  assert.equal(placed.controls.rails[0].missing, true);
  // The loop e -> x -> e: one of the two reads the other against the flow.
  const steps = placed.controls.items.flatMap((i) => i.wires).filter((w) => w.length === 3);
  assert.equal(steps.length, 1);
  // The switch is on the face of the controls, the links lead across.
  assert.ok(placed.controls.items.some((i) => i.id === "sink:S1"));
  assert.ok(svg.includes('data-face="controls" data-signal="pwm"'));
  assert.ok(svg.includes('data-face="circuit" data-node="n1"'));
  assert.ok(svg.includes('data-face="controls" data-control="v"'));
});

test("a model with too many components is drawn as its buses", () => {
  const components = [];
  for (let k = 0; k < 6; k += 1) {
    components.push({ name: `L${k}`, type: "pi_line", terminals: [bus("buses[0]", `b${k}`), bus("buses[1]", `b${(k + 1) % 6}`)] });
    components.push({ name: `P${k}`, type: "pi_line", terminals: [bus("buses[0]", `b${k}`), bus("buses[1]", `b${(k + 1) % 6}`)] });
    components.push({ name: `LD${k}`, type: "three_phase_load", terminals: [bus("bus", `b${k}`), terminal("neutral", ["gnd"])] });
    components.push({ name: `R${k}`, type: "resistor", terminals: [terminal("nodes[0]", [`b${k}.a`]), terminal("nodes[1]", [`x${k}`])] });
  }
  const topology = document(components);
  assert.ok(MAX_COMPONENTS >= 1000);
  const whole = drawn(topology);
  assert.equal(whole.faces.circuit.reduced, false);
  assert.equal(whole.placed.circuit.items.length, 24);
  const { faces, placed, svg } = drawn(topology, { maxComponents: 20 });
  assert.equal(faces.circuit.reduced, true);
  assert.match(faces.circuit.note, /24 components.*6 buses/);
  assert.equal(placed.circuit.rails.length, 6);
  assert.ok(placed.circuit.rails.every((r) => r.kind === "bus"));
  // One line for the two lines between two buses; no loads, no resistors.
  assert.equal(placed.circuit.items.length, 6);
  assert.ok(placed.circuit.items.every((i) => i.members.length === 2));
  assert.deepEqual(wiresThroughSymbols(placed.circuit), []);
  assert.ok(svg.includes("2 components"));
});
