// Sidecar layout schema, deterministic serialization, non-destructive merge,
// and diff extraction tests (no WebAssembly module required).
//
//   node --test web/tests/layout_sidecar.test.mjs

import assert from "node:assert/strict";
import { test } from "node:test";

import { circuitFace, controlsFace, place, wiresThroughSymbols } from "../src/layout.js";
import {
  LAYOUT_FORMAT,
  LAYOUT_VERSION,
  applyLayoutOverrides,
  extractLayoutOverrides,
  parseLayoutSidecar,
  serializeLayoutSidecar,
} from "../src/layout_sidecar.js";
import { toSvg } from "../src/render.js";
import { SYMBOL_WIDTH } from "../src/symbols.js";

const terminal = (name, nodes, more = {}) => ({ name, path: `x.${name}`, nodes, ...more });
const phases = ["a", "b", "c"];
const bus = (field, name) => terminal(field, phases.map((p) => `${name}.${p}`), { bus: name, phases });

function makeTopology(components, more = {}) {
  const nodes = [];
  const buses = [];
  for (const c of components) {
    for (const t of c.terminals) {
      if (t.bus !== undefined && !buses.some((b) => b.name === t.bus)) {
        buses.push({ name: t.bus, nodes: phases.map((p) => `${t.bus}.${p}`), components: [] });
      }
      for (const name of t.nodes) {
        if (!nodes.some((node) => node.name === name)) nodes.push({ name, components: [] });
      }
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

function sampleTopology() {
  const input = (field, signal) => ({ field, path: `controls.${field}`, signal });
  return makeTopology(
    [
      { name: "G1", type: "three_phase_source", terminals: [bus("bus", "b1"), terminal("neutral", ["gnd"])] },
      { name: "L1", type: "pi_line", terminals: [bus("buses[0]", "b1"), bus("buses[1]", "b2")] },
      { name: "R1", type: "resistor", terminals: [terminal("nodes[0]", ["b1.a"]), terminal("nodes[1]", ["b1.b"])] },
      {
        name: "S1",
        type: "controlled_switch",
        terminals: [terminal("nodes[0]", ["b2.a"]), terminal("nodes[1]", ["gnd"])],
        signals: [{ field: "gate", path: "components[3].gate", signal: "pwm" }],
      },
    ],
    {
      controls: [
        { name: "v", type: "measurement", path: "controls[0]", inputs: [], outputs: ["v"], measures: { type: "bus_voltage", bus: "b2" } },
        { name: "pwm", type: "comparator", path: "controls[1]", inputs: [input("inputs[0]", "v"), input("inputs[1]", "ref")], outputs: ["pwm"] },
      ],
      signals: [
        { name: "v", from: "v", to: [{ control: "pwm", field: "inputs[0]" }] },
        { name: "pwm", from: "pwm", to: [{ component: "S1", field: "gate" }] },
        { name: "ref", from: null, to: [{ control: "pwm", field: "inputs[1]" }] },
      ],
    },
  );
}

test("parseLayoutSidecar validates valid, empty, unknown-property, and invalid inputs", () => {
  assert.equal(LAYOUT_FORMAT, "openkeito-layout");
  assert.equal(LAYOUT_VERSION, "0.1");

  const valid = parseLayoutSidecar(
    JSON.stringify({
      format: LAYOUT_FORMAT,
      version: LAYOUT_VERSION,
      extraRoot: true,
      faces: {
        circuit: {
          extraFace: 1,
          rails: { "bus:b1": { dx: 20, note: "ignored" } },
          items: { "component:R1": { dy: 40 } },
        },
        extraUnknownFace: { rails: {} },
      },
    }),
  );
  assert.equal(valid.valid, true);
  assert.deepEqual(valid.errors, []);
  assert.ok(valid.warnings.length >= 3);
  assert.deepEqual(valid.layout, {
    format: LAYOUT_FORMAT,
    version: LAYOUT_VERSION,
    faces: {
      circuit: {
        rails: { "bus:b1": { dx: 20 } },
        items: { "component:R1": { dx: 0, dy: 40 } },
      },
      controls: { rails: {}, items: {} },
    },
  });

  const empty = parseLayoutSidecar({ format: LAYOUT_FORMAT, version: LAYOUT_VERSION });
  assert.equal(empty.valid, true);
  assert.deepEqual(empty.layout.faces, {
    circuit: { rails: {}, items: {} },
    controls: { rails: {}, items: {} },
  });

  for (const bad of [
    "{ not json",
    null,
    [],
    { version: LAYOUT_VERSION },
    { format: "other-format", version: LAYOUT_VERSION },
    { format: LAYOUT_FORMAT, version: "0.2" },
    { format: LAYOUT_FORMAT, version: LAYOUT_VERSION, faces: [] },
    { format: LAYOUT_FORMAT, version: LAYOUT_VERSION, faces: { circuit: { rails: { "bus:b1": { dx: NaN } } } } },
    { format: LAYOUT_FORMAT, version: LAYOUT_VERSION, faces: { circuit: { rails: { "bus:b1": { dx: Infinity } } } } },
    { format: LAYOUT_FORMAT, version: LAYOUT_VERSION, faces: { circuit: { items: { "component:R1": { dx: "20" } } } } },
    { format: LAYOUT_FORMAT, version: LAYOUT_VERSION, faces: { controls: { items: { "control:v": { dy: null } } } } },
  ]) {
    const res = parseLayoutSidecar(bad);
    assert.equal(res.valid, false);
    assert.ok(res.errors.length > 0);
    assert.equal(res.layout, null);
  }
});

test("serializeLayoutSidecar sorts keys deterministically and ends with a newline", () => {
  const first = serializeLayoutSidecar({
    version: LAYOUT_VERSION,
    format: LAYOUT_FORMAT,
    faces: {
      controls: { items: { "control:z": { dy: -10, dx: 5 }, "control:a": { dx: 0, dy: 20 } }, rails: {} },
      circuit: {
        items: { "component:R2": { dy: 40, dx: 0 }, "component:R1": { dx: -12, dy: 15 } },
        rails: { "bus:b2": { dx: 30 }, "bus:b1": { dx: -0 } },
      },
    },
  });
  const second = serializeLayoutSidecar({
    format: LAYOUT_FORMAT,
    version: LAYOUT_VERSION,
    faces: {
      circuit: {
        rails: { "bus:b1": { dx: 0 }, "bus:b2": { dx: 30 } },
        items: { "component:R1": { dx: -12, dy: 15 }, "component:R2": { dx: 0, dy: 40 } },
      },
      controls: { rails: {}, items: { "control:a": { dx: 0, dy: 20 }, "control:z": { dx: 5, dy: -10 } } },
    },
  });

  assert.equal(first, second);
  assert.ok(first.endsWith("\n"));
  assert.ok(first.indexOf('"bus:b1"') < first.indexOf('"bus:b2"'));
  assert.ok(first.indexOf('"component:R1"') < first.indexOf('"component:R2"'));
  assert.ok(first.indexOf('"circuit"') < first.indexOf('"controls"'));

  const reparsed = parseLayoutSidecar(first);
  assert.equal(reparsed.valid, true);
  assert.equal(serializeLayoutSidecar(reparsed.layout), first);

  assert.throws(() => serializeLayoutSidecar(null), TypeError);
  assert.throws(() => serializeLayoutSidecar({ faces: { circuit: { rails: { "bus:b1": { dx: NaN } } } } }), RangeError);
});

test("place() without or with empty sidecar matches baseline byte-for-byte", () => {
  const topology = sampleTopology();
  const circuit = circuitFace(topology);
  const controls = controlsFace(topology);

  const baseCircuit = place(circuit);
  const baseControls = place(controls);
  const baseCircuitSvg = toSvg(baseCircuit, "Circuit");
  const baseControlsSvg = toSvg(baseControls, "Controls");

  const emptySidecar = {
    format: LAYOUT_FORMAT,
    version: LAYOUT_VERSION,
    faces: { circuit: { rails: {}, items: {} }, controls: { rails: {}, items: {} } },
  };

  for (const opts of [{}, { sidecar: null }, { sidecar: emptySidecar }, { overrides: emptySidecar }]) {
    const c = place(circuit, opts);
    const k = place(controls, opts);
    assert.deepEqual(c, baseCircuit);
    assert.deepEqual(k, baseControls);
    assert.equal(toSvg(c, "Circuit"), baseCircuitSvg);
    assert.equal(toSvg(k, "Controls"), baseControlsSvg);
  }
});

test("applyLayoutOverrides moves rails and items consistently, preserves wires/dots, and ignores stale IDs", () => {
  const topology = sampleTopology();
  const circuit = circuitFace(topology);
  const controls = controlsFace(topology);
  const slots = new Map([
    ["bus:b1", new Set(["V"])],
    ["component:L1", new Set(["I0", "I1"])],
    ["component:S1", new Set(["state"])],
  ]);

  const baseCircuit = place(circuit, { slots });
  const baseCircuitSnapshot = structuredClone(baseCircuit);
  const baseControls = place(controls);

  const sidecar = {
    format: LAYOUT_FORMAT,
    version: LAYOUT_VERSION,
    faces: {
      circuit: {
        rails: {
          "bus:b2": { dx: 48 },
          "bus:stale_unknown": { dx: 999 },
        },
        items: {
          "component:L1": { dx: 18, dy: 160 },
          "component:R1": { dx: 0, dy: 200 },
          "component:S1": { dx: 10, dy: 24 },
          "component:stale_unknown": { dx: 100, dy: 100 },
        },
      },
      controls: {
        rails: { "signal:v": { dx: 25 } },
        items: { "control:pwm": { dx: 14, dy: 30 }, "sink:S1": { dx: 8, dy: 20 } },
      },
    },
  };

  const placedCircuit = place(circuit, { slots, sidecar });
  const placedControls = place(controls, { sidecar });

  // Non-destructive: baseline untouched.
  assert.deepEqual(baseCircuit, baseCircuitSnapshot);

  // Unregistered component (G1) and rail (bus:b1) stay at their auto-placed coordinates.
  const baseG1 = baseCircuit.items.find((i) => i.id === "component:G1");
  const nextG1 = placedCircuit.items.find((i) => i.id === "component:G1");
  assert.equal(nextG1.x, baseG1.x);
  assert.equal(nextG1.y, baseG1.y);
  assert.equal(placedCircuit.rails[0].x, baseCircuit.rails[0].x);

  // Moved rail b2 + moved line L1: wires and dots track both endpoints.
  const b2Index = placedCircuit.rails.findIndex((r) => r.id === "bus:b2");
  const baseB2 = baseCircuit.rails[b2Index];
  const nextB2 = placedCircuit.rails[b2Index];
  assert.equal(nextB2.x, baseB2.x + 48);

  const baseL1 = baseCircuit.items.find((i) => i.id === "component:L1");
  const nextL1 = placedCircuit.items.find((i) => i.id === "component:L1");
  assert.equal(nextL1.x, baseL1.x + 18);
  assert.equal(nextL1.y, baseL1.y + 160);
  assert.equal(nextL1.symbol.x, baseL1.symbol.x + 18);
  assert.equal(nextL1.symbol.y, baseL1.symbol.y + 160);
  assert.equal(nextL1.labelX, baseL1.labelX + 18);
  assert.equal(nextL1.labelY, baseL1.labelY + 160);
  assert.equal(nextL1.values[0].x, baseL1.values[0].x + 18);
  assert.equal(nextL1.values[0].y, baseL1.values[0].y + 160);

  // L1 wires must reach rail b1 and moved rail b2 at nextL1.symbol.y.
  const b1X = placedCircuit.rails[0].x;
  const l1DotXs = nextL1.dots.map((d) => d.x).sort((a, b) => a - b);
  assert.deepEqual(l1DotXs, [b1X, nextB2.x].sort((a, b) => a - b));
  for (const dot of nextL1.dots) {
    assert.equal(dot.y, nextL1.symbol.y);
  }
  assert.ok(nextL1.wires.some((w) => w.some(([x, y]) => x === nextB2.x && y === nextL1.symbol.y)));
  assert.ok(nextL1.wires.some((w) => w.some(([x, y]) => x === nextL1.symbol.x && y === nextL1.symbol.y)));
  assert.ok(nextL1.wires.some((w) => w.some(([x, y]) => x === nextL1.symbol.x + SYMBOL_WIDTH && y === nextL1.symbol.y)));

  // S1 moved: ground symbol, chips, values, and rail b2 dot/y2 stay consistent.
  const baseS1 = baseCircuit.items.find((i) => i.id === "component:S1");
  const nextS1 = placedCircuit.items.find((i) => i.id === "component:S1");
  assert.equal(nextS1.grounds[0].x, baseS1.grounds[0].x + 10);
  assert.equal(nextS1.grounds[0].y, baseS1.grounds[0].y + 24);
  assert.equal(nextS1.chips[0].x, baseS1.chips[0].x + 10);
  assert.equal(nextS1.chips[0].y, baseS1.chips[0].y + 24);

  // R1 (feedback/against on b1) moved vertically: tags and dots follow, rail y2 covers all dots.
  const nextR1 = placedCircuit.items.find((i) => i.id === "component:R1");
  for (const dot of placedCircuit.dots) {
    if (dot.rail !== null) {
      const rail = placedCircuit.rails[dot.rail];
      assert.equal(dot.x, rail.x);
      assert.ok(dot.y >= rail.y1 && dot.y <= rail.y2);
    }
  }
  assert.equal(nextR1.tags.length, 2);
  assert.ok(nextR1.tags.every((t) => t.y === nextR1.symbol.y - 3 || t.y > nextR1.symbol.y));
  assert.deepEqual(wiresThroughSymbols(placedCircuit), []);

  // Controls face: arrows, portLabels, wires, and dots stay consistent.
  const basePwm = baseControls.items.find((i) => i.id === "control:pwm");
  const nextPwm = placedControls.items.find((i) => i.id === "control:pwm");
  assert.equal(nextPwm.arrows[0].x, basePwm.arrows[0].x + 14);
  assert.equal(nextPwm.arrows[0].y, basePwm.arrows[0].y + 30);
  const baseSink = baseControls.items.find((i) => i.id === "sink:S1");
  const nextSink = placedControls.items.find((i) => i.id === "sink:S1");
  assert.equal(nextSink.portLabels[0].x, baseSink.portLabels[0].x + 8);
  assert.equal(nextSink.portLabels[0].y, baseSink.portLabels[0].y + 20);
  for (const dot of placedControls.dots) {
    if (dot.rail !== null) {
      const rail = placedControls.rails[dot.rail];
      assert.equal(dot.x, rail.x);
      assert.ok(dot.y >= rail.y1 && dot.y <= rail.y2);
    }
  }
  assert.deepEqual(wiresThroughSymbols(placedControls), []);

  // Canvas bounds accommodate moved items and rails.
  assert.equal(placedCircuit.width, baseCircuit.width + 10);
  assert.ok(placedCircuit.height > baseCircuit.height);

  // SVG renders cleanly with no NaN / undefined / Infinity.
  const svg = toSvg(placedCircuit, "Circuit") + toSvg(placedControls, "Controls");
  assert.ok(!/NaN|undefined|Infinity/.test(svg));

  // Round-trip extraction produces minimal deterministic overrides and reapplies identically.
  const extracted = extractLayoutOverrides(
    { circuit: placedCircuit, controls: placedControls },
    { circuit: baseCircuit, controls: baseControls },
  );
  assert.deepEqual(extracted, {
    format: LAYOUT_FORMAT,
    version: LAYOUT_VERSION,
    faces: {
      circuit: {
        rails: { "bus:b2": { dx: 48 } },
        items: {
          "component:L1": { dx: 18, dy: 160 },
          "component:R1": { dx: 0, dy: 200 },
          "component:S1": { dx: 10, dy: 24 },
        },
      },
      controls: {
        rails: { "signal:v": { dx: 25 } },
        items: { "control:pwm": { dx: 14, dy: 30 }, "sink:S1": { dx: 8, dy: 20 } },
      },
    },
  });

  const reappliedCircuit = applyLayoutOverrides(baseCircuit, extracted.faces.circuit);
  const reappliedControls = applyLayoutOverrides(baseControls, extracted.faces.controls);
  assert.deepEqual(reappliedCircuit, placedCircuit);
  assert.deepEqual(reappliedControls, placedControls);
});

test("wire reroutes via escape path when a rail crosses past an item symbol", () => {
  const topology = makeTopology([
    { name: "E1", type: "voltage_source", terminals: [terminal("nodes[0]", ["n1"]), terminal("nodes[1]", ["gnd"])] },
    { name: "R1", type: "resistor", terminals: [terminal("nodes[0]", ["n1"]), terminal("nodes[1]", ["n2"])] },
  ]);
  const circuit = circuitFace(topology);
  const base = place(circuit);
  // Move rail n1 far to the right, past R1's left port.
  const moved = applyLayoutOverrides(base, {
    rails: { "node:n1": { dx: 300 } },
    items: {},
  });
  const r1 = moved.items.find((i) => i.id === "component:R1");
  const n1Rail = moved.rails.find((r) => r.id === "node:n1");
  assert.ok(r1.dots.some((d) => d.x === n1Rail.x));
  assert.deepEqual(wiresThroughSymbols(moved), []);
});
