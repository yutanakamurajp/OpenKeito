// Pure JS inspector core tests: catalog/schema metadata for all 58 types,
// round-trip JSON edits, edit history, and mapping validate diagnostics.
//
// The catalog, schema, and validation come from the WebAssembly module, which
// must be built first (crates/openkeito-wasm/README.md, "Build").

import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { test } from "node:test";

import {
  addItem,
  buildCatalogIndex,
  createEditHistory,
  getOutputTypes,
  getSimulationFields,
  mapDiagnosticsToFields,
  parseModelForEdit,
  removeItem,
  setItemField,
  toggleOutputForTarget,
  updateItem,
  updateSimulationField,
} from "../src/inspector.js";
import { CONTROL, controlSymbol } from "../src/symbols.js";

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

const rlcText = readFileSync(new URL("examples/rlc.json", root), "utf8");

function buildScaffoldModelText() {
  const signalNames = [
    "pwm",
    "v_ref",
    "i_ref",
    "efd",
    "tm",
    "q_command",
    "v_out",
    "error",
    "duty",
    "carrier",
    "v_meas",
    "omega",
    "command",
    "p_ref",
    "df",
    "v",
    "i",
    "v_d",
    "v.d",
    "v.q",
    "fault_mode",
    "i_fault",
    "i_normal",
    "v.a",
    "v.b",
    "v.c",
    "pll.theta",
    "v_d_ref",
    "v_q_ref",
    "v_0_ref",
    "i_d_cmd",
    "i_q_cmd",
    "i_error_d",
    "i_error_q",
    "ff_d",
    "ff_q",
    "r_i_d",
    "v_ref.a",
    "v_ref.b",
    "v_ref.c",
    "i_ref.a",
    "i_ref.b",
    "i_ref.c",
    "x",
  ];
  const seqImpedance = {
    type: "sequence",
    r1_ohm: 0.1,
    l1_h: 0.001,
    r0_ohm: 0.3,
    l0_h: 0.003,
  };
  return JSON.stringify({
    format: "openkeito-model",
    version: "0.8",
    simulation: {
      time_step_s: 1e-6,
      end_time_s: 0.01,
      method: "2s-dirk",
    },
    components: [
      {
        type: "voltage_source",
        name: "E_ref",
        nodes: ["n_ref", "gnd"],
        waveform: { type: "dc", value: 10 },
      },
      { type: "resistor", name: "R_n1", nodes: ["n_ref", "n1"], resistance_ohm: 10 },
      { type: "resistor", name: "R_n2", nodes: ["n1", "n2"], resistance_ohm: 10 },
      { type: "resistor", name: "R_n3", nodes: ["n3", "gnd"], resistance_ohm: 10 },
      { type: "resistor", name: "R_out", nodes: ["out", "gnd"], resistance_ohm: 10 },
      {
        type: "three_phase_source",
        name: "G_ref",
        bus: "b_ref",
        line_voltage_rms_v: 400,
        frequency_hz: 50,
      },
      { type: "coupled_rl", name: "Z_ref_b1", buses: ["b_ref", "b1"], impedance: seqImpedance },
      { type: "coupled_rl", name: "Z_b1_b2", buses: ["b1", "b2"], impedance: seqImpedance },
      { type: "coupled_rl", name: "Z_b2_b3", buses: ["b2", "b3"], impedance: seqImpedance },
      { type: "coupled_rl", name: "Z_b3_b4", buses: ["b3", "b4"], impedance: seqImpedance },
      {
        type: "three_phase_load",
        name: "LD_b3",
        bus: "b3",
        rated_voltage_kv: 0.4,
        rated_frequency_hz: 50,
        active_power_mw: 0.01,
        reactive_power_mvar: 0,
      },
      {
        type: "three_phase_load",
        name: "LD_b4",
        bus: "b4",
        rated_voltage_kv: 0.4,
        rated_frequency_hz: 50,
        active_power_mw: 0.01,
        reactive_power_mvar: 0,
      },
    ],
    controls: signalNames.map((name) => ({ type: "constant", name, value: 0 })),
  }, null, 2);
}

test("buildCatalogIndex builds metadata for all 58 types, simulation, and outputs", async () => {
  const wasm = await module();
  const catalog = JSON.parse(wasm.components());
  const schema = JSON.parse(wasm.schema());
  const index = buildCatalogIndex(catalog, schema);

  assert.equal(index.all.length, 58);
  assert.equal(index.components.length, 26);
  assert.equal(index.controls.length, 32);
  assert.equal(index.byType.size, 58);

  for (const entry of catalog) {
    const info = index.get(entry.type);
    assert.ok(info, `missing type ${entry.type}`);
    assert.equal(info.type, entry.type);
    assert.equal(info.category, entry.category);
    assert.equal(info.summary, entry.summary);
    assert.equal(info.signConvention, entry.sign_convention);
    assert.deepEqual(info.example, entry.example);
    assert.equal(info.fields.length, entry.parameters.length);

    for (const field of info.fields) {
      assert.equal(typeof field.name, "string");
      assert.equal(typeof field.unit, "string");
      assert.equal(typeof field.required, "boolean");
      assert.equal(typeof field.description, "string");
      assert.ok(field.kind.length > 0);
    }
  }

  // Spot-check specific field kinds and constraints.
  const resistor = index.get("resistor");
  const rNodes = resistor.fields.find((f) => f.name === "nodes");
  assert.equal(rNodes.kind, "nodes");
  assert.equal(rNodes.required, true);
  const rVal = resistor.fields.find((f) => f.name === "resistance_ohm");
  assert.equal(rVal.kind, "number");
  assert.equal(rVal.unit, "ohm");

  const piLine = index.get("pi_line");
  const sections = piLine.fields.find((f) => f.name === "sections");
  assert.equal(sections.kind, "integer");
  assert.equal(sections.minimum, 0);

  const fnBlock = index.get("function");
  const fnKind = fnBlock.fields.find((f) => f.name === "function");
  assert.equal(fnKind.kind, "enum");
  assert.ok(fnKind.enumValues.includes("sin"));

  const simFields = getSimulationFields(index);
  assert.ok(simFields.length >= 9);
  assert.equal(simFields.find((f) => f.name === "time_step_s").kind, "number");
  assert.equal(simFields.find((f) => f.name === "time_step_s").unit, "s");
  assert.equal(simFields.find((f) => f.name === "time_step_s").required, true);
  assert.deepEqual(simFields.find((f) => f.name === "method").enumValues, ["2s-dirk", "trapezoidal"]);

  const outputTypes = getOutputTypes(index);
  assert.deepEqual(
    outputTypes.map((o) => o.type),
    ["node_voltage", "branch_current", "branch_voltage", "signal", "bus_voltage"],
  );
});

test("every one of the 58 catalog examples passes wasm.validate after addItem", async () => {
  const wasm = await module();
  const index = buildCatalogIndex(JSON.parse(wasm.components()), JSON.parse(wasm.schema()));
  const scaffold = buildScaffoldModelText();

  for (const def of index.all) {
    const section = def.category === "circuit" ? "components" : "controls";
    const added = addItem(scaffold, section, def.type, index);
    assert.equal(added.ok, true, `addItem failed for ${def.type}: ${added.error}`);
    const report = JSON.parse(wasm.validate(added.text, def.type));
    assert.equal(report.valid, true, `${def.type}: ${JSON.stringify(report.errors)}`);
  }
});

test("round-trip editing helpers preserve format/version and produce formatted JSON", async () => {
  const wasm = await module();
  const index = buildCatalogIndex(JSON.parse(wasm.components()), JSON.parse(wasm.schema()));

  assert.equal(parseModelForEdit("{").ok, false);
  assert.equal(parseModelForEdit("[]").ok, false);

  // 1. setItemField & updateItem & updateSimulationField
  let res = setItemField(rlcText, "components", 1, "resistance_ohm", 25.5);
  assert.equal(res.ok, true);
  assert.ok(res.text.endsWith("\n"));
  assert.ok(res.text.includes('  "version": "0.1"'));
  assert.equal(res.model.format, "openkeito-model");
  assert.equal(res.model.version, "0.1");
  assert.equal(res.model.components[1].resistance_ohm, 25.5);

  res = updateItem(res.text, "components", 2, { inductance_h: 0.02, initial_current_a: 0.5 });
  assert.equal(res.ok, true);
  assert.equal(res.model.components[2].inductance_h, 0.02);
  assert.equal(res.model.components[2].initial_current_a, 0.5);

  // Deleting an optional field with undefined
  res = setItemField(res.text, "components", 2, "initial_current_a", undefined);
  assert.equal(res.ok, true);
  assert.equal("initial_current_a" in res.model.components[2], false);

  res = updateSimulationField(res.text, "switching", "interpolated");
  assert.equal(res.ok, true);
  assert.equal(res.model.simulation.switching, "interpolated");

  res = updateSimulationField(res.text, "switching", undefined);
  assert.equal(res.ok, true);
  assert.equal("switching" in res.model.simulation, false);

  // 2. addItem: sequential unique names for collisions (e.g. R2, R3, duty, duty2)
  res = addItem(res.text, "components", "resistor", index, { patch: { nodes: ["n3", "gnd"] } });
  assert.equal(res.ok, true);
  assert.equal(res.model.components[4].name, "R2");
  res = addItem(res.text, "components", "resistor", index, { patch: { nodes: ["n3", "gnd"] } });
  assert.equal(res.ok, true);
  assert.equal(res.model.components[5].name, "R3");

  // Shared namespace collision across components and controls
  res = addItem(res.text, "controls", "constant", index);
  assert.equal(res.ok, true);
  assert.equal(res.model.controls[0].name, "duty");
  res = addItem(res.text, "controls", "pi", index, { patch: { input: "duty" } });
  assert.equal(res.ok, true);
  assert.equal(res.model.controls[1].name, "duty2");

  // 3. removeItem
  const countBefore = res.model.components.length;
  res = removeItem(res.text, "components", countBefore - 1);
  assert.equal(res.ok, true);
  assert.equal(res.model.components.length, countBefore - 1);
  // Remove added controls and extra resistor so rlc remains valid
  res = removeItem(res.text, "controls", 1);
  res = removeItem(res.text, "controls", 0);
  res = removeItem(res.text, "components", 4);
  assert.equal(res.ok, true);
  assert.equal(JSON.parse(wasm.validate(res.text)).valid, true);

  // 4. toggleOutputForTarget: add, remove (key-order independent), and omitted outputs handling
  const n1Output = { type: "node_voltage", node: "n1" };
  res = toggleOutputForTarget(res.text, n1Output);
  assert.equal(res.ok, true);
  assert.equal(res.model.outputs.length, 3);
  res = toggleOutputForTarget(res.text, { node: "n1", type: "node_voltage" });
  assert.equal(res.ok, true);
  assert.equal(res.model.outputs.length, 2);

  // Removing all outputs keeps outputs as [] instead of omitting it
  res = toggleOutputForTarget(res.text, { type: "node_voltage", node: "n3" });
  res = toggleOutputForTarget(res.text, { type: "branch_current", component: "L1" });
  assert.deepEqual(res.model.outputs, []);

  // Toggling when outputs is omitted creates an explicit outputs array
  const noOutputsModel = JSON.parse(rlcText);
  delete noOutputsModel.outputs;
  const noOutputsRes = toggleOutputForTarget(JSON.stringify(noOutputsModel), n1Output);
  assert.equal(noOutputsRes.ok, true);
  assert.deepEqual(noOutputsRes.model.outputs, [n1Output]);
  const toggledBack = toggleOutputForTarget(noOutputsRes.text, n1Output);
  assert.deepEqual(toggledBack.model.outputs, []);
});

test("createEditHistory manages push, consecutive duplicates, undo, redo, branch discard, and limit", () => {
  const history = createEditHistory("v0", 3);
  assert.equal(history.current(), "v0");
  assert.equal(history.canUndo(), false);
  assert.equal(history.canRedo(), false);

  // Consecutive identical push is ignored
  assert.equal(history.push("v0"), false);
  assert.equal(history.canUndo(), false);

  assert.equal(history.push("v1", "step 1"), true);
  assert.equal(history.push("v2", "step 2"), true);
  assert.equal(history.current(), "v2");
  assert.equal(history.currentLabel(), "step 2");
  assert.equal(history.canUndo(), true);
  assert.equal(history.canRedo(), false);

  assert.equal(history.undo(), "v1");
  assert.equal(history.current(), "v1");
  assert.equal(history.canRedo(), true);

  // Pushing after undo discards redo branch
  assert.equal(history.push("v2b", "branch"), true);
  assert.equal(history.canRedo(), false);
  assert.equal(history.redo(), null);
  assert.equal(history.undo(), "v1");
  assert.equal(history.redo(), "v2b");

  // Limit of 3 undo steps
  history.push("v3");
  history.push("v4");
  assert.equal(history.undo(), "v3");
  assert.equal(history.undo(), "v2b");
  assert.equal(history.undo(), "v1");
  assert.equal(history.canUndo(), false);
  assert.equal(history.undo(), null);

  history.reset("fresh");
  assert.equal(history.current(), "fresh");
  assert.equal(history.canUndo(), false);
  assert.equal(history.canRedo(), false);
});

test("mapDiagnosticsToFields maps both E001 json_path and validation path diagnostics", async () => {
  const wasm = await module();

  // 1. Multiple E001 errors (duplicate key + wrong type + unknown field)
  const badE001 = `{
  "format": "openkeito-model",
  "version": "0.6",
  "simulation": { "time_step_s": "oops", "end_time_s": 0.01 },
  "components": [
    { "type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": { "type": "dc", "value": 1 } },
    { "type": "resistor", "name": "R1", "nodes": ["n1", "gnd"], "resistance_ohm": 10, "resistance_ohm": 20 },
    { "type": "resistor", "name": "R2", "nodes": ["n1", "gnd"], "resistance_ohm": "bad", "extra": true }
  ],
  "outputs": [
    { "type": "node_voltage", "node": 123 }
  ]
}`;
  const e001Report = JSON.parse(wasm.validate(badE001));
  assert.equal(e001Report.valid, false);
  const mappedE001 = mapDiagnosticsToFields(e001Report);

  assert.equal(mappedE001.forSimulationField("time_step_s").length, 1);
  assert.equal(mappedE001.forSimulationField("time_step_s")[0].code, "E001");

  assert.equal(mappedE001.forField("components", 1, "resistance_ohm").length, 1);
  assert.equal(mappedE001.forItem("components", 2).length, 2);
  assert.equal(mappedE001.forField("components", 2, "extra").length, 1);
  assert.equal(mappedE001.forField("components", 2, "resistance_ohm").length, 1);
  assert.equal(mappedE001.forField("outputs", 0, "node").length, 1);
  assert.equal(mappedE001.byPath.get("components[2].resistance_ohm")?.length, 1);

  // 2. Semantic validation errors (E130 negative resistance + E150 unknown node + E110 time_step_s)
  const semanticBad = JSON.stringify({
    format: "openkeito-model",
    version: "0.6",
    simulation: { time_step_s: -1e-6, end_time_s: 0.01 },
    components: [
      { type: "voltage_source", name: "E1", nodes: ["n1", "gnd"], waveform: { type: "dc", value: 1 } },
      { type: "resistor", name: "R1", nodes: ["n1", "gnd"], resistance_ohm: -5 },
    ],
    outputs: [
      { type: "node_voltage", node: "missing_node" },
    ],
  });
  const semReport = JSON.parse(wasm.validate(semanticBad));
  assert.equal(semReport.valid, false);
  const mappedSem = mapDiagnosticsToFields(semReport);

  const simDiags = mappedSem.forSimulationField("time_step_s");
  assert.equal(simDiags.length, 1);
  assert.equal(simDiags[0].code, "E110");

  const r1Diags = mappedSem.forField("components", 1, "resistance_ohm");
  assert.equal(r1Diags.length, 1);
  assert.equal(r1Diags[0].code, "E130");
  assert.equal(mappedSem.forItem("components", 1).length, 1);

  const outDiags = mappedSem.forField("outputs", 0, "node");
  assert.equal(outDiags.length, 1);
  assert.equal(outDiags[0].code, "E150");

  // 3. Unmapped syntax error (line/column only)
  const syntaxReport = JSON.parse(wasm.validate("{ not json"));
  const mappedSyntax = mapDiagnosticsToFields(syntaxReport);
  assert.equal(mappedSyntax.unmapped.length, 1);
  assert.equal(mappedSyntax.unmapped[0].code, "E001");
});

test("CONTROL.piecewise_linear has an explicit symbol", () => {
  assert.ok(Object.hasOwn(CONTROL, "piecewise_linear"));
  const svg = controlSymbol("piecewise_linear", 1);
  assert.ok(svg.includes('class="block-box"'));
  assert.equal(svg.includes('class="unknown-box"'), false);
});

test("buildCatalogIndex unwraps nullable LimitValue ($ref and inline) and piecewise_linear numeric arrays", () => {
  const catalog = [
    {
      type: "limiter",
      category: "control",
      summary: "Signal limiter",
      sign_convention: "",
      parameters: [
        { name: "name", unit: "", required: true, description: "Signal name" },
        { name: "input", unit: "", required: true, description: "Input signal" },
        { name: "minimum", unit: "", required: false, description: "Lower limit" },
        { name: "maximum", unit: "", required: false, description: "Upper limit" },
      ],
      example: { type: "limiter", name: "lim", input: "u", minimum: -1, maximum: 1 },
    },
    {
      type: "piecewise_linear",
      category: "control",
      summary: "Piecewise linear lookup",
      sign_convention: "",
      parameters: [
        { name: "name", unit: "", required: true, description: "Signal name" },
        { name: "input", unit: "", required: true, description: "Input signal" },
        { name: "x", unit: "", required: true, description: "Breakpoints" },
        { name: "y", unit: "", required: true, description: "Values" },
      ],
      example: { type: "piecewise_linear", name: "pwl", input: "u", x: [0, 1], y: [0, 2] },
    },
  ];

  const schema = {
    $defs: {
      LimitValue: {
        anyOf: [{ type: "number" }, { type: "string" }],
      },
      Control: {
        oneOf: [
          {
            type: "object",
            required: ["type", "name", "input"],
            properties: {
              type: { const: "limiter" },
              name: { type: "string" },
              input: { type: "string" },
              // $ref + null form
              minimum: {
                description: "Lower limit (number or signal)",
                anyOf: [{ $ref: "#/$defs/LimitValue" }, { type: "null" }],
              },
              // Inline number | string | null form
              maximum: {
                description: "Upper limit (number or signal)",
                anyOf: [{ type: "number" }, { type: "string" }, { type: "null" }],
              },
            },
          },
          {
            type: "object",
            required: ["type", "name", "input", "x", "y"],
            properties: {
              type: { const: "piecewise_linear" },
              name: { type: "string" },
              input: { type: "string" },
              x: { type: "array", items: { type: "number" }, minItems: 2 },
              y: { type: "array", items: { type: "number" }, minItems: 2 },
            },
          },
        ],
      },
    },
  };

  const index = buildCatalogIndex(catalog, schema);

  const limiter = index.get("limiter");
  const minField = limiter.fields.find((f) => f.name === "minimum");
  assert.equal(minField.kind, "number_or_signal");
  assert.equal(minField.nullable, true);
  assert.equal(minField.required, false);

  const maxField = limiter.fields.find((f) => f.name === "maximum");
  assert.equal(maxField.kind, "number_or_signal");
  assert.equal(maxField.nullable, true);
  assert.equal(maxField.required, false);

  const pwl = index.get("piecewise_linear");
  const xField = pwl.fields.find((f) => f.name === "x");
  assert.equal(xField.kind, "array");
  assert.equal(xField.minItems, 2);
  assert.equal(xField.itemSchema?.kind, "number");

  const yField = pwl.fields.find((f) => f.name === "y");
  assert.equal(yField.kind, "array");
  assert.equal(yField.minItems, 2);
  assert.equal(yField.itemSchema?.kind, "number");
});

test("buildCatalogIndex handles integrator LimitValue, pi.anti_windup, and component initialization", () => {
  const catalog = [
    {
      type: "integrator",
      category: "control",
      summary: "Integrator with optional limits",
      sign_convention: "",
      parameters: [
        { name: "name", unit: "", required: true, description: "Signal name" },
        { name: "input", unit: "", required: true, description: "Input signal" },
        { name: "gain", unit: "", required: false, description: "Integrator gain" },
        { name: "initial_value", unit: "", required: false, description: "Initial state" },
        { name: "minimum", unit: "", required: false, description: "Lower limit" },
        { name: "maximum", unit: "", required: false, description: "Upper limit" },
      ],
      example: { type: "integrator", name: "int1", input: "u", gain: 1, initial_value: 0 },
    },
    {
      type: "pi",
      category: "control",
      summary: "PI controller",
      sign_convention: "",
      parameters: [
        { name: "name", unit: "", required: true, description: "Signal name" },
        { name: "input", unit: "", required: true, description: "Input signal" },
        { name: "kp", unit: "", required: true, description: "Proportional gain" },
        { name: "ki", unit: "", required: true, description: "Integral gain" },
        { name: "anti_windup", unit: "", required: false, description: "Anti-windup mode" },
      ],
      example: { type: "pi", name: "pi1", input: "err", kp: 1, ki: 10 },
    },
    {
      type: "nonlinear_inductor",
      category: "circuit",
      summary: "Piecewise-linear saturable inductor",
      sign_convention: "",
      parameters: [
        { name: "name", unit: "", required: true, description: "Component name" },
        { name: "nodes", unit: "", required: true, description: "Terminal nodes" },
        { name: "initialization", unit: "", required: false, description: "Steady-state initialization participation" },
      ],
      example: { type: "nonlinear_inductor", name: "NL1", nodes: ["n1", "gnd"] },
    },
  ];

  const schema = {
    $defs: {
      LimitValue: {
        anyOf: [{ type: "number" }, { type: "string" }],
      },
      AntiWindup: {
        oneOf: [
          { type: "string", const: "conditional", description: "Clamp integrator when saturated" },
          { type: "string", const: "state", description: "Back-calculate integrator state" },
        ],
      },
      ComponentInitialization: {
        type: "string",
        enum: ["include", "exclude"],
      },
      Component: {
        oneOf: [
          {
            type: "object",
            required: ["type", "name", "nodes"],
            properties: {
              type: { const: "nonlinear_inductor" },
              name: { type: "string" },
              nodes: { type: "array", items: { type: "string" }, minItems: 2, maxItems: 2 },
              initialization: {
                $ref: "#/$defs/ComponentInitialization",
                default: "include",
                description: "Steady-state initialization participation",
              },
            },
          },
        ],
      },
      Control: {
        oneOf: [
          {
            type: "object",
            required: ["type", "name", "input"],
            properties: {
              type: { const: "integrator" },
              name: { type: "string" },
              input: { type: "string" },
              gain: { type: "number", default: 1 },
              initial_value: { type: "number", default: 0 },
              minimum: {
                description: "Lower limit (number or signal)",
                anyOf: [{ $ref: "#/$defs/LimitValue" }, { type: "null" }],
              },
              maximum: {
                description: "Upper limit (number or signal)",
                anyOf: [{ $ref: "#/$defs/LimitValue" }, { type: "null" }],
              },
            },
          },
          {
            type: "object",
            required: ["type", "name", "input", "kp", "ki"],
            properties: {
              type: { const: "pi" },
              name: { type: "string" },
              input: { type: "string" },
              kp: { type: "number" },
              ki: { type: "number" },
              anti_windup: {
                $ref: "#/$defs/AntiWindup",
                default: "conditional",
                description: "Anti-windup mode",
              },
            },
          },
        ],
      },
    },
  };

  const index = buildCatalogIndex(catalog, schema);

  const integrator = index.get("integrator");
  for (const limitName of ["minimum", "maximum"]) {
    const limitField = integrator.fields.find((f) => f.name === limitName);
    assert.ok(limitField, `missing ${limitName} on integrator`);
    assert.equal(limitField.kind, "number_or_signal");
    assert.equal(limitField.nullable, true);
    assert.equal(limitField.required, false);
  }

  const pi = index.get("pi");
  const antiWindup = pi.fields.find((f) => f.name === "anti_windup");
  assert.ok(antiWindup);
  assert.equal(antiWindup.kind, "enum");
  assert.deepEqual(antiWindup.enumValues, ["conditional", "state"]);
  assert.equal(antiWindup.default, "conditional");
  assert.equal(antiWindup.required, false);

  const nlInductor = index.get("nonlinear_inductor");
  const initialization = nlInductor.fields.find((f) => f.name === "initialization");
  assert.ok(initialization);
  assert.equal(initialization.kind, "enum");
  assert.deepEqual(initialization.enumValues, ["include", "exclude"]);
  assert.equal(initialization.default, "include");
  assert.equal(initialization.required, false);

  // Round-trip editing with the new 0.7 fields
  const baseModel = JSON.stringify({
    format: "openkeito-model",
    version: "0.7",
    simulation: { time_step_s: 1e-6, end_time_s: 0.01 },
    components: [],
    controls: [],
  });
  let res = addItem(baseModel, "components", "nonlinear_inductor", index);
  assert.equal(res.ok, true);
  res = setItemField(res.text, "components", 0, "initialization", "exclude");
  assert.equal(res.ok, true);
  assert.equal(res.model.components[0].initialization, "exclude");

  res = addItem(res.text, "controls", "integrator", index);
  assert.equal(res.ok, true);
  res = setItemField(res.text, "controls", 0, "minimum", "v_min");
  assert.equal(res.ok, true);
  assert.equal(res.model.controls[0].minimum, "v_min");

  res = addItem(res.text, "controls", "pi", index);
  assert.equal(res.ok, true);
  res = setItemField(res.text, "controls", 1, "anti_windup", "state");
  assert.equal(res.ok, true);
  assert.equal(res.model.controls[1].anti_windup, "state");
});
