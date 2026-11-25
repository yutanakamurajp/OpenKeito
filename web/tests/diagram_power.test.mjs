// Power-system diagram features: voltage/current heat levels (`data-heat`),
// three-phase expanded rails (`expandThreePhase`), safety limit fallback
// (`MAX_THREE_PHASE_EXPAND`), and case118-scale performance.
//
//   node --test web/tests/diagram_power.test.mjs

import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { test } from "node:test";

import { parseNumericCsv } from "../src/csv.js";
import {
  MAX_THREE_PHASE_EXPAND,
  circuitFace,
  controlsFace,
  place,
  wiresThroughSymbols,
} from "../src/layout.js";
import {
  bindColumns,
  classifyCurrentLevel,
  classifyVoltageLevel,
  columnsOf,
  computeNominalScales,
  describeValues,
  frameAt,
  slotsOf,
  stateTimeline,
} from "../src/overlay.js";
import { heatAttribute, toSvg } from "../src/render.js";

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

const textOf = (file) => readFileSync(new URL(`examples/${file}`, root), "utf8");

async function runExample(file) {
  const wasm = await module();
  const text = textOf(file);
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
  const scales = computeNominalScales(binding, table);
  return { topology, summary, table, binding, timeline, slots, scales };
}

const terminal = (name, nodes, more = {}) => ({ name, path: `x.${name}`, nodes, ...more });
const PHASES = ["a", "b", "c"];
const busTerminal = (field, name) => terminal(field, PHASES.map((p) => `${name}.${p}`), { bus: name, phases: PHASES });

function buildSyntheticTopology(components, controls = []) {
  const nodes = [];
  const buses = [];
  const nodeSet = new Map();
  const busSet = new Map();
  for (const c of components) {
    for (const t of c.terminals) {
      if (t.bus !== undefined && !busSet.has(t.bus)) {
        const busObj = { name: t.bus, nodes: PHASES.map((p) => `${t.bus}.${p}`), components: [] };
        busSet.set(t.bus, busObj);
        buses.push(busObj);
      }
      for (const name of t.nodes) {
        if (!nodeSet.has(name)) {
          const nodeObj = { name, components: [] };
          nodeSet.set(name, nodeObj);
          nodes.push(nodeObj);
        }
      }
    }
  }
  for (const node of nodes) {
    if (node.name === "gnd") node.ground = true;
    const dot = node.name.lastIndexOf(".");
    if (dot > 0) {
      const of = node.name.slice(0, dot);
      const phase = node.name.slice(dot + 1);
      if (busSet.has(of) && PHASES.includes(phase)) {
        node.bus = of;
        node.phase = phase;
      }
    }
  }
  return {
    components: components.map((c, k) => ({ path: `components[${k}]`, ...c })),
    nodes,
    buses,
    controls,
    signals: [],
    diagnostics: [],
  };
}

test("classifyVoltageLevel and classifyCurrentLevel follow threshold contracts", () => {
  assert.equal(classifyVoltageLevel(110, 100), "over");
  assert.equal(classifyVoltageLevel(-115, 100), "over");
  assert.equal(classifyVoltageLevel(90, 100), "high");
  assert.equal(classifyVoltageLevel(109.9, 100), "high");
  assert.equal(classifyVoltageLevel(45, 100), "mid");
  assert.equal(classifyVoltageLevel(-89.9, 100), "mid");
  assert.equal(classifyVoltageLevel(44.9, 100), "low");
  assert.equal(classifyVoltageLevel(0, 100), "low");
  assert.equal(classifyVoltageLevel(100, 0), "low");
  assert.equal(classifyVoltageLevel(NaN, 100), "low");
  assert.equal(classifyVoltageLevel(Infinity, 100), "low");

  assert.equal(classifyCurrentLevel(85, 100), "over");
  assert.equal(classifyCurrentLevel(-95, 100), "over");
  assert.equal(classifyCurrentLevel(50, 100), "high");
  assert.equal(classifyCurrentLevel(84.9, 100), "high");
  assert.equal(classifyCurrentLevel(15, 100), "mid");
  assert.equal(classifyCurrentLevel(-49.9, 100), "mid");
  assert.equal(classifyCurrentLevel(14.9, 100), "low");
  assert.equal(classifyCurrentLevel(0, 100), "low");
  assert.equal(classifyCurrentLevel(100, 0), "low");
  assert.equal(classifyCurrentLevel(NaN, 100), "low");
});

test("frameAt returns empty levels when row < 0 or data is missing, and heatAttribute formats data-heat", () => {
  const empty = frameAt({ binding: null, timeline: null, table: null, row: -1 });
  assert.ok(empty.levels instanceof Map);
  assert.equal(empty.levels.size, 0);
  assert.equal(heatAttribute(empty, "bus:b1"), "");
  assert.equal(heatAttribute(null, "bus:b1"), "");

  const fakeFrame = { levels: new Map([["bus:b1", { level: "over", ratio: 1.2, value: 120, unit: "V" }]]) };
  assert.equal(heatAttribute(fakeFrame, "bus:b1"), ' data-heat="over"');
  assert.equal(heatAttribute(fakeFrame, "bus:b2"), "");
});

test("rlc.json and three_phase_fault.json produce heat levels and data-heat attributes without breaking classes", async () => {
  const validLevels = new Set(["low", "mid", "high", "over"]);

  for (const file of ["rlc.json", "three_phase_fault.json"]) {
    const run = await runExample(file);
    const face = circuitFace(run.topology);
    const placed = place(face, { slots: run.slots });

    const seen = new Set();
    const step = Math.max(1, Math.floor(run.table.rows / 60));
    for (let row = 0; row < run.table.rows; row += step) {
      const frame = frameAt({
        binding: run.binding,
        timeline: run.timeline,
        table: run.table,
        row,
        phase: "a",
        scales: run.scales,
      });
      assert.ok(frame.levels instanceof Map && frame.levels.size > 0, `${file} row ${row}`);
      for (const [, entry] of frame.levels) {
        assert.ok(validLevels.has(entry.level), `${file}: unexpected level ${entry.level}`);
        assert.ok(Number.isFinite(entry.ratio) && entry.ratio >= 0);
        seen.add(entry.level);
      }

      // Calling without precomputed scales matches calling with scales.
      const fallbackFrame = frameAt({
        binding: run.binding,
        timeline: run.timeline,
        table: run.table,
        row,
        phase: "a",
      });
      assert.deepEqual([...fallbackFrame.levels.entries()], [...frame.levels.entries()], `${file} row ${row}`);
    }
    assert.deepEqual([...seen].sort(), ["high", "low", "mid", "over"], `${file} should observe all 4 levels`);

    const sampleRow = Math.floor(run.table.rows / 2);
    const sampleFrame = frameAt({
      binding: run.binding,
      timeline: run.timeline,
      table: run.table,
      row: sampleRow,
      phase: "a",
      scales: run.scales,
    });
    const svg = toSvg(placed, "Circuit", sampleFrame);
    assert.ok(/data-heat="(low|mid|high|over)"/.test(svg), `${file}: SVG must include data-heat`);
    assert.ok(!/NaN|undefined|Infinity/.test(svg), file);

    // Null frame produces identical SVG without data-heat attributes.
    const bareSvg = toSvg(placed, "Circuit", null);
    assert.ok(!bareSvg.includes("data-heat="));
  }

  // Existing state class contracts (`class="item circuit state-off"`, etc.) remain intact alongside data-heat.
  const breaker = await runExample("rl_breaker.json");
  const breakerPlaced = place(circuitFace(breaker.topology), { slots: breaker.slots });
  const closeIdx = breaker.table.columns[0].findIndex((t) => t >= 0.005);
  const svgBefore = toSvg(
    breakerPlaced,
    "Circuit",
    frameAt({ binding: breaker.binding, timeline: breaker.timeline, table: breaker.table, row: closeIdx - 1, scales: breaker.scales }),
  );
  const svgActing = toSvg(
    breakerPlaced,
    "Circuit",
    frameAt({ binding: breaker.binding, timeline: breaker.timeline, table: breaker.table, row: closeIdx, scales: breaker.scales }),
  );
  const svgAfter = toSvg(
    breakerPlaced,
    "Circuit",
    frameAt({ binding: breaker.binding, timeline: breaker.timeline, table: breaker.table, row: closeIdx + 5, scales: breaker.scales }),
  );
  assert.ok(svgBefore.includes('class="item circuit state-off"'));
  assert.ok(svgActing.includes('class="item circuit state-off acting"'));
  assert.ok(svgAfter.includes('class="item circuit state-on"'));
  assert.ok(svgAfter.includes('data-heat="'));
});

test("expandThreePhase expands buses into 3 phase rails, preserves connections, and respects default byte-identity", async () => {
  const wasm = await module();
  const examples = [
    "three_phase_fault.json",
    "synchronous_machine.json",
    "converter.json",
    "line_areas.json",
  ];

  for (const file of examples) {
    const topology = JSON.parse(wasm.topology(textOf(file)));

    // Default (`expandThreePhase = false`) matches explicit false byte-for-byte.
    const defaultFace = circuitFace(topology);
    const falseFace = circuitFace(topology, { expandThreePhase: false });
    const defaultSvg = toSvg(place(defaultFace), "Circuit");
    const falseSvg = toSvg(place(falseFace), "Circuit");
    assert.equal(defaultSvg, falseSvg, `${file}: default must match expandThreePhase: false`);
    assert.deepEqual(defaultFace, falseFace, `${file}: face objects must match`);

    const expandedFace = circuitFace(topology, { expandThreePhase: true });
    const placed = place(expandedFace);
    const svg = toSvg(placed, "Circuit");

    assert.equal(expandedFace.reduced, false, file);
    assert.equal(expandedFace.note, "", file);
    assert.deepEqual(wiresThroughSymbols(placed), [], `${file}: wires must not cross symbols`);
    assert.ok(!/NaN|undefined|Infinity/.test(svg), `${file}: invalid numbers in SVG`);

    // Every bus produces 3 phase rails `node:<bus>.a/b/c`.
    const railIds = new Set(placed.rails.map((r) => r.id));
    for (const bus of topology.buses) {
      for (const p of PHASES) {
        const id = `node:${bus.name}.${p}`;
        assert.ok(railIds.has(id), `${file}: missing phase rail ${id}`);
      }
    }

    // Expected total rail count: 3 per bus + non-bus non-ground nodes.
    const expectedRails =
      topology.buses.length * 3 +
      topology.nodes.filter((n) => !n.ground && n.bus === undefined).length;
    assert.equal(placed.rails.length, expectedRails, file);

    // Every component port reaches every rail of its terminal nodes (and has dots there).
    const itemsByName = new Map(placed.items.map((item) => [item.name, item]));
    for (const comp of topology.components) {
      const item = itemsByName.get(comp.name);
      assert.ok(item, `${file}: missing item ${comp.name}`);
      comp.terminals.forEach((term, k) => {
        const port = item.ports[k];
        assert.ok(port, `${file}: missing port ${comp.name}.${term.name}`);
        const wantedRails = new Set(
          term.nodes.filter((n) => expandedFace.nodeRail.has(n)).map((n) => expandedFace.nodeRail.get(n).rail),
        );
        const reachedRails = new Set(port.rails.map((r) => r.rail));
        assert.deepEqual(reachedRails, wantedRails, `${file}: ${comp.name}.${term.name} rail mismatch`);
        const expectedGround = term.nodes.some((n) => !expandedFace.nodeRail.has(n));
        assert.equal(port.ground, expectedGround, `${file}: ${comp.name}.${term.name} ground mismatch`);
        for (const r of reachedRails) {
          const rx = placed.rails[r].x;
          assert.ok(
            item.dots.some((d) => d.rail === r && d.x === rx),
            `${file}: ${comp.name}.${term.name} missing dot on ${placed.rails[r].id}`,
          );
        }
      });
    }
  }

  // Single-phase components attached to phase nodes of a bus connect directly to those phase rails,
  // and `frameAt` heat levels propagate to each phase rail.
  const run = await runExample("three_phase_fault.json");
  const expandedPlaced = place(circuitFace(run.topology, { expandThreePhase: true }), { slots: run.slots });
  const faultRow = run.table.columns[0].findIndex((t) => t >= 0.12);
  const faultFrame = frameAt({
    binding: run.binding,
    timeline: run.timeline,
    table: run.table,
    row: faultRow,
    phase: "a",
    scales: run.scales,
  });
  const expandedSvg = toSvg(expandedPlaced, "Circuit", faultFrame);
  assert.ok(expandedSvg.includes('data-rail="node:b2.a"'));
  assert.ok(expandedSvg.includes('data-rail="node:b2.b"'));
  assert.ok(expandedSvg.includes('data-rail="node:b2.c"'));
  assert.equal(faultFrame.levels.get("node:b2.a")?.level, "low");
  assert.match(expandedSvg, /data-rail="node:b2\.a"[^>]*data-heat="low"/);
});

test("expandThreePhase falls back to single-line representation when exceeding MAX_THREE_PHASE_EXPAND", () => {
  assert.equal(MAX_THREE_PHASE_EXPAND, 400);

  const makeChain = (count) => {
    const components = [
      { name: "G0", type: "three_phase_source", terminals: [busTerminal("bus", "b0"), terminal("neutral", ["gnd"])] },
    ];
    for (let i = 1; i < count; i += 1) {
      components.push({
        name: `L${i}`,
        type: "pi_line",
        terminals: [busTerminal("buses[0]", `b${i - 1}`), busTerminal("buses[1]", `b${i}`)],
      });
    }
    return buildSyntheticTopology(components);
  };

  // Exactly at the limit (400 components): expands into 3 phase rails per bus.
  const atLimit = makeChain(MAX_THREE_PHASE_EXPAND);
  const faceAt = circuitFace(atLimit, { expandThreePhase: true });
  assert.equal(faceAt.reduced, false);
  assert.equal(faceAt.note, "");
  assert.equal(faceAt.rails.length, MAX_THREE_PHASE_EXPAND * 3);
  assert.ok(faceAt.rails.every((r) => r.kind === "node"));

  // Above the limit (401 components): stays single-line and sets an explanatory note.
  const overLimit = makeChain(MAX_THREE_PHASE_EXPAND + 1);
  const faceOver = circuitFace(overLimit, { expandThreePhase: true });
  const placedOver = place(faceOver);
  assert.equal(faceOver.reduced, false);
  assert.notEqual(faceOver.note, "");
  assert.match(faceOver.note, /400/);
  assert.match(faceOver.note, /single-line/i);
  assert.equal(faceOver.rails.length, MAX_THREE_PHASE_EXPAND + 1);
  assert.ok(faceOver.rails.every((r) => r.kind === "bus"));
  assert.deepEqual(wiresThroughSymbols(placedOver), []);
});

test("case118-scale (118 buses, 353 components, 401 rows) updates scales, frames, and levels within 50 ms", () => {
  const BUS_COUNT = 118;
  const COMP_COUNT = 353;
  const ROWS = 401;

  const components = [];
  for (let b = 0; b < BUS_COUNT; b += 1) {
    if (b % 12 === 0) {
      components.push({
        name: `G${b}`,
        type: "three_phase_source",
        terminals: [busTerminal("bus", `b${b}`), terminal("neutral", ["gnd"])],
      });
    } else {
      components.push({
        name: `LD${b}`,
        type: "three_phase_load",
        terminals: [busTerminal("bus", `b${b}`), terminal("neutral", ["gnd"])],
      });
    }
  }
  for (let b = 0; b < BUS_COUNT - 1; b += 1) {
    components.push({
      name: `LINE${b}`,
      type: b % 9 === 0 ? "three_phase_transformer" : "pi_line",
      terminals: [busTerminal("buses[0]", `b${b}`), busTerminal("buses[1]", `b${b + 1}`)],
    });
  }
  let extra = 0;
  while (components.length < COMP_COUNT) {
    const from = (extra * 7) % BUS_COUNT;
    const to = (from + 3) % BUS_COUNT;
    components.push({
      name: `TIE${extra}`,
      type: "coupled_rl",
      terminals: [busTerminal("buses[0]", `b${from}`), busTerminal("buses[1]", `b${to}`)],
    });
    extra += 1;
  }
  assert.equal(components.length, COMP_COUNT);

  const topology = buildSyntheticTopology(components);
  assert.equal(topology.buses.length, BUS_COUNT);

  // Build synthetic result columns: 118*3 bus voltages + 118 branch currents.
  const header = ["time_s"];
  const columns = [new Float64Array(ROWS)];
  const resultColumns = [];
  for (let r = 0; r < ROWS; r += 1) columns[0][r] = r * 5e-4;

  const omega = 2 * Math.PI * 50;
  for (let b = 0; b < BUS_COUNT; b += 1) {
    for (let p = 0; p < 3; p += 1) {
      const colName = `V(b${b}.${PHASES[p]})`;
      header.push(colName);
      resultColumns.push({ name: colName, kind: "node_voltage", unit: "V", target: `b${b}.${PHASES[p]}` });
      const arr = new Float64Array(ROWS);
      const shift = (p * 2 * Math.PI) / 3;
      const amp = 53890 * (0.95 + 0.001 * (b % 20));
      for (let r = 0; r < ROWS; r += 1) {
        const sag = b === 5 && r > 100 && r < 180 ? 0.25 : r > 180 && r < 220 && b === 5 ? 1.18 : 1.0;
        arr[r] = amp * sag * Math.cos(omega * columns[0][r] - shift);
      }
      columns.push(arr);
    }
  }
  for (let b = 0; b < BUS_COUNT - 1; b += 1) {
    const compName = `LINE${b}`;
    const colName = `I(${compName}.0.a)`;
    header.push(colName);
    resultColumns.push({ name: colName, kind: "branch_current", unit: "A", target: compName });
    const arr = new Float64Array(ROWS);
    const amp = 400 + b * 2;
    for (let r = 0; r < ROWS; r += 1) {
      arr[r] = amp * Math.cos(omega * columns[0][r] - 0.3);
    }
    columns.push(arr);
  }

  const table = { header, columns, rows: ROWS };
  const binding = bindColumns(topology, resultColumns, header);
  const timeline = stateTimeline(topology, { components: [] }, []);
  const slots = slotsOf(binding, timeline);

  const faceSingle = circuitFace(topology);
  const placedSingle = place(faceSingle, { slots });
  const faceExpanded = circuitFace(topology, { expandThreePhase: true });
  const placedExpanded = place(faceExpanded, { slots });
  assert.equal(faceExpanded.rails.length, BUS_COUNT * 3);

  // Measure nominal scale computation + frameAt + level lookup/SVG update.
  const t0 = performance.now();
  const scales = computeNominalScales(binding, table);
  const tScales = performance.now() - t0;

  const t1 = performance.now();
  const frame = frameAt({ binding, timeline, table, row: 150, phase: "a", scales });
  const tFrame = performance.now() - t1;

  const t2 = performance.now();
  let coloredCount = 0;
  for (const rail of placedSingle.rails) if (frame.levels.has(rail.id)) coloredCount += 1;
  for (const item of placedSingle.items) if (frame.levels.has(item.id)) coloredCount += 1;
  const tLevels = performance.now() - t2;

  const t3 = performance.now();
  const svg = toSvg(placedSingle, "Circuit", frame);
  const tSvg = performance.now() - t3;

  const totalUpdateMs = tScales + tFrame + tLevels + tSvg;
  console.log(
    `[case118 benchmark] computeNominalScales=${tScales.toFixed(2)}ms, ` +
      `frameAt=${tFrame.toFixed(2)}ms, levelUpdate=${tLevels.toFixed(2)}ms, ` +
      `toSvg=${tSvg.toFixed(2)}ms, total=${totalUpdateMs.toFixed(2)}ms ` +
      `(colored=${coloredCount}, expandedRails=${placedExpanded.rails.length})`,
  );

  assert.ok(coloredCount >= 200);
  assert.ok(svg.includes('data-heat="'));
  assert.ok(totalUpdateMs < 50, `Expected total < 50ms, got ${totalUpdateMs.toFixed(2)}ms`);
});

test("frameAt keeps balanced steady-state AC buses and single-phase 50 Hz columns steady across a cycle", async () => {
  const text = textOf("three_phase_fault.json");
  const model = JSON.parse(text);
  const run = await runExample("three_phase_fault.json");
  const scales = computeNominalScales(run.binding, run.table, { model });

  const startRow = run.table.columns[0].findIndex((t) => t >= 0.04 - 1e-9);
  const endRow = run.table.columns[0].findIndex((t) => t >= 0.06 - 1e-9);
  assert.ok(startRow > 0 && endRow > startRow);

  const firstFrame = frameAt({
    binding: run.binding,
    timeline: run.timeline,
    table: run.table,
    row: startRow,
    phase: "a",
    scales,
  });
  const expectedT1Level = firstFrame.levels.get("component:T1")?.level;
  assert.ok(expectedT1Level);

  for (let row = startRow; row <= endRow; row += 1) {
    const frame = frameAt({
      binding: run.binding,
      timeline: run.timeline,
      table: run.table,
      row,
      phase: "a",
      scales,
    });
    assert.equal(frame.levels.get("bus:b2")?.level, "high", `bus:b2 flickered at row ${row}`);
    assert.equal(frame.levels.get("bus:b3")?.level, "high", `bus:b3 flickered at row ${row}`);
    for (const p of PHASES) {
      assert.equal(frame.levels.get(`node:b2.${p}`)?.level, "high", `node:b2.${p} flickered at row ${row}`);
      assert.equal(frame.levels.get(`node:b3.${p}`)?.level, "high", `node:b3.${p} flickered at row ${row}`);
    }
    assert.equal(frame.levels.get("component:T1")?.level, expectedT1Level, `T1 pre-fault level changed at row ${row}`);
  }

  // Single-phase 50 Hz voltage waveform uses trailing 1-cycle max|x| so zero crossings stay "high".
  const rows = 301;
  const dt = 2e-4;
  const time = Float64Array.from({ length: rows }, (_, k) => k * dt);
  const vac = Float64Array.from(time, (t) => 100 * Math.sin(2 * Math.PI * 50 * t));
  const table = { header: ["time_s", "V(n1)"], columns: [time, vac], rows };
  const topology = buildSyntheticTopology([
    { name: "E1", type: "voltage_source", terminals: [terminal("nodes[0]", ["n1"]), terminal("nodes[1]", ["gnd"])] },
  ]);
  const binding = bindColumns(topology, [{ name: "V(n1)", kind: "node_voltage", unit: "V", target: "n1" }], table.header);
  const acScales = computeNominalScales(binding, table, { frequencyHz: 50 });
  const cycStart = time.findIndex((t) => t >= 0.04 - 1e-9);
  const cycEnd = time.findIndex((t) => t >= 0.06 - 1e-9);
  for (let row = cycStart; row <= cycEnd; row += 1) {
    const f = frameAt({ binding, timeline: null, table, row, scales: acScales });
    assert.equal(f.levels.get("node:n1")?.level, "high", `single-phase 50 Hz flickered at row ${row}`);
  }
});

test("expandThreePhase preserves bus voltage slots and clickable phase rails", async () => {
  const run = await runExample("three_phase_fault.json");
  const model = JSON.parse(textOf("three_phase_fault.json"));
  const scales = computeNominalScales(run.binding, run.table, { model });

  const expandedFace = circuitFace(run.topology, { expandThreePhase: true });
  const expandedPlaced = place(expandedFace, { slots: run.slots });
  const sampleRow = run.table.columns[0].findIndex((t) => t >= 0.05);
  const frameB = frameAt({
    binding: run.binding,
    timeline: run.timeline,
    table: run.table,
    row: sampleRow,
    phase: "b",
    scales,
  });
  const svg = toSvg(expandedPlaced, "Circuit", frameB);

  for (const bus of ["b2", "b3"]) {
    for (const p of PHASES) {
      const railId = `node:${bus}.${p}`;
      const slotId = `${railId}|V`;
      const rail = expandedPlaced.rails.find((r) => r.id === railId);
      assert.ok(rail, `missing expanded rail ${railId}`);
      assert.equal(rail.columns, true, `${railId} must be clickable`);
      assert.equal(rail.value?.slot, slotId, `${railId} must reserve value slot`);
      assert.deepEqual(columnsOf(run.binding, railId), [`V(${bus}.${p})`]);
      assert.ok(describeValues(run.binding, run.table, sampleRow, railId).includes(`V(${bus}.${p})`));
      assert.match(svg, new RegExp(`class="rail node has-columns"[^>]*data-rail="${railId}"[^>]*role="button"`));
      assert.match(svg, new RegExp(`data-slot="${railId}\\|V">[^<]+</text>`));
    }
  }
});

test("three-phase current groups separate I0 and I1 and keep heat level invariant across phase selection", async () => {
  // 1. Transformer T1 in three_phase_fault.json has two separate three-phase current groups: I0 and I1.
  const faultRun = await runExample("three_phase_fault.json");
  const faultModel = JSON.parse(textOf("three_phase_fault.json"));
  const faultScales = computeNominalScales(faultRun.binding, faultRun.table, { model: faultModel });
  const t1Groups = faultScales.threePhaseGroups.filter((g) => g.owner === "component:T1" && g.unit === "A");
  assert.deepEqual(
    t1Groups.map((g) => g.slot).sort(),
    ["I0", "I1"],
  );
  assert.ok(faultScales.threePhaseGroupKeys.has("component:T1|I0"));
  assert.ok(faultScales.threePhaseGroupKeys.has("component:T1|I1"));

  // 2. G1 in synchronous_machine.json keeps identical heat level regardless of selected display phase.
  const smRun = await runExample("synchronous_machine.json");
  const smModel = JSON.parse(textOf("synchronous_machine.json"));
  const smScales = computeNominalScales(smRun.binding, smRun.table, { model: smModel });
  const row08 = smRun.table.columns[0].findIndex((t) => t >= 0.8 - 1e-9);
  assert.ok(row08 >= 0);

  for (const row of [row08, 0, Math.floor(smRun.table.rows / 2), smRun.table.rows - 1]) {
    const levelsByPhase = PHASES.map((phase) =>
      frameAt({
        binding: smRun.binding,
        timeline: smRun.timeline,
        table: smRun.table,
        row,
        phase,
        scales: smScales,
      }).levels.get("component:G1"),
    );
    assert.ok(levelsByPhase[0], `missing G1 level at row ${row}`);
    for (const info of levelsByPhase) {
      assert.equal(info.level, levelsByPhase[0].level, `G1 level differed by phase at row ${row}`);
      assert.equal(info.ratio, levelsByPhase[0].ratio, `G1 ratio differed by phase at row ${row}`);
    }
  }
});
