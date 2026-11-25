// The parts of the page that need no browser: reading the CSV of a result,
// finding the place of a diagnostic in the text, decimating curves, and the
// numbers of the axes.
//
//   node --test web/tests/modules.test.mjs

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import { parseNumericCsv } from "../src/csv.js";
import { decimate, toLog10 } from "../src/decimate.js";
import { formatValue, logTicks, niceTicks, siScale, tickLabels } from "../src/format.js";
import { itemRanges, locate } from "../src/locate.js";

// The diagram (it needs the built module for the topology of the examples).
import "./diagram.test.mjs";
// The results of a run on the diagram (it runs the examples with the module).
import "./overlay.test.mjs";
// M8 stage 1 modules.
import "./layout_sidecar.test.mjs";
import "./inspector.test.mjs";
import "./batch.test.mjs";
import "./a11y.test.mjs";
// M8 stage 2 modules.
import "./diagram_edit.test.mjs";
import "./diagram_power.test.mjs";
import "./batch_view.test.mjs";

test("a numeric CSV becomes one array per column", () => {
  const t = parseNumericCsv("time_s,V(n1),I(L1)\n0e0,1.5e0,-2e-3\n1e-6,,3\n");
  assert.deepEqual(t.header, ["time_s", "V(n1)", "I(L1)"]);
  assert.equal(t.rows, 2);
  assert.deepEqual([...t.columns[0]], [0, 1e-6]);
  assert.deepEqual([...t.columns[1]], [1.5, NaN]);
  assert.deepEqual([...t.columns[2]], [-0.002, 3]);
});

test("a CSV without rows, without a last newline, and with short rows", () => {
  assert.equal(parseNumericCsv("time_s,V(a)\n").rows, 0);
  assert.equal(parseNumericCsv("time_s,V(a)").rows, 0);
  const t = parseNumericCsv("a,b,c\r\n1,2,3\n4\n5,6,7");
  assert.equal(t.rows, 3);
  assert.deepEqual([...t.columns[1]], [2, NaN, 6]);
  assert.deepEqual([...t.columns[2]], [3, NaN, 7]);
});

const rlc = readFileSync(new URL("../../examples/rlc.json", import.meta.url), "utf8");

test("a path of a diagnostic is found in the text of the model", () => {
  const at = (path) => {
    const r = locate(rlc, path);
    return r && rlc.slice(r.start, r.end);
  };
  assert.equal(at("components[1].resistance_ohm"), "10.0");
  assert.equal(at("outputs[0].node"), '"n3"');
  assert.equal(at("simulation.time_step_s"), "1e-6");
  assert.match(at("components[3]"), /^\{"type": "capacitor".*\}$/);
  // A field that is missing: the object that should have it.
  assert.match(at("components[0].frequency_hz"), /^\{"type": "voltage_source"/);
  assert.match(at("controls"), /^\{\s*"format"/);
  assert.equal(locate(rlc, ""), null);
  assert.equal(locate("{ not JSON", "components[1]"), null);
});

test("the components of a model are found in its text", () => {
  const ranges = itemRanges(rlc, "components");
  assert.equal(ranges.length, 4);
  assert.match(rlc.slice(ranges[2].start, ranges[2].end), /^\{"type": "inductor".*\}$/);
  assert.deepEqual(ranges[1], locate(rlc, "components[1]"));
  assert.deepEqual(itemRanges(rlc, "controls"), []);
  assert.deepEqual(itemRanges("{ not JSON", "components"), []);
  assert.deepEqual(itemRanges('{"components": [{"a": 1}, {"b": ', "components"), []);
});

test("CSV handles empty text and whitespace-only cells", () => {
  assert.deepEqual(parseNumericCsv(""), { header: [], columns: [], rows: 0 });
  const t = parseNumericCsv("a,b\n1, \n2,\r\n");
  assert.equal(t.rows, 2);
  assert.deepEqual([...t.columns[0]], [1, 2]);
  assert.ok(Number.isNaN(t.columns[1][0]));
  assert.ok(Number.isNaN(t.columns[1][1]));
});

test("locate handles escaped quotes, nested arrays, and non-ASCII columns", () => {
  const model = '{\n  "note": "a\\"b}c]",\n  "components": [{"nodes": [["n1", "n2"]]}]\n}';
  const nested = locate(model, "components[0].nodes[0][1]");
  assert.equal(model.slice(nested.start, nested.end), '"n2"');
  const note = locate(model, "note");
  assert.equal(model.slice(note.start, note.end), '"a\\"b}c]"');

  const jp = '{\n  "名前": 1,\n  "値" 2\n}';
  // Line 3 is '  "値" 2': bytes 2 ('  '), 1 ('"'), 3 ('値'), 1 ('"'), 1 (' '), 1 ('2') -> col 9 is '2'.
  const col = locate(jp, "line 3 column 9");
  assert.equal(jp.slice(col.start, col.end), '  "値" 2');
  const mid = locate(jp, "line 3 column 8");
  assert.equal(jp.slice(mid.start, mid.end), '  "値" ');
});

test("decimate preserves peaks, NaN gaps, and log10 scale", () => {
  // Small path (n <= 2 * width)
  const txSmall = new Float64Array([0, 1, NaN, 3]);
  const ySmall = new Float64Array([10, 0, 5, 100]);
  const small = decimate(txSmall, ySmall, 0, 3, 0, 10, 10, true);
  assert.equal(small.every, true);
  assert.equal(small.count, 4);
  assert.equal(small.ys[0], 1);
  assert.ok(Number.isNaN(small.ys[1])); // log10(0) -> NaN
  assert.ok(Number.isNaN(small.ys[2])); // NaN x breaks line
  assert.equal(small.ys[3], 2);

  // Bucket path (n > 2 * width): 10 points into width = 2
  const txBig = new Float64Array(10);
  const yBig = new Float64Array([1, 10, -5, 2, 0, 100, 50, NaN, 20, 5]);
  for (let i = 0; i < 10; i += 1) txBig[i] = i;
  const big = decimate(txBig, yBig, 0, 9, 0, 0.2, 2, false);
  assert.equal(big.every, false);
  assert.equal(big.min, -5);
  assert.equal(big.max, 100);
  // First bucket (indices 0..4 -> pixel 0): first=1, min=-5 (at 2), max=10 (at 1), last=0
  assert.deepEqual([...big.ys.slice(0, 4)], [1, 10, -5, 0]);
  // Second bucket (indices 5..9 -> pixel 1): first=100, max=100 (at 5), min=5 (at 9), last=5
  assert.deepEqual([...big.ys.slice(4, 8)], [100, 100, 5, 5]);

  assert.ok(Number.isNaN(toLog10(0)));
  assert.ok(Number.isNaN(toLog10(-1)));
  assert.ok(Number.isNaN(toLog10(Infinity)));
  assert.equal(toLog10(100), 2);
});

test("line and column of a parse error select a part of that line", () => {
  const text = "{\n  \"a\": 1,\n  \"b\" 2\n}";
  const r = locate(text, "line 3 column 7");
  assert.equal(text.slice(r.start, r.end), '  "b" 2');
  assert.equal(locate(text, "line 9 column 1"), null);
  const whole = locate(text, "line 2 column 1");
  assert.equal(text.slice(whole.start, whole.end), '  "a": 1,');
});

test("ticks are at 1, 2 or 5 times a power of ten", () => {
  assert.deepEqual(niceTicks(0, 0.3, 6).ticks.map((t) => Number(t.toFixed(6))), [0, 0.05, 0.1, 0.15, 0.2, 0.25, 0.3]);
  const { ticks, step } = niceTicks(-99871, 83374, 4);
  assert.equal(step, 50000);
  assert.deepEqual(tickLabels(ticks, step, siScale(99871)), ["-50", "0", "50"]);
  assert.deepEqual(logTicks(1, 3.4, 5), [1, 2, 3]);
  assert.equal(logTicks(2, 2.3, 5), null);
  assert.deepEqual(niceTicks(100000, 100000 + 1e-11, 4), { ticks: [100000], step: 1 });
});

test("values are shown with six digits", () => {
  assert.equal(formatValue(0), "0");
  assert.equal(formatValue(1e-6), "0.000001");
  assert.equal(formatValue(-38526.123456), "−38526.1");
  assert.equal(formatValue(1.73191234e-13), "1.73191e-13");
  assert.equal(formatValue(NaN), "–");
  assert.equal(formatValue(null), "–");
  assert.equal(formatValue(undefined), "–");
  assert.equal(formatValue(Infinity), "∞");
  assert.equal(formatValue(-Infinity), "−∞");
});
