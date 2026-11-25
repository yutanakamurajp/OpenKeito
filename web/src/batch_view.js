// Browser view for inspecting, filtering, threshold-checking, comparing,
// and opening individual cases from `openkeito batch` or Python batch runs.

import {
  buildBatchModel,
  entriesFromFileList,
  extractComparisonSeries,
  filterAndSortCases,
} from "./batch.js";
import { Chart } from "./chart.js";
import { fill, h } from "./dom.js";
import { formatDuration, formatValue } from "./format.js";

let nextViewId = 1;

const STATUS_OPTIONS = [
  { value: "all", label: "All statuses" },
  { value: "completed", label: "Completed" },
  { value: "failed", label: "Failed" },
  { value: "invalid", label: "Invalid" },
];

const METRIC_FIELD_OPTIONS = [
  { value: "max_abs", label: "max |x|" },
  { value: "max", label: "max" },
  { value: "min", label: "min" },
  { value: "last", label: "last" },
];

function parseThresholdInput(value) {
  if (typeof value !== "string") return null;
  const trimmed = value.trim();
  if (trimmed === "") return null;
  const parsed = Number(trimmed);
  return Number.isFinite(parsed) ? parsed : null;
}

function formatErrorSummary(errors) {
  if (!Array.isArray(errors) || errors.length === 0) return "–";
  const first = errors[0];
  let text = "";
  if (typeof first === "string") {
    text = first;
  } else if (first && typeof first === "object") {
    const head = [first.code, first.message].filter(Boolean).join(": ");
    text = head || first.path || "error";
  }
  if (!text) text = "error";
  return errors.length > 1 ? `${text} (+${errors.length - 1})` : text;
}

function formatViolationTitle(violations) {
  if (!Array.isArray(violations) || violations.length === 0) return "";
  return violations
    .map((v) => `${v.column} ${v.field}=${formatValue(v.value)} (${v.kind} limit ${formatValue(v.limit)})`)
    .join("; ");
}

function resampleSeries(srcTime, srcValues, targetTime) {
  const out = new Float64Array(targetTime.length);
  const n = srcTime.length;
  if (n === 0) {
    out.fill(NaN);
    return out;
  }
  let j = 0;
  for (let i = 0; i < targetTime.length; i += 1) {
    const t = targetTime[i];
    if (t < srcTime[0] || t > srcTime[n - 1]) {
      out[i] = NaN;
      continue;
    }
    while (j + 1 < n && srcTime[j + 1] < t) j += 1;
    if (srcTime[j] === t) {
      out[i] = srcValues[j];
    } else if (j + 1 < n && srcTime[j + 1] === t) {
      out[i] = srcValues[j + 1];
    } else if (j + 1 < n) {
      const t0 = srcTime[j];
      const t1 = srcTime[j + 1];
      const v0 = srcValues[j];
      const v1 = srcValues[j + 1];
      if (t1 > t0 && Number.isFinite(v0) && Number.isFinite(v1)) {
        const alpha = (t - t0) / (t1 - t0);
        out[i] = v0 + alpha * (v1 - v0);
      } else {
        out[i] = NaN;
      }
    } else {
      out[i] = NaN;
    }
  }
  return out;
}

/// Aligns comparison series onto a shared monotonic time axis without copying
/// when all selected cases already share the same time grid.
export function alignComparisonSeries(seriesList) {
  if (!Array.isArray(seriesList) || seriesList.length === 0) {
    return { time: new Float64Array(0), series: [] };
  }

  const ref = seriesList[0].time;
  let sameGrid = true;
  for (let k = 1; k < seriesList.length && sameGrid; k += 1) {
    const t = seriesList[k].time;
    if (t.length !== ref.length) {
      sameGrid = false;
      break;
    }
    for (let i = 0; i < ref.length; i += 1) {
      if (t[i] !== ref[i]) {
        sameGrid = false;
        break;
      }
    }
  }

  if (sameGrid) {
    return {
      time: ref,
      series: seriesList.map((s) => ({ ...s, values: s.values })),
    };
  }

  const allTimes = [];
  for (const s of seriesList) {
    for (let i = 0; i < s.time.length; i += 1) {
      const t = s.time[i];
      if (Number.isFinite(t)) allTimes.push(t);
    }
  }
  allTimes.sort((a, b) => a - b);

  const uniqueTimes = [];
  for (const t of allTimes) {
    if (uniqueTimes.length === 0 || t !== uniqueTimes[uniqueTimes.length - 1]) {
      uniqueTimes.push(t);
    }
  }
  const mergedTime = Float64Array.from(uniqueTimes);

  return {
    time: mergedTime,
    series: seriesList.map((s) => ({
      ...s,
      values: resampleSeries(s.time, s.values, mergedTime),
    })),
  };
}

export class BatchView {
  /// `root`: container element for the batch view.
  /// `onOpenCase`: invoked with `{ caseName, caseInfo, summary, table, events, modelText }`.
  constructor(root, { onOpenCase = () => {} } = {}) {
    if (!root) {
      throw new Error("BatchView requires a root element");
    }
    this.root = root;
    this.root.classList.add("batch-view");
    this.onOpenCase = onOpenCase;
    const idPrefix = `batch-view-${nextViewId++}`;

    this.model = null;
    this.batchGeneration = 0;
    this.error = null;
    this.selectedCases = new Set();
    this.modelTextCache = new Map();
    this.compareSeq = 0;
    this.lastCompareKey = "";
    this.filters = {
      status: "all",
      query: "",
      column: "",
      metricField: "max_abs",
      direction: "desc",
      thresholdMin: "",
      thresholdMax: "",
    };

    this.folderInput = h("input", {
      type: "file",
      webkitdirectory: true,
      multiple: true,
      "aria-label": "Load batch folder",
      "data-role": "folder-input",
      onchange: (e) => {
        void this.handleFileInput(e.target);
      },
    });

    this.filesInput = h("input", {
      type: "file",
      multiple: true,
      "aria-label": "Select batch files",
      "data-role": "files-input",
      onchange: (e) => {
        void this.handleFileInput(e.target);
      },
    });

    this.loadBar = h(
      "div",
      { class: "batch-load actions" },
      h(
        "label",
        { class: "field batch-upload" },
        h("span", { text: "Batch folder" }),
        this.folderInput,
      ),
      h(
        "label",
        { class: "field batch-upload" },
        h("span", { text: "Or select files" }),
        this.filesInput,
      ),
    );

    this.statusEl = h("p", { class: "status batch-status", role: "status", "aria-live": "polite" });
    this.summaryEl = h("section", { class: "batch-summary", "aria-label": "Batch summary" });

    this.statusSelect = h(
      "select",
      {
        id: `${idPrefix}-status`,
        "data-filter": "status",
        onchange: () => {
          this.filters.status = this.statusSelect.value;
          this.renderTable();
        },
      },
      STATUS_OPTIONS.map((opt) => h("option", { value: opt.value, text: opt.label })),
    );

    this.queryInput = h("input", {
      id: `${idPrefix}-query`,
      type: "search",
      placeholder: "Search case, model, error…",
      "data-filter": "query",
      oninput: () => {
        this.filters.query = this.queryInput.value;
        this.renderTable();
      },
    });

    this.columnSelect = h("select", {
      id: `${idPrefix}-column`,
      "data-filter": "column",
      onchange: () => {
        this.filters.column = this.columnSelect.value;
        this.renderTable();
        void this.updateComparison();
      },
    });

    this.metricFieldSelect = h(
      "select",
      {
        id: `${idPrefix}-metric`,
        "data-filter": "metricField",
        onchange: () => {
          this.filters.metricField = this.metricFieldSelect.value;
          this.renderTable();
        },
      },
      METRIC_FIELD_OPTIONS.map((opt) => h("option", { value: opt.value, text: opt.label })),
    );

    this.directionButton = h("button", {
      id: `${idPrefix}-direction`,
      type: "button",
      class: "batch-sort-dir",
      "aria-label": "Toggle sort direction",
      "data-filter": "direction",
      onclick: () => {
        this.filters.direction = this.filters.direction === "desc" ? "asc" : "desc";
        this.syncDirectionButton();
        this.renderTable();
      },
    });

    this.thresholdMinInput = h("input", {
      id: `${idPrefix}-min`,
      type: "number",
      step: "any",
      placeholder: "min",
      "data-filter": "thresholdMin",
      oninput: () => {
        this.filters.thresholdMin = this.thresholdMinInput.value;
        this.renderTable();
      },
    });

    this.thresholdMaxInput = h("input", {
      id: `${idPrefix}-max`,
      type: "number",
      step: "any",
      placeholder: "max",
      "data-filter": "thresholdMax",
      oninput: () => {
        this.filters.thresholdMax = this.thresholdMaxInput.value;
        this.renderTable();
      },
    });

    this.filtersEl = h(
      "section",
      { class: "batch-filters", "aria-label": "Filter, sort and thresholds" },
      h("div", { class: "field" }, h("label", { for: `${idPrefix}-status`, text: "Status" }), this.statusSelect),
      h("div", { class: "field" }, h("label", { for: `${idPrefix}-query`, text: "Search" }), this.queryInput),
      h("div", { class: "field" }, h("label", { for: `${idPrefix}-column`, text: "Column" }), this.columnSelect),
      h("div", { class: "field" }, h("label", { for: `${idPrefix}-metric`, text: "Sort by" }), this.metricFieldSelect),
      h("div", { class: "field" }, h("label", { for: `${idPrefix}-direction`, text: "Order" }), this.directionButton),
      h("div", { class: "field" }, h("label", { for: `${idPrefix}-min`, text: "Min limit" }), this.thresholdMinInput),
      h("div", { class: "field" }, h("label", { for: `${idPrefix}-max`, text: "Max limit" }), this.thresholdMaxInput),
    );

    this.tableWrapEl = h("section", { class: "batch-table-section", "aria-label": "Batch cases" });

    this.compareTitleEl = h("h3", { text: "Waveform comparison" });
    this.compareStatusEl = h("p", { class: "hint batch-compare-status" });
    this.compareEmptyEl = h("p", {
      class: "empty batch-compare-empty",
      text: "Select one or more cases in the table above to compare their waveforms.",
    });
    this.chartEl = h("div", { class: "batch-compare-chart", hidden: true });
    this.chart = new Chart(this.chartEl);

    this.compareEl = h(
      "section",
      { class: "batch-compare", "aria-label": "Waveform comparison" },
      this.compareTitleEl,
      this.compareStatusEl,
      this.compareEmptyEl,
      this.chartEl,
    );

    fill(
      this.root,
      this.loadBar,
      this.statusEl,
      this.summaryEl,
      this.filtersEl,
      this.tableWrapEl,
      this.compareEl,
    );

    this.syncFilterControls();
    this.render();
  }

  async handleFileInput(input) {
    const files = input?.files;
    if (!files || files.length === 0) return;
    try {
      await this.loadFiles(files);
    } catch {
      // Error state is already stored and rendered by loadEntries.
    }
  }

  async loadFiles(fileList) {
    const entries = entriesFromFileList(fileList);
    return this.loadEntries(entries);
  }

  async loadEntries(entries) {
    this.error = null;
    this.selectedCases.clear();
    this.modelTextCache.clear();
    this.compareSeq += 1;
    this.lastCompareKey = "";
    try {
      const model = await buildBatchModel(entries);
      this.model = model;
      this.batchGeneration += 1;
      this.filters.column = model.columns[0]?.name ?? "";
      this.render();
      return model;
    } catch (err) {
      this.model = null;
      this.error = err?.message ?? String(err);
      this.render();
      throw err;
    }
  }

  getModel() {
    return this.model;
  }

  syncDirectionButton() {
    const isDesc = this.filters.direction === "desc";
    this.directionButton.textContent = isDesc ? "Desc ↓" : "Asc ↑";
    this.directionButton.dataset.direction = this.filters.direction;
  }

  syncColumnOptions() {
    const columns = this.model?.columns ?? [];
    const options = columns.length > 0
      ? columns.map((col) =>
          h("option", {
            value: col.name,
            text: col.unit ? `${col.name} (${col.unit})` : col.name,
          }),
        )
      : [h("option", { value: "", text: "No metric columns" })];

    fill(this.columnSelect, options);

    if (columns.length === 0) {
      this.filters.column = "";
    } else if (!columns.some((c) => c.name === this.filters.column)) {
      this.filters.column = columns[0].name;
    }
    this.columnSelect.value = this.filters.column;
    this.columnSelect.disabled = columns.length === 0;
  }

  syncFilterControls() {
    this.statusSelect.value = this.filters.status;
    this.queryInput.value = this.filters.query;
    this.metricFieldSelect.value = this.filters.metricField;
    this.thresholdMinInput.value = this.filters.thresholdMin;
    this.thresholdMaxInput.value = this.filters.thresholdMax;
    this.syncDirectionButton();
    this.syncColumnOptions();
  }

  currentThreshold() {
    const column = this.filters.column;
    if (!column) return null;
    const min = parseThresholdInput(this.filters.thresholdMin);
    const max = parseThresholdInput(this.filters.thresholdMax);
    if (min === null && max === null) return null;
    const rule = { column };
    if (min !== null) rule.min = min;
    if (max !== null) rule.max = max;
    return rule;
  }

  filteredCases() {
    if (!this.model) return [];
    return filterAndSortCases(this.model, {
      status: this.filters.status,
      query: this.filters.query,
      column: this.filters.column || null,
      metricField: this.filters.metricField,
      direction: this.filters.direction,
      threshold: this.currentThreshold(),
    });
  }

  renderStatus() {
    if (this.error) {
      this.statusEl.className = "status batch-status error";
      this.statusEl.textContent = this.error;
      return;
    }
    if (!this.model) {
      this.statusEl.className = "status batch-status";
      this.statusEl.textContent = "Choose a batch output folder (containing batch-summary.json) to inspect cases.";
      return;
    }
    this.statusEl.className = `status batch-status ${this.model.summary.status === "completed" ? "good" : ""}`.trim();
    this.statusEl.textContent = `Loaded ${this.model.cases.length.toLocaleString("en-US")} cases.`;
  }

  renderSummary() {
    if (!this.model) {
      fill(this.summaryEl);
      return;
    }
    const { summary, root } = this.model;
    const counts = summary.counts;
    const batchName = summary.batch?.name || root || "Batch";
    const elapsedText = summary.elapsed_s !== null ? formatDuration(summary.elapsed_s) : "–";

    fill(
      this.summaryEl,
      h(
        "div",
        { class: "batch-summary-head" },
        h("h3", { class: "batch-name", "data-stat": "name", text: batchName }),
        h("span", {
          class: `badge batch-overall-status status-${summary.status}`,
          text: summary.status,
        }),
      ),
      h(
        "dl",
        { class: "facts batch-facts" },
        h("div", { "data-stat": "completed" }, h("dt", { text: "Completed" }), h("dd", { text: String(counts.completed) })),
        h("div", { "data-stat": "failed" }, h("dt", { text: "Failed" }), h("dd", { text: String(counts.failed) })),
        h("div", { "data-stat": "invalid" }, h("dt", { text: "Invalid" }), h("dd", { text: String(counts.invalid) })),
        h("div", { "data-stat": "elapsed" }, h("dt", { text: "Elapsed" }), h("dd", { text: elapsedText })),
      ),
    );
  }

  renderTable() {
    if (!this.model) {
      fill(
        this.tableWrapEl,
        h("p", { class: "empty", text: "No batch loaded yet." }),
      );
      return;
    }

    const cases = this.filteredCases();
    const breachCount = cases.filter((c) => c.exceeded).length;
    const selectedCol = this.filters.column;
    const colMeta = this.model.columns.find((c) => c.name === selectedCol);
    const colSuffix = colMeta ? (colMeta.unit ? ` (${colMeta.unit})` : "") : "";
    const colTitle = selectedCol ? `${selectedCol}${colSuffix}` : "Metric";

    if (cases.length === 0) {
      fill(
        this.tableWrapEl,
        h("p", { class: "empty", text: "No cases match the current filter." }),
      );
      return;
    }

    const rows = cases.map((c) => {
      const compareCheckbox = h("input", {
        type: "checkbox",
        class: "batch-compare-check",
        "data-case": c.name,
        checked: this.selectedCases.has(c.name),
        disabled: !c.hasResult,
        "aria-label": `Compare ${c.name}`,
        onchange: () => {
          void this.toggleCaseComparison(c.name, compareCheckbox.checked);
        },
      });

      const statusCell = h(
        "td",
        { class: "batch-status-cell" },
        h("span", { class: `badge batch-status-badge status-${c.status}`, text: c.status }),
        c.exceeded &&
          h("span", {
            class: "badge breach-badge",
            title: formatViolationTitle(c.violations),
            text: "breach",
          }),
      );

      const openButton = h("button", {
        type: "button",
        class: "batch-open-btn",
        "data-case": c.name,
        onclick: () => {
          void this.openCase(c.name).catch(() => {});
        },
      }, "Open");

      const m = c.metric;
      return h(
        "tr",
        {
          class: c.exceeded ? "batch-row breach" : "batch-row",
          "data-case": c.name,
          "data-status": c.status,
        },
        h("td", {}, compareCheckbox),
        h("th", { scope: "row", title: c.model || c.name, text: c.name }),
        statusCell,
        h("td", { class: "number", text: String(c.warnings ?? 0) }),
        h("td", { class: "batch-error-cell", text: formatErrorSummary(c.errors) }),
        h("td", { class: "number", text: formatValue(m?.min) }),
        h("td", { class: "number", text: formatValue(m?.max) }),
        h("td", { class: "number", text: formatValue(m?.max_abs) }),
        h("td", { class: "number", text: formatValue(m?.last) }),
        h("td", {}, openButton),
      );
    });

    const captionText =
      `Cases (${cases.length.toLocaleString("en-US")} of ${this.model.cases.length.toLocaleString("en-US")})` +
      (breachCount > 0 ? ` — ${breachCount.toLocaleString("en-US")} threshold breach${breachCount === 1 ? "" : "es"}` : "");

    fill(
      this.tableWrapEl,
      h(
        "div",
        { class: "table-scroll" },
        h(
          "table",
          { class: "batch-table" },
          h("caption", { text: captionText }),
          h(
            "thead",
            {},
            h(
              "tr",
              {},
              h("th", { scope: "col", text: "Compare" }),
              h("th", { scope: "col", text: "Case" }),
              h("th", { scope: "col", text: "Status" }),
              h("th", { scope: "col", class: "number", text: "Warnings" }),
              h("th", { scope: "col", text: "Errors" }),
              h("th", { scope: "col", class: "number", text: `${colTitle} min` }),
              h("th", { scope: "col", class: "number", text: `${colTitle} max` }),
              h("th", { scope: "col", class: "number", text: `${colTitle} max |x|` }),
              h("th", { scope: "col", class: "number", text: `${colTitle} last` }),
              h("th", { scope: "col", text: "Action" }),
            ),
          ),
          h("tbody", {}, rows),
        ),
      ),
    );
  }

  async toggleCaseComparison(caseName, checked) {
    if (checked) this.selectedCases.add(caseName);
    else this.selectedCases.delete(caseName);
    return this.updateComparison();
  }

  async updateComparison() {
    if (!this.model || this.selectedCases.size === 0) {
      this.compareSeq += 1;
      this.lastCompareKey = "";
      this.compareStatusEl.textContent = "";
      this.compareEmptyEl.textContent = "Select one or more cases in the table above to compare their waveforms.";
      this.compareEmptyEl.hidden = false;
      this.chartEl.hidden = true;
      return [];
    }

    const column = this.filters.column;
    if (!column) {
      this.compareSeq += 1;
      this.lastCompareKey = "";
      this.compareStatusEl.textContent = "";
      this.compareEmptyEl.textContent = "Select a metric column to compare waveforms.";
      this.compareEmptyEl.hidden = false;
      this.chartEl.hidden = true;
      return [];
    }

    const selectedNames = this.model.cases
      .map((c) => c.name)
      .filter((name) => this.selectedCases.has(name));
    const compareKey = `${this.batchGeneration}|${column}|${selectedNames.join("\0")}`;
    if (compareKey === this.lastCompareKey) {
      return null;
    }

    const seq = ++this.compareSeq;
    this.compareStatusEl.textContent = `Loading ${selectedNames.length} case${selectedNames.length === 1 ? "" : "s"}…`;

    let loaded;
    try {
      loaded = await Promise.all(selectedNames.map((name) => this.model.loadCase(name)));
    } catch (err) {
      if (seq !== this.compareSeq) return null;
      this.compareStatusEl.textContent = "";
      this.compareEmptyEl.textContent = `Could not load waveform data: ${err?.message ?? String(err)}`;
      this.compareEmptyEl.hidden = false;
      this.chartEl.hidden = true;
      return [];
    }

    if (seq !== this.compareSeq) return null;

    const series = extractComparisonSeries(loaded, column);
    this.lastCompareKey = compareKey;

    if (series.length === 0) {
      this.compareStatusEl.textContent = "";
      this.compareEmptyEl.textContent = `No waveform data found for "${column}" in the selected cases.`;
      this.compareEmptyEl.hidden = false;
      this.chartEl.hidden = true;
      return series;
    }

    const aligned = alignComparisonSeries(series);
    const unit = series.find((s) => s.unit)?.unit ?? "";
    this.compareEmptyEl.hidden = true;
    this.chartEl.hidden = false;
    this.compareStatusEl.textContent = `Comparing ${series.length} case${series.length === 1 ? "" : "s"} on ${column}${unit ? ` (${unit})` : ""}.`;
    this.chart.setData({
      x: { data: aligned.time, label: "time", unit: "s" },
      panels: [
        {
          title: unit ? `${column} (${unit})` : column,
          unit,
          series: aligned.series.map((s) => ({
            name: s.caseName,
            data: s.values,
            visible: true,
          })),
        },
      ],
    });
    return series;
  }

  async openCase(caseName) {
    if (!this.model) return null;
    const caseInfo = this.model.cases.find((c) => c.name === caseName) ?? null;
    try {
      const [loaded, modelText] = await Promise.all([
        this.model.loadCase(caseName),
        this.readModelText(caseName, caseInfo),
      ]);
      const payload = {
        caseName,
        caseInfo,
        summary: loaded.summary,
        table: loaded.result,
        events: loaded.events,
        modelText,
      };
      this.onOpenCase(payload);
      return payload;
    } catch (err) {
      this.error = `Could not open case "${caseName}": ${err?.message ?? String(err)}`;
      this.renderStatus();
      throw err;
    }
  }

  async readModelText(caseName, caseInfo) {
    if (this.modelTextCache.has(caseName)) {
      return this.modelTextCache.get(caseName);
    }
    const modelEntry = caseInfo?.files?.model ?? null;
    const text = modelEntry ? await modelEntry.readText() : null;
    this.modelTextCache.set(caseName, text);
    return text;
  }

  render() {
    this.syncFilterControls();
    this.renderStatus();
    this.renderSummary();
    this.renderTable();
    void this.updateComparison();
  }
}
