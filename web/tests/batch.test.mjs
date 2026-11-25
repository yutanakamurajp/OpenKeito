// Unit tests for the batch folder loader, aggregator, filter/sort, and
// multi-case waveform comparison module (no wasm build required).
//
//   node --test web/tests/batch.test.mjs

import assert from "node:assert/strict";
import { test } from "node:test";

import {
  buildBatchModel,
  entriesFromFileList,
  extractComparisonSeries,
  filterAndSortCases,
  indexBatchFolder,
  parseBatchMetrics,
  parseBatchSummary,
  parseEventsCsv,
} from "../src/batch.js";

function makeSampleSummary() {
  return JSON.stringify({
    tool: "openkeito",
    version: "0.9.0",
    command: "batch",
    batch: {
      path: "batches/scan.json",
      name: "fault-sweep",
      sha256: "abc123",
    },
    status: "failed",
    jobs: 4,
    counts: { completed: 2, failed: 1, invalid: 1 },
    cases: [
      {
        name: "c1",
        status: "completed",
        model: "batches/c1.json",
        out_dir: "out/c1",
        elapsed_s: 0.12,
        warnings: 1,
        errors: [],
      },
      {
        name: "c2",
        status: "failed",
        model: "batches/c2.json",
        out_dir: "out/c2",
        elapsed_s: 0.05,
        warnings: 0,
        errors: [{ code: "E200", path: "solver", message: "Singular matrix at t=0.05", hint: "Add shunt resistance" }],
      },
      {
        name: "c3",
        status: "completed",
        model: "batches/c3.json",
        out_dir: "out/c3",
        elapsed_s: 0.15,
        warnings: 0,
        errors: [],
      },
      {
        name: "c4",
        status: "invalid",
        model: "batches/c4.json",
        out_dir: null,
        elapsed_s: null,
        warnings: 0,
        errors: [{ code: "E101", path: "components[0].resistance_ohm", message: "must be positive", hint: "Set > 0" }],
      },
    ],
    metrics_csv_path: "out/batch-metrics.csv",
    elapsed_s: 0.34,
  });
}

function makeSampleMetricsCrlf() {
  return [
    "case,status,column,unit,min,max,max_abs,last",
    "c1,completed,V(b1.a),V,-180.5,182.0,182.0,12.5",
    "c1,completed,I(L1),A,-12.0,15.5,15.5,-3.2",
    "c2,failed,,,,,,",
    "c3,completed,V(b1.a),V,-210.0,205.0,210.0,-4.0",
    "c3,completed,I(L1),A,-8.0,9.5,9.5,1.1",
    "c4,invalid,,,,,,",
    "",
  ].join("\r\n");
}

function makeCaseSummary(name, status, columns) {
  return JSON.stringify({
    tool: "openkeito",
    version: "0.9.0",
    status,
    result: {
      time_column: "time_s",
      columns,
    },
  });
}

test("parseBatchSummary parses mixed completed/failed/invalid summary and rejects invalid inputs", () => {
  const summary = parseBatchSummary(makeSampleSummary());
  assert.equal(summary.command, "batch");
  assert.equal(summary.status, "failed");
  assert.equal(summary.batch.name, "fault-sweep");
  assert.deepEqual(summary.counts, { completed: 2, failed: 1, invalid: 1 });
  assert.equal(summary.cases.length, 4);
  assert.equal(summary.cases[1].errors[0].code, "E200");
  assert.equal(summary.cases[3].out_dir, null);

  assert.throws(() => parseBatchSummary(""), /non-empty/);
  assert.throws(() => parseBatchSummary("{not json"), /Invalid JSON/);
  assert.throws(() => parseBatchSummary(JSON.stringify({ command: "run", status: "completed", cases: [] })), /command "batch"/);
  assert.throws(() => parseBatchSummary(JSON.stringify({ command: "batch", status: "unknown", cases: [] })), /invalid status/);
  assert.throws(() => parseBatchSummary(JSON.stringify({ command: "batch", status: "completed" })), /cases array/);
});

test("parseBatchMetrics handles CRLF, empty failed/invalid rows, and numeric stats", () => {
  const parsed = parseBatchMetrics(makeSampleMetricsCrlf());
  assert.equal(parsed.rows.length, 6);
  assert.deepEqual(parsed.columns, [
    { name: "V(b1.a)", unit: "V" },
    { name: "I(L1)", unit: "A" },
  ]);

  const c1 = parsed.byCase.get("c1");
  assert.equal(c1.status, "completed");
  assert.deepEqual(c1.metrics.get("V(b1.a)"), {
    column: "V(b1.a)",
    unit: "V",
    min: -180.5,
    max: 182,
    max_abs: 182,
    last: 12.5,
  });

  const c2 = parsed.byCase.get("c2");
  assert.equal(c2.status, "failed");
  assert.equal(c2.metrics.size, 0);
  assert.equal(c2.rows.length, 0);

  const c4 = parsed.byCase.get("c4");
  assert.equal(c4.status, "invalid");
  assert.equal(c4.metrics.size, 0);

  const emptyRow = parsed.rows[2];
  assert.equal(emptyRow.case, "c2");
  assert.equal(emptyRow.column, "");
  assert.equal(emptyRow.min, null);
  assert.equal(emptyRow.max_abs, null);

  assert.deepEqual(parseBatchMetrics(""), { rows: [], byCase: new Map(), columns: [] });
});

test("entriesFromFileList and indexBatchFolder support nested folder prefix and cases/ layout", async () => {
  // Mock browser FileList-like objects with webkitRelativePath and text().
  const mockFiles = [
    { name: "batch-summary.json", webkitRelativePath: "out-001/batch-summary.json", text: async () => makeSampleSummary() },
    { name: "batch-metrics.csv", webkitRelativePath: "out-001/batch-metrics.csv", text: async () => makeSampleMetricsCrlf() },
    { name: "result.csv", webkitRelativePath: "out-001/c1/result.csv", text: async () => "time_s,V(b1.a)\n0,1\n" },
    { name: "summary.json", webkitRelativePath: "out-001/c1/summary.json", text: async () => "{}" },
    { name: "events.csv", webkitRelativePath: "out-001/cases/c3/events.csv", text: async () => "time_s,component,action,interrupted_current_a\n0.01,CB1,open,12.5\n" },
    { name: "phasors.csv", webkitRelativePath: "out-001/cases/c3/phasors.csv", text: async () => "" },
    { name: "c3.json", webkitRelativePath: "out-001/cases/c3/c3.json", text: async () => "{}" },
  ];

  const entries = entriesFromFileList(mockFiles);
  const index = indexBatchFolder(entries);

  assert.equal(index.root, "out-001");
  assert.ok(index.summary);
  assert.ok(index.metrics);
  assert.equal(index.cases.get("c1").result.name, "result.csv");
  assert.equal(index.cases.get("c1").summary.name, "summary.json");
  assert.equal(index.cases.get("c3").events.name, "events.csv");
  assert.equal(index.cases.get("c3").phasors.name, "phasors.csv");
  assert.equal(index.cases.get("c3").model.name, "c3.json");

  // Also test flat root without enclosing folder name.
  const flatIndex = indexBatchFolder([
    { path: "batch-summary.json", name: "batch-summary.json", readText: async () => makeSampleSummary() },
    { path: "c1/result.csv", name: "result.csv", readText: async () => "" },
  ]);
  assert.equal(flatIndex.root, "");
  assert.ok(flatIndex.cases.get("c1").result);
});

test("buildBatchModel defers reading case files until loadCase and caches subsequent calls", async () => {
  const counts = new Map();
  const track = (path, content) => ({
    path,
    name: path.split("/").pop(),
    readText: async () => {
      counts.set(path, (counts.get(path) ?? 0) + 1);
      return content;
    },
  });

  const c1Summary = makeCaseSummary("c1", "completed", [
    { name: "V(b1.a)", unit: "V" },
    { name: "I(L1)", unit: "A" },
  ]);
  const c1Result = "time_s,V(b1.a),I(L1)\r\n0,0,0\r\n0.001,180,10\r\n0.002,100,-3.2\r\n";
  const c1Events = "time_s,component,action,interrupted_current_a\r\n0.001,CB1,close,\r\n0.002,CB1,open,3.2\r\n";

  const c3Summary = makeCaseSummary("c3", "completed", [
    { name: "V(b1.a)", unit: "V" },
    { name: "I(L1)", unit: "A" },
  ]);
  const c3Result = "time_s,V(b1.a),I(L1)\n0,10,1\n0.001,-210,9.5\n0.002,-4,1.1\n";

  const entries = [
    track("out-001/batch-summary.json", makeSampleSummary()),
    track("out-001/batch-metrics.csv", makeSampleMetricsCrlf()),
    track("out-001/c1/summary.json", c1Summary),
    track("out-001/c1/result.csv", c1Result),
    track("out-001/c1/events.csv", c1Events),
    track("out-001/c3/summary.json", c3Summary),
    track("out-001/c3/result.csv", c3Result),
  ];

  const batchModel = await buildBatchModel(entries);

  // Initial load reads ONLY batch-summary.json and batch-metrics.csv.
  assert.equal(counts.get("out-001/batch-summary.json"), 1);
  assert.equal(counts.get("out-001/batch-metrics.csv"), 1);
  assert.equal(counts.get("out-001/c1/result.csv") ?? 0, 0);
  assert.equal(counts.get("out-001/c1/summary.json") ?? 0, 0);
  assert.equal(counts.get("out-001/c1/events.csv") ?? 0, 0);
  assert.equal(counts.get("out-001/c3/result.csv") ?? 0, 0);
  assert.equal(batchModel.isCaseLoaded("c1"), false);
  assert.equal(batchModel.cases.length, 4);

  // First loadCase("c1") reads c1 files exactly once.
  const loaded1 = await batchModel.loadCase("c1");
  assert.equal(counts.get("out-001/c1/result.csv"), 1);
  assert.equal(counts.get("out-001/c1/summary.json"), 1);
  assert.equal(counts.get("out-001/c1/events.csv"), 1);
  assert.equal(counts.get("out-001/c3/result.csv") ?? 0, 0);
  assert.equal(batchModel.isCaseLoaded("c1"), true);
  assert.equal(loaded1.result.rows, 3);
  assert.deepEqual([...loaded1.result.columns[1]], [0, 180, 100]);
  assert.equal(loaded1.events.length, 2);
  assert.equal(loaded1.events[0].interrupted_current_a, null);
  assert.equal(loaded1.events[1].interrupted_current_a, 3.2);

  // Second loadCase("c1") returns cached object without re-reading.
  const loaded1Again = await batchModel.loadCase("c1");
  assert.equal(loaded1Again, loaded1);
  assert.equal(counts.get("out-001/c1/result.csv"), 1);

  await assert.rejects(() => batchModel.loadCase("missing"), /Unknown batch case/);
});

test("filterAndSortCases filters by status/query, sorts with missing metrics last, and flags thresholds", async () => {
  const entries = [
    { path: "batch-summary.json", name: "batch-summary.json", readText: async () => makeSampleSummary() },
    { path: "batch-metrics.csv", name: "batch-metrics.csv", readText: async () => makeSampleMetricsCrlf() },
  ];
  const model = await buildBatchModel(entries);

  // Status filter + query filter (case-insensitive on error code/message).
  const failedOnly = filterAndSortCases(model, { status: "failed" });
  assert.deepEqual(failedOnly.map((c) => c.name), ["c2"]);

  const singularMatch = filterAndSortCases(model, { query: "singular" });
  assert.deepEqual(singularMatch.map((c) => c.name), ["c2"]);

  const codeMatch = filterAndSortCases(model, { query: "e101" });
  assert.deepEqual(codeMatch.map((c) => c.name), ["c4"]);

  // Sort descending by V(b1.a) max_abs: c3 (210) -> c1 (182) -> missing metrics last (c2, c4).
  const descSorted = filterAndSortCases(model, {
    column: "V(b1.a)",
    metricField: "max_abs",
    direction: "desc",
    threshold: { column: "V(b1.a)", field: "max_abs", maxAbs: 200 },
  });
  assert.deepEqual(descSorted.map((c) => c.name), ["c3", "c1", "c2", "c4"]);
  assert.deepEqual(descSorted.map((c) => c.exceeded), [true, false, false, false]);
  assert.equal(descSorted[0].violations[0].limit, 200);

  // Sort ascending by I(L1) last: c1 (-3.2) -> c3 (1.1) -> missing metrics still last (c2, c4).
  const ascSorted = filterAndSortCases(model, {
    column: "I(L1)",
    metricField: "last",
    direction: "asc",
    threshold: { column: "I(L1)", min: -2, max: 12 },
  });
  assert.deepEqual(ascSorted.map((c) => c.name), ["c1", "c3", "c2", "c4"]);
  // c1 has min=-12 (< -2) and max=15.5 (> 12), c3 has min=-8 (< -2).
  assert.equal(ascSorted[0].exceeded, true);
  assert.equal(ascSorted[0].violations.length, 2);
  assert.equal(ascSorted[1].exceeded, true);
  assert.equal(ascSorted[1].violations.length, 1);
});

test("extractComparisonSeries extracts aligned time/values/unit from loaded cases", async () => {
  const entries = [
    { path: "batch-summary.json", name: "batch-summary.json", readText: async () => makeSampleSummary() },
    { path: "batch-metrics.csv", name: "batch-metrics.csv", readText: async () => makeSampleMetricsCrlf() },
    {
      path: "c1/summary.json",
      name: "summary.json",
      readText: async () => makeCaseSummary("c1", "completed", [{ name: "V(b1.a)", unit: "V" }]),
    },
    {
      path: "c1/result.csv",
      name: "result.csv",
      readText: async () => "time_s,V(b1.a)\n0,0\n0.01,182\n",
    },
    {
      path: "c2/summary.json",
      name: "summary.json",
      readText: async () => makeCaseSummary("c2", "failed", []),
    },
    {
      path: "c3/summary.json",
      name: "summary.json",
      readText: async () => makeCaseSummary("c3", "completed", [{ name: "V(b1.a)", unit: "V" }]),
    },
    {
      path: "c3/result.csv",
      name: "result.csv",
      readText: async () => "time_s,V(b1.a)\n0,5\n0.01,-210\n",
    },
  ];

  const model = await buildBatchModel(entries);
  const loaded = await Promise.all([model.loadCase("c1"), model.loadCase("c2"), model.loadCase("c3")]);
  const series = extractComparisonSeries(loaded, "V(b1.a)");

  assert.equal(series.length, 2);
  assert.equal(series[0].caseName, "c1");
  assert.equal(series[0].status, "completed");
  assert.equal(series[0].unit, "V");
  assert.deepEqual([...series[0].time], [0, 0.01]);
  assert.deepEqual([...series[0].values], [0, 182]);

  assert.equal(series[1].caseName, "c3");
  assert.deepEqual([...series[1].values], [5, -210]);
});

test("parseEventsCsv parses CRLF and empty interrupted current", () => {
  const events = parseEventsCsv("time_s,component,action,interrupted_current_a\r\n0.005,CB1,close,\r\n0.02,CB1,open,-4.5\r\n");
  assert.deepEqual(events, [
    { time_s: 0.005, component: "CB1", action: "close", interrupted_current_a: null },
    { time_s: 0.02, component: "CB1", action: "open", interrupted_current_a: -4.5 },
  ]);
});

test("buildBatchModel resolves cases[].model fallback (file path and inline batch#case) when case/model.json is absent", async () => {
  const summary = JSON.stringify({
    tool: "openkeito",
    version: "0.10.0",
    command: "batch",
    batch: { path: "sweep.json", name: "fallback-check" },
    status: "completed",
    counts: { completed: 2, failed: 0, invalid: 0 },
    cases: [
      { name: "from_path", status: "completed", model: "models/c_path.json", warnings: 0, errors: [] },
      { name: "from_inline", status: "completed", model: "sweep.json#from_inline", warnings: 0, errors: [] },
    ],
    elapsed_s: 0.1,
  });
  const sweepBatch = JSON.stringify({
    format: "openkeito-batch",
    version: "0.1",
    cases: [
      { name: "from_path", model: "models/c_path.json" },
      { name: "from_inline", model: { format: "openkeito-model", version: "0.6", name: "inline-ok" } },
    ],
  });
  const pathModel = JSON.stringify({ format: "openkeito-model", version: "0.6", name: "path-ok" });

  const batchModel = await buildBatchModel([
    { path: "out/batch-summary.json", name: "batch-summary.json", readText: async () => summary },
    { path: "out/from_path/summary.json", name: "summary.json", readText: async () => "{}" },
    { path: "out/from_inline/summary.json", name: "summary.json", readText: async () => "{}" },
    { path: "models/c_path.json", name: "c_path.json", readText: async () => pathModel },
    { path: "sweep.json", name: "sweep.json", readText: async () => sweepBatch },
  ]);

  assert.equal(batchModel.cases[0].hasModel, true);
  assert.equal(batchModel.cases[1].hasModel, true);
  assert.equal(await batchModel.cases[0].files.model.readText(), pathModel);
  const inlineObj = JSON.parse(await batchModel.cases[1].files.model.readText());
  assert.equal(inlineObj.name, "inline-ok");
});

test("buildBatchModel does not attach another case's model.json to an invalid case", async () => {
  const summary = JSON.stringify({
    tool: "openkeito",
    version: "0.10.0",
    command: "batch",
    batch: { path: "batch.json", name: "good-and-bad" },
    status: "failed",
    counts: { completed: 1, failed: 0, invalid: 1 },
    cases: [
      { name: "good", status: "completed", model: "d1/model.json", warnings: 0, errors: [] },
      {
        name: "bad",
        status: "invalid",
        model: "d2/model.json",
        warnings: 0,
        errors: [{ code: "E130", path: "components[1].resistance_ohm", message: "must be positive", hint: "Set > 0" }],
      },
    ],
    elapsed_s: 0.1,
  });
  const goodModel = JSON.stringify({ format: "openkeito-model", version: "0.6", name: "good-r10" });
  const badModel = JSON.stringify({ format: "openkeito-model", version: "0.6", name: "bad-r-10" });

  // Only batch output directory files: `bad` must NOT inherit `out/good/model.json`.
  const outputOnly = await buildBatchModel([
    { path: "out/batch-summary.json", name: "batch-summary.json", readText: async () => summary },
    { path: "out/good/summary.json", name: "summary.json", readText: async () => "{}" },
    { path: "out/good/result.csv", name: "result.csv", readText: async () => "time_s\n0\n" },
    { path: "out/good/model.json", name: "model.json", readText: async () => goodModel },
  ]);
  assert.equal(outputOnly.cases[0].hasModel, true);
  assert.equal(outputOnly.cases[1].hasModel, false);
  assert.equal(outputOnly.cases[1].files.model, null);

  // When the external `d2/model.json` is also selected, `bad` resolves strictly to its own model.
  const withExternal = await buildBatchModel([
    { path: "out/batch-summary.json", name: "batch-summary.json", readText: async () => summary },
    { path: "out/good/summary.json", name: "summary.json", readText: async () => "{}" },
    { path: "out/good/model.json", name: "model.json", readText: async () => goodModel },
    { path: "d2/model.json", name: "model.json", readText: async () => badModel },
  ]);
  assert.equal(withExternal.cases[1].hasModel, true);
  assert.equal(await withExternal.cases[1].files.model.readText(), badModel);
});
