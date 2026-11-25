// Browser-free tests for BatchView: summary strip, lazy loading, status/query
// filtering, threshold .breach highlighting, multi-case waveform comparison,
// and opening individual cases (no wasm build required).
//
//   node --test web/tests/batch_view.test.mjs

import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { pathToFileURL } from "node:url";

import { BatchView } from "../src/batch_view.js";
import { circuitFace, place } from "../src/layout.js";
import { toSvg } from "../src/render.js";

function camelCase(name) {
  return name.replace(/-([a-z])/g, (_, ch) => ch.toUpperCase());
}

function matchesSimple(el, part) {
  let rest = part.trim();
  if (!rest) return false;

  const tagMatch = rest.match(/^[a-zA-Z0-9_-]+/);
  if (tagMatch) {
    if (el.tagName !== tagMatch[0].toUpperCase()) return false;
    rest = rest.slice(tagMatch[0].length);
  }

  while (rest.length > 0) {
    if (rest.startsWith(".")) {
      const m = rest.match(/^\.([a-zA-Z0-9_-]+)/);
      if (!m || !el.classList.contains(m[1])) return false;
      rest = rest.slice(m[0].length);
    } else if (rest.startsWith("[")) {
      const end = rest.indexOf("]");
      if (end < 0) return false;
      const inner = rest.slice(1, end);
      const eq = inner.indexOf("=");
      if (eq < 0) {
        if (!el.hasAttribute(inner)) return false;
      } else {
        const attr = inner.slice(0, eq);
        const val = inner.slice(eq + 1).replace(/^"|"$/g, "");
        if (el.getAttribute(attr) !== val) return false;
      }
      rest = rest.slice(end + 1);
    } else {
      return false;
    }
  }
  return true;
}

function matchesOne(el, selector) {
  const parts = selector.trim().split(/\s+/);
  if (!matchesSimple(el, parts[parts.length - 1])) return false;
  let cur = el.parentNode;
  for (let i = parts.length - 2; i >= 0; i -= 1) {
    while (cur && !matchesSimple(cur, parts[i])) {
      cur = cur.parentNode;
    }
    if (!cur) return false;
    cur = cur.parentNode;
  }
  return true;
}

function matchesSelector(el, selector) {
  return selector
    .split(",")
    .map((part) => part.trim())
    .some((part) => matchesOne(el, part));
}

function createMockDom() {
  class ClassList {
    constructor(el) {
      this.el = el;
    }
    _list() {
      const raw = this.el.getAttribute("class") ?? "";
      return raw.split(/\s+/).filter(Boolean);
    }
    _write(list) {
      this.el.setAttribute("class", list.join(" "));
    }
    add(...names) {
      const set = new Set(this._list());
      for (const name of names) if (name) set.add(name);
      this._write([...set]);
    }
    remove(...names) {
      const drop = new Set(names);
      this._write(this._list().filter((c) => !drop.has(c)));
    }
    contains(name) {
      return this._list().includes(name);
    }
  }

  class MockElement {
    constructor(tag) {
      this.tagName = tag.toUpperCase();
      this.attributes = new Map();
      this.dataset = {};
      this.style = {};
      this.children = [];
      this.parentNode = null;
      this.listeners = new Map();
      this.classList = new ClassList(this);
      this._textContent = "";
      this.hidden = false;
      this.disabled = false;
      this.value = "";
      this.checked = false;
      this.files = null;
      this.clientWidth = 800;
      this.clientHeight = 300;
    }

    get className() {
      return this.getAttribute("class") ?? "";
    }

    set className(value) {
      this.setAttribute("class", String(value));
    }

    get textContent() {
      if (this.children.length === 0) return this._textContent;
      return this._textContent + this.children.map((c) => c.textContent).join("");
    }

    set textContent(value) {
      this.children = [];
      this._textContent = String(value ?? "");
    }

    setAttribute(name, value) {
      const s = String(value);
      this.attributes.set(name, s);
      if (name.startsWith("data-")) this.dataset[camelCase(name.slice(5))] = s;
      if (name === "value") this.value = s;
    }

    getAttribute(name) {
      return this.attributes.get(name) ?? null;
    }

    hasAttribute(name) {
      return this.attributes.has(name);
    }

    append(...nodes) {
      for (const node of nodes) {
        if (typeof node === "string") {
          this._textContent += node;
        } else if (node) {
          node.parentNode = this;
          this.children.push(node);
        }
      }
    }

    replaceChildren(...nodes) {
      this.children = [];
      this._textContent = "";
      this.append(...nodes);
    }

    querySelectorAll(selector) {
      const out = [];
      const walk = (node) => {
        for (const child of node.children) {
          if (matchesSelector(child, selector)) out.push(child);
          walk(child);
        }
      };
      walk(this);
      return out;
    }

    querySelector(selector) {
      return this.querySelectorAll(selector)[0] ?? null;
    }

    addEventListener(type, fn) {
      const list = this.listeners.get(type) ?? [];
      list.push(fn);
      this.listeners.set(type, list);
    }

    dispatchEvent(event) {
      const ev = {
        target: this,
        defaultPrevented: false,
        preventDefault() {
          this.defaultPrevented = true;
        },
        ...event,
      };
      let cur = ev.target;
      while (cur) {
        for (const fn of cur.listeners.get(ev.type) ?? []) fn(ev);
        if (ev.bubbles === false) break;
        cur = cur.parentNode;
      }
      return ev;
    }

    getContext() {
      return {
        setTransform() {},
        clearRect() {},
        beginPath() {},
        moveTo() {},
        lineTo() {},
        stroke() {},
        fill() {},
        rect() {},
        clip() {},
        save() {},
        restore() {},
        arc() {},
        fillText() {},
        fillRect() {},
        setLineDash() {},
        measureText: (t) => ({ width: t.length * 6 }),
      };
    }
  }

  return {
    document: {
      documentElement: new MockElement("html"),
      createElement: (tag) => new MockElement(tag),
    },
  };
}

function withMockBrowser(fn) {
  const dom = createMockDom();
  const prevDocument = globalThis.document;
  const prevRaf = globalThis.requestAnimationFrame;
  const prevGetComputedStyle = globalThis.getComputedStyle;
  globalThis.document = dom.document;
  globalThis.requestAnimationFrame = () => 1;
  globalThis.getComputedStyle = () => ({ getPropertyValue: () => "#000000" });
  return Promise.resolve()
    .then(() => fn(dom))
    .finally(() => {
      globalThis.document = prevDocument;
      globalThis.requestAnimationFrame = prevRaf;
      globalThis.getComputedStyle = prevGetComputedStyle;
    });
}

function makeFixtureEntries() {
  const reads = new Map();
  const track = (path, content) => ({
    path,
    name: path.split("/").pop(),
    readText: async () => {
      reads.set(path, (reads.get(path) ?? 0) + 1);
      return content;
    },
  });

  const batchSummary = JSON.stringify({
    tool: "openkeito",
    version: "0.9.0",
    command: "batch",
    batch: {
      path: "batches/sweep.json",
      name: "fault-sweep",
      sha256: "deadbeef",
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
        errors: [{ code: "E200", path: "solver", message: "Singular matrix", hint: "Add shunt" }],
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
    elapsed_s: 1.25,
  });

  const batchMetrics = [
    "case,status,column,unit,min,max,max_abs,last",
    "c1,completed,V(b1.a),V,-180.5,182.0,182.0,12.5",
    "c1,completed,I(L1),A,-12.0,15.5,15.5,-3.2",
    "c2,failed,,,,,,",
    "c3,completed,V(b1.a),V,-210.0,205.0,210.0,-4.0",
    "c3,completed,I(L1),A,-8.0,9.5,9.5,1.1",
    "c4,invalid,,,,,,",
    "",
  ].join("\r\n");

  const caseSummary = (cols) =>
    JSON.stringify({
      tool: "openkeito",
      version: "0.9.0",
      status: "completed",
      result: { time_column: "time_s", columns: cols },
    });

  const c1Model = JSON.stringify({ format: "openkeito-model", version: "0.6", name: "c1" });

  const entries = [
    track("sweep/batch-summary.json", batchSummary),
    track("sweep/batch-metrics.csv", batchMetrics),
    track("sweep/c1/summary.json", caseSummary([{ name: "V(b1.a)", unit: "V" }, { name: "I(L1)", unit: "A" }])),
    track("sweep/c1/result.csv", "time_s,V(b1.a),I(L1)\n0,0,0\n0.001,182,15.5\n0.002,12.5,-3.2\n"),
    track("sweep/c1/events.csv", "time_s,component,action,interrupted_current_a\n0.001,CB1,open,15.5\n"),
    track("sweep/c1/c1.json", c1Model),
    track("sweep/c3/summary.json", caseSummary([{ name: "V(b1.a)", unit: "V" }, { name: "I(L1)", unit: "A" }])),
    track("sweep/c3/result.csv", "time_s,V(b1.a),I(L1)\n0,5,1\n0.001,-210,9.5\n0.002,-4,1.1\n"),
  ];

  return { entries, reads, c1Model };
}

test("BatchView renders summary counts and rows without reading per-case result.csv", async () => {
  await withMockBrowser(async (dom) => {
    const { entries, reads } = makeFixtureEntries();
    const root = dom.document.createElement("div");
    const view = new BatchView(root);

    // Folder and fallback multiple file inputs exist with expected attributes.
    const folderInput = root.querySelector('[data-role="folder-input"]');
    const filesInput = root.querySelector('[data-role="files-input"]');
    assert.ok(folderInput);
    assert.ok(filesInput);
    assert.equal(folderInput.hasAttribute("webkitdirectory"), true);
    assert.equal(folderInput.hasAttribute("multiple"), true);
    assert.equal(filesInput.hasAttribute("multiple"), true);

    await view.loadEntries(entries);

    // Summary strip displays batch name, completed/failed/invalid counts, and elapsed time.
    assert.equal(root.querySelector('[data-stat="name"]').textContent, "fault-sweep");
    assert.match(root.querySelector('[data-stat="completed"]').textContent, /2/);
    assert.match(root.querySelector('[data-stat="failed"]').textContent, /1/);
    assert.match(root.querySelector('[data-stat="invalid"]').textContent, /1/);
    assert.match(root.querySelector('[data-stat="elapsed"]').textContent, /1\.25 s/);

    // All 4 cases appear in the table, sorted by default (V(b1.a) max_abs desc -> c3, c1, c2, c4).
    const rows = root.querySelectorAll("tbody tr");
    assert.equal(rows.length, 4);
    assert.deepEqual(
      rows.map((r) => r.dataset.case),
      ["c3", "c1", "c2", "c4"],
    );

    // Lazy loading: NO per-case files were read on initial folder load.
    assert.equal(reads.get("sweep/batch-summary.json"), 1);
    assert.equal(reads.get("sweep/batch-metrics.csv"), 1);
    assert.equal(reads.get("sweep/c1/result.csv") ?? 0, 0);
    assert.equal(reads.get("sweep/c3/result.csv") ?? 0, 0);
    assert.equal(reads.get("sweep/c1/summary.json") ?? 0, 0);
    assert.equal(reads.get("sweep/c1/events.csv") ?? 0, 0);
    assert.equal(reads.get("sweep/c1/c1.json") ?? 0, 0);
  });
});

test("BatchView applies status filter, search query, sort order, and .breach threshold highlighting", async () => {
  await withMockBrowser(async (dom) => {
    const { entries } = makeFixtureEntries();
    const root = dom.document.createElement("div");
    const view = new BatchView(root);
    await view.loadEntries(entries);

    const statusSelect = root.querySelector('[data-filter="status"]');
    const queryInput = root.querySelector('[data-filter="query"]');
    const colSelect = root.querySelector('[data-filter="column"]');
    const metricSelect = root.querySelector('[data-filter="metricField"]');
    const dirBtn = root.querySelector('[data-filter="direction"]');
    const minInput = root.querySelector('[data-filter="thresholdMin"]');
    const maxInput = root.querySelector('[data-filter="thresholdMax"]');

    // Threshold max=200 on V(b1.a) flags c3 (205 > 200) with .breach and badge.
    maxInput.value = "200";
    maxInput.dispatchEvent({ type: "input" });
    let rows = root.querySelectorAll("tbody tr");
    assert.equal(rows[0].dataset.case, "c3");
    assert.equal(rows[0].classList.contains("breach"), true);
    assert.ok(rows[0].querySelector(".breach-badge"));
    assert.equal(rows[1].dataset.case, "c1");
    assert.equal(rows[1].classList.contains("breach"), false);
    assert.equal(rows[1].querySelector(".breach-badge"), null);

    // Lower threshold min=-190 also flags c3 (-210 < -190), not c1 (-180.5).
    maxInput.value = "";
    maxInput.dispatchEvent({ type: "input" });
    minInput.value = "-190";
    minInput.dispatchEvent({ type: "input" });
    rows = root.querySelectorAll("tbody tr");
    assert.equal(rows[0].classList.contains("breach"), true);
    assert.equal(rows[1].classList.contains("breach"), false);

    // Clear threshold and toggle sort direction on I(L1) last ascending: c1 (-3.2) before c3 (1.1).
    minInput.value = "";
    minInput.dispatchEvent({ type: "input" });
    colSelect.value = "I(L1)";
    colSelect.dispatchEvent({ type: "change" });
    metricSelect.value = "last";
    metricSelect.dispatchEvent({ type: "change" });
    dirBtn.dispatchEvent({ type: "click" });
    rows = root.querySelectorAll("tbody tr");
    assert.deepEqual(
      rows.map((r) => r.dataset.case),
      ["c1", "c3", "c2", "c4"],
    );

    // Status filter: failed -> only c2.
    statusSelect.value = "failed";
    statusSelect.dispatchEvent({ type: "change" });
    rows = root.querySelectorAll("tbody tr");
    assert.deepEqual(rows.map((r) => r.dataset.case), ["c2"]);

    // Status back to all + search query matching E101 -> only c4.
    statusSelect.value = "all";
    statusSelect.dispatchEvent({ type: "change" });
    queryInput.value = "e101";
    queryInput.dispatchEvent({ type: "input" });
    rows = root.querySelectorAll("tbody tr");
    assert.deepEqual(rows.map((r) => r.dataset.case), ["c4"]);
  });
});

test("BatchView lazily loads checked cases for multi-case comparison chart and clears on uncheck", async () => {
  await withMockBrowser(async (dom) => {
    const { entries, reads } = makeFixtureEntries();
    const root = dom.document.createElement("div");
    const view = new BatchView(root);
    await view.loadEntries(entries);

    const chartEl = root.querySelector(".batch-compare-chart");
    const emptyEl = root.querySelector(".batch-compare-empty");
    assert.equal(chartEl.hidden, true);
    assert.equal(emptyEl.hidden, false);

    // Check c1 and c3 checkboxes.
    const checkC1 = root.querySelector('.batch-compare-check[data-case="c1"]');
    const checkC3 = root.querySelector('.batch-compare-check[data-case="c3"]');
    const checkC2 = root.querySelector('.batch-compare-check[data-case="c2"]');
    assert.equal(checkC2.disabled, true);

    checkC1.checked = true;
    checkC1.dispatchEvent({ type: "change" });
    await new Promise((r) => setTimeout(r, 0));

    assert.equal(reads.get("sweep/c1/result.csv"), 1);
    assert.equal(reads.get("sweep/c3/result.csv") ?? 0, 0);

    checkC3.checked = true;
    checkC3.dispatchEvent({ type: "change" });
    await new Promise((r) => setTimeout(r, 0));

    assert.equal(reads.get("sweep/c1/result.csv"), 1);
    assert.equal(reads.get("sweep/c3/result.csv"), 1);
    assert.equal(chartEl.hidden, false);
    assert.equal(emptyEl.hidden, true);

    assert.equal(view.chart.panels.length, 1);
    assert.equal(view.chart.panels[0].unit, "V");
    assert.deepEqual(
      view.chart.panels[0].series.map((s) => s.name),
      ["c1", "c3"],
    );
    assert.deepEqual([...view.chart.x], [0, 0.001, 0.002]);
    assert.deepEqual([...view.chart.panels[0].series[0].data], [0, 182, 12.5]);
    assert.deepEqual([...view.chart.panels[0].series[1].data], [5, -210, -4]);

    // Switching metric column updates chart series from cached cases without re-reading CSV.
    const colSelect = root.querySelector('[data-filter="column"]');
    colSelect.value = "I(L1)";
    colSelect.dispatchEvent({ type: "change" });
    await new Promise((r) => setTimeout(r, 0));
    assert.equal(reads.get("sweep/c1/result.csv"), 1);
    assert.equal(reads.get("sweep/c3/result.csv"), 1);
    assert.equal(view.chart.panels[0].unit, "A");
    assert.deepEqual([...view.chart.panels[0].series[0].data], [0, 15.5, -3.2]);
    assert.deepEqual([...view.chart.panels[0].series[1].data], [1, 9.5, 1.1]);

    // Unchecking all cases restores guidance message and hides chart.
    checkC1.checked = false;
    checkC1.dispatchEvent({ type: "change" });
    checkC3.checked = false;
    checkC3.dispatchEvent({ type: "change" });
    await new Promise((r) => setTimeout(r, 0));
    assert.equal(chartEl.hidden, true);
    assert.equal(emptyEl.hidden, false);
  });
});

test("BatchView Open button calls onOpenCase with loaded summary, table, events, and optional modelText", async () => {
  await withMockBrowser(async (dom) => {
    const { entries, c1Model } = makeFixtureEntries();
    const opened = [];
    const root = dom.document.createElement("div");
    const view = new BatchView(root, {
      onOpenCase: (payload) => opened.push(payload),
    });
    await view.loadEntries(entries);

    // c1 has summary.json, result.csv, events.csv, and c1.json.
    const openC1 = root.querySelector('.batch-open-btn[data-case="c1"]');
    assert.equal(openC1.textContent, "Open");
    openC1.dispatchEvent({ type: "click" });
    await new Promise((r) => setTimeout(r, 0));

    assert.equal(opened.length, 1);
    assert.equal(opened[0].caseName, "c1");
    assert.equal(opened[0].caseInfo.name, "c1");
    assert.equal(opened[0].summary.status, "completed");
    assert.equal(opened[0].table.rows, 3);
    assert.deepEqual(opened[0].table.header, ["time_s", "V(b1.a)", "I(L1)"]);
    assert.equal(opened[0].events.length, 1);
    assert.equal(opened[0].events[0].component, "CB1");
    assert.equal(opened[0].modelText, c1Model);

    // c3 has summary.json and result.csv, but no model JSON -> modelText is null.
    const openC3 = root.querySelector('.batch-open-btn[data-case="c3"]');
    openC3.dispatchEvent({ type: "click" });
    await new Promise((r) => setTimeout(r, 0));

    assert.equal(opened.length, 2);
    assert.equal(opened[1].caseName, "c3");
    assert.equal(opened[1].summary.status, "completed");
    assert.equal(opened[1].table.rows, 3);
    assert.deepEqual(opened[1].events, []);
    assert.equal(opened[1].modelText, null);
  });
});

test("BatchView loadFiles handles browser FileList-like input and aligns unequal-length comparison series", async () => {
  await withMockBrowser(async (dom) => {
    const batchSummary = JSON.stringify({
      tool: "openkeito",
      version: "0.9.0",
      command: "batch",
      batch: { name: "unequal-grid" },
      status: "failed",
      counts: { completed: 1, failed: 1, invalid: 0 },
      cases: [
        { name: "full", status: "completed", model: "full.json", warnings: 0, errors: [] },
        { name: "early_stop", status: "failed", model: "early.json", warnings: 0, errors: ["stopped"] },
      ],
      elapsed_s: 0.2,
    });
    const batchMetrics = [
      "case,status,column,unit,min,max,max_abs,last",
      "full,completed,V(n1),V,0,10,10,10",
      "early_stop,failed,V(n1),V,0,5,5,5",
      "",
    ].join("\n");

    const fileList = [
      { name: "batch-summary.json", webkitRelativePath: "out/batch-summary.json", text: async () => batchSummary },
      { name: "batch-metrics.csv", webkitRelativePath: "out/batch-metrics.csv", text: async () => batchMetrics },
      { name: "result.csv", webkitRelativePath: "out/full/result.csv", text: async () => "time_s,V(n1)\n0,0\n0.002,10\n" },
      { name: "result.csv", webkitRelativePath: "out/early_stop/result.csv", text: async () => "time_s,V(n1)\n0,0\n0.001,5\n" },
    ];

    const root = dom.document.createElement("div");
    const view = new BatchView(root);
    const folderInput = root.querySelector('[data-role="folder-input"]');
    folderInput.files = fileList;
    folderInput.dispatchEvent({ type: "change" });
    await new Promise((r) => setTimeout(r, 0));

    assert.ok(view.getModel());
    assert.equal(view.getModel().cases.length, 2);

    const cFull = root.querySelector('.batch-compare-check[data-case="full"]');
    const cEarly = root.querySelector('.batch-compare-check[data-case="early_stop"]');
    cFull.checked = true;
    cFull.dispatchEvent({ type: "change" });
    cEarly.checked = true;
    cEarly.dispatchEvent({ type: "change" });
    await new Promise((r) => setTimeout(r, 0));

    // Merged time grid has [0, 0.001, 0.002]; full is interpolated at 0.001 (5) and early_stop is NaN at 0.002.
    assert.deepEqual([...view.chart.x], [0, 0.001, 0.002]);
    assert.deepEqual([...view.chart.panels[0].series[0].data], [0, 5, 10]);
    assert.equal(view.chart.panels[0].series[1].data[0], 0);
    assert.equal(view.chart.panels[0].series[1].data[1], 5);
    assert.ok(Number.isNaN(view.chart.panels[0].series[1].data[2]));
  });
});

test("BatchView Open handles malformed summary errors without unhandled promise rejection", async () => {
  await withMockBrowser(async (dom) => {
    const batchSummary = JSON.stringify({
      tool: "openkeito",
      version: "0.10.0",
      command: "batch",
      batch: { name: "bad-case-summary" },
      status: "completed",
      counts: { completed: 1, failed: 0, invalid: 0 },
      cases: [{ name: "c1", status: "completed", model: "c1.json", warnings: 0, errors: [] }],
      elapsed_s: 0.01,
    });
    const entries = [
      { path: "b/batch-summary.json", name: "batch-summary.json", readText: async () => batchSummary },
      { path: "b/c1/summary.json", name: "summary.json", readText: async () => "{not valid json" },
    ];

    const unhandled = [];
    const onUnhandled = (reason) => unhandled.push(reason);
    process.on("unhandledRejection", onUnhandled);
    try {
      const root = dom.document.createElement("div");
      const view = new BatchView(root);
      await view.loadEntries(entries);
      const openBtn = root.querySelector('.batch-open-btn[data-case="c1"]');
      openBtn.dispatchEvent({ type: "click" });
      await new Promise((r) => setTimeout(r, 10));
      assert.equal(unhandled.length, 0);
      assert.match(root.querySelector(".batch-status").textContent, /Could not open case "c1"/);
    } finally {
      process.off("unhandledRejection", onUnhandled);
    }
  });
});

test("BatchView Open loads Diagram from real openkeito batch output without extra manual model files", async (t) => {
  const repoRoot = join(import.meta.dirname, "..", "..");
  const exeName = process.platform === "win32" ? "openkeito.exe" : "openkeito";
  const exePath = join(repoRoot, "target", "release", exeName);
  const wasmDir = join(repoRoot, "crates", "openkeito-wasm", "pkg");
  if (!existsSync(exePath) || !existsSync(join(wasmDir, "openkeito_wasm_bg.wasm"))) {
    t.skip("requires release openkeito CLI and wasm package");
    return;
  }

  const wasm = await import(pathToFileURL(join(wasmDir, "openkeito_wasm.js")).href);
  wasm.initSync({ module: readFileSync(join(wasmDir, "openkeito_wasm_bg.wasm")) });

  const workDir = mkdtempSync(join(tmpdir(), "openkeito-batch-web-"));
  try {
    const batchFile = join(workDir, "batch.json");
    const outDir = join(workDir, "out");
    writeFileSync(
      batchFile,
      JSON.stringify({
        format: "openkeito-batch",
        version: "0.1",
        name: "real-batch-check",
        cases: [
          { name: "rlc", model: join(repoRoot, "examples", "rlc.json") },
          { name: "breaker", model: JSON.parse(readFileSync(join(repoRoot, "examples", "rl_breaker.json"), "utf8")) },
        ],
      }),
    );

    execFileSync(exePath, ["batch", batchFile, "--out", outDir], { encoding: "utf8" });

    // Collect ONLY files written into outDir by `openkeito batch`.
    const diskEntries = [];
    const walk = (dir, relBase) => {
      for (const d of readdirSync(dir, { withFileTypes: true })) {
        const full = join(dir, d.name);
        const rel = `${relBase}/${d.name}`;
        if (d.isDirectory()) walk(full, rel);
        else {
          const text = readFileSync(full, "utf8");
          diskEntries.push({ path: rel, name: d.name, readText: async () => text });
        }
      }
    };
    walk(outDir, "out");

    await withMockBrowser(async (dom) => {
      const opened = [];
      const batchRoot = dom.document.createElement("div");
      const view = new BatchView(batchRoot, { onOpenCase: (payload) => opened.push(payload) });
      await view.loadEntries(diskEntries);

      for (const name of ["rlc", "breaker"]) {
        const btn = batchRoot.querySelector(`.batch-open-btn[data-case="${name}"]`);
        assert.ok(btn, `missing Open button for ${name}`);
        btn.dispatchEvent({ type: "click" });
        await new Promise((r) => setTimeout(r, 0));
      }

      assert.equal(opened.length, 2);
      for (const payload of opened) {
        assert.equal(typeof payload.modelText, "string");
        const topology = JSON.parse(wasm.topology(payload.modelText));
        const placed = place(circuitFace(topology));
        const svg = toSvg(placed, "Circuit diagram");
        assert.ok(placed.items.length >= 2, `${payload.caseName}: diagram items missing`);
        assert.ok(svg.includes("<svg") && svg.includes('data-item="component:'), `${payload.caseName}: SVG missing`);
      }
    });
  } finally {
    rmSync(workDir, { recursive: true, force: true });
  }
});
