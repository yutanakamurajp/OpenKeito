// The page: an editor for the model, the solver in a worker, and the results.

import { config } from "../config.js";
import { BatchView } from "./batch_view.js";
import { Chart } from "./chart.js";
import { Diagram } from "./diagram.js";
import { fill, h } from "./dom.js";
import { formatBytes, formatDuration, formatValue } from "./format.js";
import { InspectorPanel } from "./inspector_panel.js";
import { parseLayoutSidecar } from "./layout_sidecar.js";
import { itemRanges, locate } from "./locate.js";
import { rowAtOrBefore } from "./overlay.js";

const $ = (id) => document.getElementById(id);

const editor = $("editor");
const gutter = $("gutter");
const runButton = $("run");
const scanButton = $("scan");
const stopButton = $("stop");
const resetButton = $("reset");
const statusLine = $("status");
const exampleSelect = $("example");

/// Rows of the events table; events.csv has all of them.
const EVENT_ROWS = 300;

// ---- the solver ----

class Stopped extends Error {}
class Unavailable extends Error {}

/// The worker that holds the solver. Calls are answered in the order they
/// are made; `restart` ends a running call by ending the worker.
class Engine {
  constructor(onState) {
    this.onState = onState;
    this.pending = new Map();
    this.nextId = 1;
    this.failed = null;
    this.start();
  }

  start() {
    const url = new URL("./worker.js", import.meta.url);
    if (new URLSearchParams(location.search).get("clock") === "off") url.searchParams.set("clock", "off");
    this.worker = new Worker(url, { type: "module" });
    this.worker.onmessage = ({ data }) => {
      if (data.type === "ready") return this.onState({ version: data.version });
      if (data.type === "failed") return this.fail(data.message);
      const call = this.pending.get(data.id);
      if (!call) return undefined;
      this.pending.delete(data.id);
      if (data.error !== undefined) {
        call.reject(new Error(data.error));
        this.restart(); // after a trap the module cannot be used again
      } else {
        call.resolve(data);
      }
      return undefined;
    };
    this.worker.onerror = (event) => {
      event.preventDefault();
      this.fail(event.message || "the worker could not be started");
    };
  }

  fail(message) {
    if (this.failed) return;
    this.failed = message || "the worker could not be started";
    for (const call of this.pending.values()) call.reject(new Unavailable(this.failed));
    this.pending.clear();
    this.onState({ failed: this.failed });
  }

  call(kind, text, label) {
    if (this.failed) return Promise.reject(new Unavailable(this.failed));
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.worker.postMessage({ id, kind, text, label });
    });
  }

  restart() {
    if (this.failed) return;
    this.worker.terminate();
    for (const call of this.pending.values()) call.reject(new Stopped("stopped"));
    this.pending.clear();
    this.start();
  }
}

const state = {
  manifest: null,
  example: null, // the entry of the manifest the text came from
  original: "", // its text, to undo edits
  busy: null, // { kind, started, timer } while a run or scan is under way
  runGen: 0,
  validateTimer: 0,
  validateSeq: 0,
  validatePending: false,
  resultText: null, // the text the shown results were computed from
  result: null, // the run for the diagram: { text, table, columns, events, model }
  topologyText: null, // the text the diagram was drawn from
  tab: "diagram",
  split: true, // the waveforms also below the diagram
  visible: new Map(), // column name -> shown, kept over runs of one example
  marks: new Map(), // line number -> "error" | "warning"
  lineCount: 0,
  urls: { run: [], scan: [], events: [] }, // object URLs of the downloads, to revoke
  ready: false,
  metadataLoaded: false,
  modelMode: "json",
  ownSelection: -Infinity, // when the diagram last selected text in the editor
  ranges: { text: null, lists: [] }, // where the components and blocks are in the text
};

async function loadMetadata() {
  if (state.metadataLoaded || engine.failed) return;
  try {
    const { catalog, schema } = await engine.call("metadata", "", "");
    state.metadataLoaded = true;
    inspector.setCatalog(catalog, schema);
  } catch {
    // Retried on next ready event if the worker restarted.
  }
}

const engine = new Engine((s) => {
  if (s.failed) {
    const box = $("load-error");
    box.textContent =
      `The solver could not be loaded: ${s.failed}. This page needs a browser with WebAssembly ` +
      "and module workers, and must be served over HTTP (not opened as a file).";
    box.hidden = false;
    setStatus("The solver is not available.", "error");
    return;
  }
  $("version").textContent = s.version;
  if (!state.ready) {
    state.ready = true;
    updateButtons();
    if (!state.busy) setStatus("Ready.");
  }
  loadMetadata();
});

function sidecarStorageKey() {
  return state.example ? `openkeito-layout:${state.example.file}` : null;
}

function saveSidecarForCurrent(sidecar) {
  const key = sidecarStorageKey();
  if (!key) return;
  try {
    if (sidecar) localStorage.setItem(key, JSON.stringify(sidecar));
    else localStorage.removeItem(key);
  } catch {
    // Ignore storage unavailability in private browsing.
  }
}

function loadSidecarForExample(file) {
  try {
    const raw = localStorage.getItem(`openkeito-layout:${file}`);
    if (!raw) return null;
    const parsed = parseLayoutSidecar(raw);
    return parsed.valid ? parsed.layout : null;
  } catch {
    return null;
  }
}

const waveforms = new Chart($("waveforms-chart"));
const diagram = new Diagram($("diagram"), {
  onSelect: (path) => {
    inspector.setSelection(path);
    const range = locate(editor.value, path);
    if (!range) return;
    // The selection this makes is the diagram's own: it does not move the
    // diagram in turn.
    state.ownSelection = performance.now();
    showInEditor(range);
  },
  // The cursor of the waveforms and the row of the diagram are one.
  onRow: (row) => waveforms.moveCursor(row),
  // A press on a component shows its columns, as their check boxes do.
  onColumns: (names) => waveforms.show(names),
  onSidecarChange: (sidecar) => saveSidecarForCurrent(sidecar),
});

const inspector = new InspectorPanel($("inspector-panel"), {
  onChange: (nextText) => setText(nextText, { fromInspector: true, recordHistory: false }),
  onSelectPath: (path) => diagram.showPath(path),
});

const batchView = new BatchView($("batch-view"), {
  onOpenCase: (caseData) => void openBatchCase(caseData),
});

function selectModelMode(mode) {
  state.modelMode = mode === "form" ? "form" : "json";
  const isForm = state.modelMode === "form";
  $("mode-json").setAttribute("aria-pressed", String(!isForm));
  $("mode-form").setAttribute("aria-pressed", String(isForm));
  $("editor-box").hidden = isForm;
  $("editor-hint").hidden = isForm;
  $("inspector-panel").hidden = !isForm;
  if (isForm) inspector.setModelText(editor.value, { recordHistory: false });
}
$("mode-json").addEventListener("click", () => selectModelMode("json"));
$("mode-form").addEventListener("click", () => selectModelMode("form"));

/// The run that the diagram may show on the model of `text`: only the run
/// of that very text.
function resultFor(text) {
  return state.result !== null && state.result.text === text ? state.result : null;
}
const scanChart = new Chart($("scan-chart"));

// ---- status and buttons ----

function setStatus(text, kind = "") {
  statusLine.textContent = text;
  statusLine.className = `status ${kind}`;
}

function modelHasScan(text) {
  try {
    const model = JSON.parse(text);
    return typeof model === "object" && model !== null && model.frequency_scan !== undefined;
  } catch {
    return /"frequency_scan"\s*:/.test(text);
  }
}

function updateButtons() {
  const idle = state.ready && !state.busy;
  runButton.disabled = !idle;
  const hasScan = modelHasScan(editor.value);
  scanButton.disabled = !idle || !hasScan;
  scanButton.title = hasScan ? "" : "This model has no frequency_scan section";
  stopButton.disabled = !state.busy;
  resetButton.disabled = editor.value === state.original;
  $("stale").hidden = state.resultText === null || state.resultText === editor.value;
}

function setBusy(kind) {
  const started = performance.now();
  const verb = kind === "scan" ? "Scanning" : "Running";
  const tick = () => {
    const seconds = (performance.now() - started) / 1000;
    setStatus(`${verb}… ${seconds.toFixed(1)} s`, "busy");
  };
  state.busy = { kind, started, timer: setInterval(tick, 100) };
  setStatus(`${verb}…`, "busy");
  updateButtons();
}

function clearBusy() {
  if (state.busy) clearInterval(state.busy.timer);
  state.busy = null;
  updateButtons();
}

function label() {
  return state.example && editor.value === state.original ? state.example.file : "edited model";
}

// ---- the editor ----

function renderGutter() {
  let lines = 1;
  const text = editor.value;
  for (let i = text.indexOf("\n"); i >= 0; i = text.indexOf("\n", i + 1)) lines += 1;
  if (lines !== state.lineCount) {
    state.lineCount = lines;
    const numbers = [];
    for (let n = 1; n <= lines; n += 1) numbers.push(h("div", { text: String(n) }));
    gutter.replaceChildren(...numbers);
  }
  for (const node of gutter.querySelectorAll(".error, .warning")) node.className = "";
  for (const [line, severity] of state.marks) {
    const node = gutter.children[line - 1];
    if (node) node.className = severity;
  }
  gutter.scrollTop = editor.scrollTop;
}

function lineOf(text, offset) {
  let line = 1;
  for (let i = text.indexOf("\n"); i >= 0 && i < offset; i = text.indexOf("\n", i + 1)) line += 1;
  return line;
}

/// Selects what a diagnostic points at, and scrolls it into view.
function showInEditor(range) {
  // Without scrolling the page: on a narrow screen the editor is above the
  // diagram, and the press was in the diagram.
  editor.focus({ preventScroll: true });
  editor.setSelectionRange(range.start, range.end);
  const lineHeight = parseFloat(getComputedStyle(editor).lineHeight) || 18;
  const line = lineOf(editor.value, range.start);
  const top = (line - 1) * lineHeight;
  if (top < editor.scrollTop || top > editor.scrollTop + editor.clientHeight - 2 * lineHeight) {
    editor.scrollTop = Math.max(0, top - editor.clientHeight / 3);
  }
}

editor.addEventListener("scroll", () => {
  gutter.scrollTop = editor.scrollTop;
});
editor.addEventListener("input", () => {
  renderGutter();
  updateButtons();
  clearTimeout(state.validateTimer);
  state.validateTimer = setTimeout(validateNow, 300);
});
editor.addEventListener("keydown", (event) => {
  if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) {
    event.preventDefault();
    execute("run");
  }
});

/// The component or block the cursor of the editor is in, as its path
/// ("components[3]"), or null.
function pathAtCursor() {
  const text = editor.value;
  if (state.ranges.text !== text) {
    state.ranges = { text, lists: ["components", "controls"].map((key) => [key, itemRanges(text, key)]) };
  }
  const at = editor.selectionStart;
  for (const [key, ranges] of state.ranges.lists) {
    const index = ranges.findIndex((r) => r.start <= at && at <= r.end);
    if (index >= 0) return `${key}[${index}]`;
  }
  return null;
}

document.addEventListener("selectionchange", () => {
  if (document.activeElement !== editor) return;
  if (performance.now() - state.ownSelection < 200) return;
  const path = pathAtCursor();
  diagram.showPath(path);
  if (path) inspector.setSelection(path);
});

// ---- diagnostics ----

function diagnosticItem(d, range) {
  const severity = d.severity === "warning" ? "warning" : "error";
  const where =
    d.path &&
    (range
      ? h(
          "button",
          {
            type: "button",
            class: "diagnostic-path",
            title: "Show in the model",
            onclick: () => {
              // The text may have changed since: find the place again.
              const now = locate(editor.value, d.path);
              if (now) showInEditor(now);
            },
          },
          d.path,
        )
      : h("span", { class: "diagnostic-path", text: d.path }));
  return h(
    "li",
    { class: `diagnostic ${severity}` },
    h(
      "div",
      { class: "diagnostic-head" },
      h("span", { class: "badge", text: `${severity === "error" ? "Error" : "Warning"} ${d.code ?? ""}`.trim() }),
      where,
    ),
    h("p", { class: "diagnostic-message", text: d.message ?? "" }),
    d.hint && h("p", { class: "diagnostic-hint" }, h("strong", { text: "Hint: " }), d.hint),
  );
}

/// Shows errors and warnings below the editor and marks their lines.
function showDiagnostics(errors, warnings, text, headline) {
  const all = [...errors, ...warnings];
  state.marks = new Map();
  const items = all.map((d) => {
    const range = locate(text, d.path);
    if (range) {
      const line = lineOf(text, range.start);
      const severity = d.severity === "warning" ? "warning" : "error";
      if (severity === "error" || !state.marks.has(line)) state.marks.set(line, severity);
    }
    return diagnosticItem(d, range);
  });
  renderGutter();
  const summary = $("diagnostics-summary");
  summary.textContent = headline;
  summary.className = `diagnostics-summary ${errors.length > 0 ? "error" : warnings.length > 0 ? "warning" : "good"}`;
  $("diagnostics-list").replaceChildren(...items);
}

function plural(n, word) {
  return `${n.toLocaleString("en-US")} ${word}${n === 1 ? "" : "s"}`;
}

function invalidHeadline(errors) {
  return `✕ Not valid: ${plural(errors.length, "error")}. Each one says what to change.`;
}

function showReport(report, text) {
  const errors = report.errors ?? [];
  const warnings = report.warnings ?? [];
  inspector.setDiagnostics(errors, warnings);
  let headline;
  if (!report.valid) {
    headline = invalidHeadline(errors);
  } else {
    const what = report.schedule
      ? "Valid schedule file (this page runs models only)"
      : `✓ Valid: ${plural(report.components ?? 0, "component")}, ${plural(report.steps ?? 0, "time step")}`;
    headline = warnings.length > 0 ? `${what}, ${plural(warnings.length, "warning")}.` : `${what}.`;
  }
  showDiagnostics(errors, warnings, text, headline);
}

async function validateNow() {
  clearTimeout(state.validateTimer);
  if (engine.failed) return;
  if (state.busy) {
    state.validatePending = true;
    return;
  }
  state.validatePending = false;
  const seq = ++state.validateSeq;
  const text = editor.value;
  inspector.setModelText(text, { recordHistory: true });
  try {
    const { report } = await engine.call("validate", text, label());
    if (seq !== state.validateSeq || text !== editor.value) return;
    showReport(report, text);
    const { topology } = await engine.call("topology", text, label());
    if (seq !== state.validateSeq || text !== editor.value) return;
    const first = (topology.errors ?? [])[0];
    state.topologyText = text;
    const restored = resultFor(text);
    diagram.setTopology(topology, first ? `No diagram: ${first.message}. See the diagnostics below the model.` : "", restored);
    if (restored && waveforms.cursor >= 0) diagram.setRow(waveforms.cursor);
  } catch (e) {
    if (e instanceof Unavailable) return;
    if (e instanceof Stopped) {
      if (seq === state.validateSeq) {
        if (state.busy) state.validatePending = true;
        else validateNow(); // the worker was restarted: ask the new one
      }
    } else {
      crashed(e);
    }
  }
}

function crashed(e) {
  setStatus(
    `The solver stopped unexpectedly (${e.message}). This is a bug in OpenKeito; the solver has been restarted.`,
    "error",
  );
}

// ---- running ----

/// `auto`: the run that follows the choice of an example; it does not take
/// the reader away from the tab that is open.
async function execute(kind, auto = false) {
  if (engine.failed) return;
  if (kind === "scan" && !modelHasScan(editor.value)) return;
  if (state.busy) {
    state.runGen += 1;
    clearBusy();
    engine.restart();
  }
  const gen = ++state.runGen;
  const text = editor.value;
  const example = state.example;
  setBusy(kind);
  try {
    const output = await engine.call(kind, text, label());
    if (gen !== state.runGen) return;
    clearBusy();
    if (kind === "run") showRun(output, text, example, auto);
    else showScan(output, text, example);
    if (state.validatePending) validateNow();
  } catch (e) {
    if (gen !== state.runGen) return;
    clearBusy();
    if (e instanceof Unavailable) return;
    if (e instanceof Stopped) setStatus("Stopped. The solver was restarted and is ready.");
    else crashed(e);
    if (state.validatePending) validateNow();
  }
}

runButton.addEventListener("click", () => execute("run"));
scanButton.addEventListener("click", () => execute("scan"));
stopButton.addEventListener("click", () => {
  if (state.busy) engine.restart();
});
resetButton.addEventListener("click", () => {
  setText(state.original);
  editor.focus();
});

/// Replaces the object URLs of one kind of download.
function downloads(kind, files, extra = {}) {
  for (const url of state.urls[kind]) URL.revokeObjectURL(url);
  state.urls[kind] = [];
  const links = [];
  for (const [name, blob] of [...Object.entries(extra), ...Object.entries(files)]) {
    const url = URL.createObjectURL(blob);
    state.urls[kind].push(url);
    links.push(h("a", { href: url, download: name, class: "download" }, `${name} (${formatBytes(blob.size)})`));
  }
  return h("p", { class: "downloads" }, h("span", { text: "Download: " }), links);
}

function unitsOf(summary) {
  return new Map((summary.result?.columns ?? []).map((c) => [c.name, c.unit ?? ""]));
}

function eventsOf(summary) {
  return summary.all_events ?? summary.events ?? [];
}

function firstErrorText(summary) {
  const first = (summary.errors ?? [])[0];
  return first ? `: ${first.code} ${first.message}` : "";
}

function buildTable(caption, head, rows) {
  return h(
    "div",
    { class: "table-scroll" },
    h(
      "table",
      {},
      h("caption", { text: caption }),
      h("thead", {}, h("tr", {}, head.map((t) => h("th", { scope: "col", text: t })))),
      h("tbody", {}, rows),
    ),
  );
}

/// A run that did not get to the solver: the diagnostics of the model.
function showInvalid(summary, text) {
  const errors = summary.errors ?? [];
  showDiagnostics(errors, summary.warnings ?? [], text, invalidHeadline(errors));
  setStatus("The model is not valid: see the diagnostics below the editor.", "error");
  $("diagnostics").scrollIntoView({ block: "nearest" });
}

const UNIT_TITLES = { V: "Voltage (V)", A: "Current (A)", "": "Signals" };

function setRunStatus(summary, rows, wallSeconds) {
  const steps = summary.solver?.steps ?? summary.simulation?.steps ?? 0;
  if (summary.status === "completed") {
    const warnings = (summary.warnings ?? []).length;
    setStatus(
      `Completed in ${formatDuration(wallSeconds)}: ${plural(steps, "step")}, ${plural(rows, "row")}` +
        (warnings > 0 ? `, ${plural(warnings, "warning")} (see Summary).` : "."),
      "good",
    );
  } else {
    setStatus(`The simulation failed after ${plural(rows, "row")}${firstErrorText(summary)}. See Summary.`, "error");
  }
}

function syncVisibleColumns(names, units, example) {
  const known = names.every((n) => state.visible.has(n)) && names.length === state.visible.size;
  if (known) return;
  state.visible = new Map();
  const wanted = (example?.columns ?? []).filter((n) => names.includes(n));
  const perUnit = new Map();
  for (const name of names) {
    const unit = units.get(name) ?? "";
    const count = perUnit.get(unit) ?? 0;
    perUnit.set(unit, count + 1);
    state.visible.set(name, wanted.length > 0 ? wanted.includes(name) : count < 3);
  }
}

function buildWaveformPanels(names, table, units) {
  const panels = new Map();
  names.forEach((name, k) => {
    const unit = units.get(name) ?? "";
    if (!panels.has(unit)) {
      panels.set(unit, { title: UNIT_TITLES[unit] ?? `Signals (${unit})`, unit, series: [] });
    }
    panels.get(unit).series.push({ name, data: table.columns[k + 1], visible: state.visible.get(name) });
  });
  const order = (unit) => (unit === "V" ? 0 : unit === "A" ? 1 : 2);
  return [...panels.values()].sort((a, b) => order(a.unit) - order(b.unit));
}

function showRun(output, text, example = state.example, auto = false) {
  const { summary, table, files, wallSeconds } = output;
  if (output.code === 2 || !table) {
    showInvalid(summary, text);
    return;
  }
  state.resultText = text;
  setRunStatus(summary, table.rows, wallSeconds);

  // Waveforms: one panel per unit, in the order voltage, current, signals.
  const units = unitsOf(summary);
  const names = table.header.slice(1);
  syncVisibleColumns(names, units, example);
  const events = eventsOf(summary);
  $("waveforms-empty").hidden = true;
  $("waveforms-chart").hidden = false;
  waveforms.setData({
    x: { data: table.columns[0], label: "time", unit: "s" },
    panels: buildWaveformPanels(names, table, units),
    markers: events.map((e) => e.time_s).filter(Number.isFinite),
    onVisibility: (name, visible) => state.visible.set(name, visible),
    onCursor: (row) => diagram.setRow(row),
  });
  // The same run on the diagram, while the diagram is that of this text.
  let model = null;
  try {
    model = JSON.parse(text);
  } catch {
    // A text that ran can be read; without it a switch starts open.
  }
  state.result = { text, table, columns: summary.result?.columns ?? [], events, model, summary };
  if (state.topologyText === text) diagram.setResult(state.result);
  $("split-box").hidden = false;
  const note = $("waveforms-note");
  note.hidden = false;
  note.textContent =
    `${plural(table.rows, "row")} × ${plural(names.length, "column")}` +
    (events.length > 0 ? `, ${plural(summary.event_count ?? events.length, "event")}` : "") +
    ". Dotted vertical lines mark events when no more than 40 are in view.";

  showSummary(output);
  showEvents(summary, files);
  selectTab(auto ? state.tab : "waveforms");
  updateButtons();
}

function showScan(output, text, example = state.example) {
  const { summary, table, files, wallSeconds } = output;
  if (output.code === 2 || !table) {
    showInvalid(summary, text);
    return;
  }
  if (output.code !== 0) {
    setStatus(`The scan failed${firstErrorText(summary)}.`, "error");
    return;
  }
  const units = unitsOf(summary);
  const magnitude = { title: "Impedance magnitude (ohm)", unit: "Ω", log: true, series: [] };
  const angle = { title: "Impedance angle (degrees)", unit: "°", series: [] };
  const names = table.header.slice(1);
  const wanted = (example?.scanColumns ?? []).filter((n) => names.includes(n));
  names.forEach((name, k) => {
    const panel = units.get(name) === "deg" || /deg\(/.test(name) ? angle : magnitude;
    const visible = wanted.length > 0 ? wanted.includes(name) : panel.series.length < 8;
    panel.series.push({ name, data: table.columns[k + 1], visible });
  });
  const f = table.columns[0];
  const log = f.length > 1 && f.every((v) => v > 0) && f[f.length - 1] / f[0] >= 20;
  $("scan-empty").hidden = true;
  $("scan-chart").hidden = false;
  scanChart.setData({
    x: { data: f, label: "frequency", unit: "Hz", log },
    panels: [magnitude, angle].filter((p) => p.series.length > 0),
  });
  const singular = summary.singular_frequencies_hz ?? [];
  fill(
    $("scan-info"),
    h("p", {
      class: "hint",
      text:
        `${table.rows.toLocaleString("en-US")} frequencies from ${formatValue(f[0])} Hz to ` +
        `${formatValue(f[f.length - 1])} Hz, computed in ${formatDuration(wallSeconds)}. ` +
        "Z(node) is the impedance seen from a node, Z(a;b) a transfer impedance, Z1, Z2 and Z0 the " +
        "positive, negative and zero sequence impedance of a bus." +
        (singular.length > 0 ? ` Singular at ${singular.map(formatValue).join(", ")} Hz.` : ""),
    }),
    diagnosticList(summary.warnings ?? []),
    downloads("scan", files, {
      "scan-summary.json": new Blob([JSON.stringify(summary, null, 2)], { type: "application/json" }),
    }),
  );
  setStatus(`Scan completed in ${formatDuration(wallSeconds)}: ${plural(table.rows, "row")}.`, "good");
  selectTab("scan");
}

function diagnosticList(items) {
  if (items.length === 0) return null;
  return h("ul", { class: "diagnostic-list" }, items.map((d) => diagnosticItem(d, null)));
}

// ---- summary and events ----

function facts(title, pairs) {
  const present = pairs.filter(([, value]) => value !== undefined && value !== null && value !== "");
  if (present.length === 0) return null;
  return h(
    "section",
    { class: "fact-group" },
    h("h4", { text: title }),
    h("dl", {}, present.map(([name, value]) => h("div", {}, h("dt", { text: name }), h("dd", { text: String(value) })))),
  );
}

const count = (n) => (typeof n === "number" ? n.toLocaleString("en-US") : n);
const seconds = (s) => (typeof s === "number" ? formatDuration(s) : undefined);
const quantity = (v, unit) => (typeof v === "number" ? `${formatValue(v)} ${unit}` : undefined);

function showSummary(output) {
  const { summary, files, wallSeconds } = output;
  const sim = summary.simulation ?? {};
  const solver = summary.solver ?? {};
  const timing = summary.timing ?? {};
  const init = summary.initialization;
  const completed = summary.status === "completed";
  const units = unitsOf(summary);
  const statistics = summary.result?.statistics ?? [];
  const perStep = solver.steps > 0 && timing.run_s > 0 ? `${((timing.run_s / solver.steps) * 1e6).toFixed(2)} µs` : undefined;

  $("summary-empty").hidden = true;
  fill(
    $("summary-content"),
    h(
      "p",
      { class: `run-status ${completed ? "good" : "error"}` },
      h("span", { class: "badge", text: completed ? "✓ completed" : `✕ ${summary.status}` }),
      ` ${summary.model?.name ?? ""}`,
    ),
    diagnosticList([...(summary.errors ?? []), ...(summary.warnings ?? [])]),
    h(
      "div",
      { class: "fact-grid" },
      facts("Simulation", [
        ["Time step", quantity(sim.time_step_s, "s")],
        ["End time", quantity(sim.end_time_s, "s")],
        ["Steps", count(sim.steps)],
        ["Method", sim.method],
        ["Switching instants", sim.switching],
        ["Start", sim.initialization],
        ["Rows recorded", count(summary.result?.rows)],
      ]),
      facts("Solver", [
        ["Unknowns", count(solver.unknowns)],
        ["Linear solver", solver.linear_solver],
        ["Parts of the circuit", count(solver.parts)],
        ["Factorizations", count(solver.factorizations)],
        ["Solves", count(solver.solves)],
        ["Operating-point iterations", count(solver.operating_point_iterations)],
        ["Steps split at an instant", count(solver.split_steps)],
        ["Threads", count(solver.threads)],
      ]),
      facts("Computing time", [
        ["In this page, with the result as text", seconds(wallSeconds)],
        ["Solver, total", seconds(timing.run_s)],
        ["Building the circuit", seconds(timing.build_s)],
        ["Factorizing", seconds(timing.factorize_s)],
        ["Solving", seconds(timing.solve_s)],
        ["Writing rows", seconds(timing.output_s)],
        ["Per step", perStep],
      ]),
      init &&
        facts("Start from the steady state", [
          ["Frequency", quantity(init.frequency_hz, "Hz")],
          ["Compared until", quantity(init.checked_until_s, "s")],
          ["Largest voltage deviation", quantity(init.max_voltage_deviation_v, "V")],
          ["relative", typeof init.relative_voltage_deviation === "number" ? init.relative_voltage_deviation.toExponential(2) : undefined],
          ["Largest current deviation", quantity(init.max_current_deviation_a, "A")],
          ["relative", typeof init.relative_current_deviation === "number" ? init.relative_current_deviation.toExponential(2) : undefined],
        ]),
    ),
    statistics.length > 0 &&
      buildTable(
        "Columns of the result",
        ["Column", "Unit", "Minimum", "Maximum", "Last"],
        statistics.map((c) =>
          h(
            "tr",
            {},
            h("th", { scope: "row", text: c.name }),
            h("td", { text: units.get(c.name) ?? "" }),
            [c.min, c.max, c.last].map((v) => h("td", { class: "number", text: formatValue(v) })),
          ),
        ),
      ),
    downloads("run", files, {
      "summary.json": new Blob([JSON.stringify(summary, null, 2)], { type: "application/json" }),
    }),
    h(
      "details",
      {},
      h("summary", { text: "The whole summary (summary.json)" }),
      h("pre", { class: "json", text: JSON.stringify({ ...summary, all_events: undefined }, null, 2) }),
    ),
  );
}

function showEvents(summary, files) {
  const events = eventsOf(summary);
  const total = summary.event_count ?? events.length;
  $("event-count").textContent = total > 0 ? `(${total.toLocaleString("en-US")})` : "";
  $("events-empty").hidden = true;
  const content = $("events-content");
  if (total === 0) {
    downloads("events", {});
    fill(content, h("p", { class: "empty", text: "No switch, breaker, diode or fault acted in this run." }));
    return;
  }
  const counts = Object.entries(summary.event_counts ?? {});
  const shown = events.slice(0, EVENT_ROWS);
  fill(
    content,
    h("p", {
      class: "hint",
      text:
        `${plural(total, "event")}.` +
        (events.length > shown.length ? ` The first ${shown.length} are listed; events.csv has all of them.` : "") +
        " Press a time to see it in the waveforms, a component to see it in the diagram at that instant.",
    }),
    counts.length > 0 &&
      buildTable(
        "Actions per component",
        ["Component", "Actions"],
        counts.map(([component, actions]) =>
          h(
            "tr",
            {},
            h("th", { scope: "row", text: component }),
            h("td", { text: Object.entries(actions).map(([a, n]) => `${a} × ${n.toLocaleString("en-US")}`).join(", ") }),
          ),
        ),
      ),
    buildTable(
      "Events in the order they happened",
      ["Time (s)", "Component", "Action", "Interrupted current (A)"],
      shown.map((e) =>
        h(
          "tr",
          {},
          h(
            "td",
            { class: "number" },
            h(
              "button",
              {
                type: "button",
                class: "link-button",
                title: "Show in the waveforms",
                onclick: () => {
                  selectTab("waveforms");
                  // The row at which the event is about to act.
                  waveforms.focusOn(e.time_s, rowAtOrBefore(state.result.table.columns[0], e.time_s));
                },
              },
              formatValue(e.time_s),
            ),
          ),
          h(
            "td",
            {},
            h(
              "button",
              {
                type: "button",
                class: "link-button",
                title: "Show in the diagram at this instant",
                onclick: () => {
                  selectTab("diagram");
                  diagram.showEvent(e.time_s, e.component);
                },
              },
              e.component,
            ),
          ),
          h("td", { text: e.action }),
          h("td", { class: "number", text: e.interrupted_current_a === undefined ? "" : formatValue(e.interrupted_current_a) }),
        ),
      ),
    ),
    files["events.csv"] && downloads("events", { "events.csv": files["events.csv"] }),
  );
}

// ---- tabs ----

const tabs = [...document.querySelectorAll('[role="tab"]')];

function selectTab(name, focus = false) {
  state.tab = name;
  // With a run, the waveforms are also below the diagram, so that the
  // cursor of one can be watched in the other.
  const split = name === "diagram" && state.split && state.result !== null;
  for (const tab of tabs) {
    const selected = tab.id === `tab-${name}`;
    tab.setAttribute("aria-selected", String(selected));
    tab.tabIndex = selected ? 0 : -1;
    $(tab.getAttribute("aria-controls")).hidden = !selected && !(split && tab.id === "tab-waveforms");
    if (selected && focus) tab.focus();
  }
  $("results").classList.toggle("split", split);
}

try {
  state.split = localStorage.getItem("openkeito-split") !== "off";
} catch {
  // No storage (private window): the choice lasts for this visit.
}
$("split").checked = state.split;
$("split").addEventListener("change", () => {
  state.split = $("split").checked;
  try {
    localStorage.setItem("openkeito-split", state.split ? "on" : "off");
  } catch {
    // See above.
  }
  selectTab(state.tab);
});

tabs.forEach((tab, k) => {
  tab.addEventListener("click", () => selectTab(tab.id.slice(4)));
  tab.addEventListener("keydown", (event) => {
    const to = { ArrowRight: k + 1, ArrowLeft: k - 1, Home: 0, End: tabs.length - 1 }[event.key];
    if (to === undefined) return;
    event.preventDefault();
    selectTab(tabs[(to + tabs.length) % tabs.length].id.slice(4), true);
  });
});

// ---- examples ----

function setText(text, { fromInspector = false, recordHistory = true } = {}) {
  editor.value = text;
  if (!fromInspector) {
    editor.scrollTop = 0;
    editor.setSelectionRange(0, 0);
  }
  state.marks = new Map();
  renderGutter();
  updateButtons();
  if (!fromInspector) inspector.setModelText(text, { recordHistory });
  return validateNow();
}

async function openBatchCase({ caseName, summary, table, events, modelText }) {
  if (modelText) {
    state.example = null;
    state.original = modelText;
    state.visible = new Map();
    diagram.setSidecar(null, { tell: false });
    await setText(modelText, { recordHistory: false });
  } else {
    state.topologyText = null;
    state.visible = new Map();
    diagram.setTopology(null, `Batch case "${caseName}": no model JSON file in the loaded folder.`);
  }
  if (!table || !summary) {
    setStatus(`Opened batch case "${caseName}" (no result table available).`, summary?.status === "completed" ? "" : "error");
    if (summary) {
      showSummary({ summary, files: {}, wallSeconds: summary.elapsed_s ?? 0 });
      showEvents({ ...summary, all_events: events ?? summary.all_events ?? summary.events ?? [] }, {});
      selectTab("summary");
    }
    return;
  }
  const mergedSummary = {
    ...summary,
    all_events: events ?? summary.all_events ?? summary.events ?? [],
  };
  const targetText = modelText ?? "";
  showRun(
    {
      summary: mergedSummary,
      table,
      files: {},
      code: mergedSummary.status === "completed" ? 0 : 1,
      wallSeconds: mergedSummary.elapsed_s ?? 0,
    },
    targetText,
    null,
    true,
  );
  if (!modelText) {
    state.resultText = null;
    updateButtons();
  }
  selectTab(modelText ? "diagram" : "waveforms");
  setStatus(`Opened batch case "${caseName}".`, mergedSummary.status === "completed" ? "good" : "error");
}

async function loadExample(file, run) {
  const entry = state.manifest.examples.find((e) => e.file === file);
  if (!entry) return;
  exampleSelect.value = file;
  $("example-description").textContent = entry.description + (entry.note ? ` ${entry.note}` : "");
  try {
    const response = await fetch(`examples/${file}`);
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    const text = await response.text();
    if (exampleSelect.value !== file) return; // another one was chosen meanwhile
    if (state.busy) {
      state.runGen += 1;
      clearBusy();
      engine.restart();
    }
    state.example = entry;
    state.original = text;
    state.visible = new Map();
    diagram.setSidecar(loadSidecarForExample(file), { tell: false });
    history.replaceState(null, "", `#${file.replace(/\.json$/, "")}`);
    setText(text);
    if (run) execute("run", true);
  } catch (e) {
    setStatus(`The example ${file} could not be loaded: ${e.message}`, "error");
  }
}

function exampleFromHash() {
  let name = "";
  try {
    name = decodeURIComponent(location.hash.slice(1));
  } catch {
    return null;
  }
  const file = `${name}.json`;
  return state.manifest.examples.some((e) => e.file === file) ? file : null;
}

async function start() {
  if (typeof config.repositoryUrl === "string" && /^https:\/\//.test(config.repositoryUrl)) {
    for (const [box, link] of [["source-fact", "source-link"], ["footer-source", "footer-source-link"]]) {
      $(link).href = config.repositoryUrl;
      if (link === "footer-source-link") $(link).textContent = config.repositoryUrl.replace(/^https?:\/\//, "");
      $(box).hidden = false;
    }
  }
  renderGutter();
  try {
    const response = await fetch("examples.json");
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    state.manifest = await response.json();
  } catch (e) {
    setStatus(`The list of examples could not be loaded: ${e.message}`, "error");
    return;
  }
  for (const group of state.manifest.groups) {
    const entries = state.manifest.examples.filter((e) => e.group === group.id);
    if (entries.length === 0) continue;
    exampleSelect.append(
      h(
        "optgroup",
        { label: group.title },
        entries.map((e) => h("option", { value: e.file, text: e.title })),
      ),
    );
  }
  exampleSelect.addEventListener("change", () => loadExample(exampleSelect.value, true));
  window.addEventListener("hashchange", () => {
    const next = exampleFromHash();
    if (next && next !== state.example?.file) loadExample(next, true);
  });
  const first = exampleFromHash() ?? state.manifest.default;
  loadExample(first, true);
}

// ---- color scheme ----

const THEMES = ["auto", "light", "dark"];
function applyTheme(theme) {
  if (theme === "auto") delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = theme;
  $("theme").textContent = `Theme: ${theme}`;
}
let theme = "auto";
try {
  const saved = localStorage.getItem("openkeito-theme");
  if (THEMES.includes(saved)) theme = saved;
} catch {
  // No storage (private window): the choice lasts for this visit.
}
applyTheme(theme);
$("theme").addEventListener("click", () => {
  theme = THEMES[(THEMES.indexOf(theme) + 1) % THEMES.length];
  applyTheme(theme);
  try {
    localStorage.setItem("openkeito-theme", theme);
  } catch {
    // See above.
  }
});

start();
