// Pure ES module for lazy loading, indexing, filtering, sorting, and
// comparing outputs produced by `openkeito batch` and Python batch runners.

import { parseNumericCsv } from "./csv.js";

const VALID_CASE_STATUSES = new Set(["completed", "failed", "invalid"]);
const VALID_METRIC_FIELDS = new Set(["min", "max", "max_abs", "last"]);
const REQUIRED_METRICS_HEADERS = ["case", "status", "column", "unit", "min", "max", "max_abs", "last"];

function finiteNumberOr(value, fallback) {
  return typeof value === "number" && Number.isFinite(value) ? value : fallback;
}

function parseOptionalNumber(cell) {
  const trimmed = cell.trim();
  if (trimmed === "") return null;
  const value = Number(trimmed);
  return Number.isFinite(value) ? value : null;
}

function splitCsvLines(text) {
  const lines = [];
  for (let i = 0; i < text.length; ) {
    const next = text.indexOf("\n", i);
    const end = next < 0 ? text.length : next;
    let line = text.slice(i, end);
    if (line.endsWith("\r")) line = line.slice(0, -1);
    if (line.length > 0) lines.push(line);
    i = end + 1;
  }
  return lines;
}

function normalizePath(rawPath) {
  return String(rawPath ?? "")
    .replace(/\\/g, "/")
    .replace(/\/+/g, "/")
    .replace(/^(?:\.\/)+/, "")
    .replace(/^\/+/, "")
    .replace(/\/+$/, "");
}

function basename(path) {
  const idx = path.lastIndexOf("/");
  return idx < 0 ? path : path.slice(idx + 1);
}

function emptyCaseFiles() {
  return {
    summary: null,
    result: null,
    events: null,
    phasors: null,
    model: null,
    extra: new Map(),
  };
}

/// Parses and validates `batch-summary.json`, returning a normalized object.
export function parseBatchSummary(text) {
  if (typeof text !== "string" || text.trim() === "") {
    throw new Error("batch-summary.json must be a non-empty JSON string");
  }
  let raw;
  try {
    raw = JSON.parse(text);
  } catch (err) {
    throw new Error(`Invalid JSON in batch-summary.json: ${err.message}`);
  }
  if (raw === null || typeof raw !== "object" || Array.isArray(raw)) {
    throw new Error("batch-summary.json root must be a JSON object");
  }
  if (raw.command !== "batch") {
    throw new Error(`batch-summary.json expected command "batch", got ${JSON.stringify(raw.command)}`);
  }
  if (raw.status !== "completed" && raw.status !== "failed") {
    throw new Error(`batch-summary.json has invalid status ${JSON.stringify(raw.status)}`);
  }
  if (!Array.isArray(raw.cases)) {
    throw new Error("batch-summary.json must contain a cases array");
  }

  const cases = raw.cases.map((c, index) => {
    if (c === null || typeof c !== "object" || Array.isArray(c)) {
      throw new Error(`batch-summary.json cases[${index}] must be an object`);
    }
    if (typeof c.name !== "string" || c.name === "") {
      throw new Error(`batch-summary.json cases[${index}].name must be a non-empty string`);
    }
    if (!VALID_CASE_STATUSES.has(c.status)) {
      throw new Error(`batch-summary.json cases[${index}] (${c.name}) has invalid status ${JSON.stringify(c.status)}`);
    }
    const warnings = Array.isArray(c.warnings)
      ? c.warnings.length
      : Math.max(0, Math.trunc(finiteNumberOr(c.warnings, 0)));
    return {
      name: c.name,
      status: c.status,
      model: typeof c.model === "string" ? c.model : "",
      out_dir: typeof c.out_dir === "string" ? c.out_dir : null,
      elapsed_s: finiteNumberOr(c.elapsed_s, null),
      warnings,
      errors: Array.isArray(c.errors) ? c.errors : [],
    };
  });

  const derivedCounts = { completed: 0, failed: 0, invalid: 0 };
  for (const c of cases) derivedCounts[c.status] += 1;

  const rawCounts = raw.counts && typeof raw.counts === "object" && !Array.isArray(raw.counts) ? raw.counts : null;
  const counts = {
    completed: rawCounts ? Math.max(0, Math.trunc(finiteNumberOr(rawCounts.completed, derivedCounts.completed))) : derivedCounts.completed,
    failed: rawCounts ? Math.max(0, Math.trunc(finiteNumberOr(rawCounts.failed, derivedCounts.failed))) : derivedCounts.failed,
    invalid: rawCounts ? Math.max(0, Math.trunc(finiteNumberOr(rawCounts.invalid, derivedCounts.invalid))) : derivedCounts.invalid,
  };

  const rawBatch = raw.batch && typeof raw.batch === "object" && !Array.isArray(raw.batch) ? raw.batch : {};
  return {
    tool: typeof raw.tool === "string" ? raw.tool : "openkeito",
    version: typeof raw.version === "string" ? raw.version : "",
    command: "batch",
    batch: {
      path: typeof rawBatch.path === "string" ? rawBatch.path : "",
      name: typeof rawBatch.name === "string" ? rawBatch.name : "",
      sha256: typeof rawBatch.sha256 === "string" ? rawBatch.sha256 : "",
    },
    status: raw.status,
    jobs: finiteNumberOr(raw.jobs, null),
    counts,
    cases,
    metrics_csv_path: typeof raw.metrics_csv_path === "string" ? raw.metrics_csv_path : null,
    elapsed_s: finiteNumberOr(raw.elapsed_s, null),
    errors: Array.isArray(raw.errors) ? raw.errors : [],
  };
}

/// Parses `batch-metrics.csv` into `{ rows, byCase, columns }`.
export function parseBatchMetrics(text) {
  if (typeof text !== "string") {
    throw new Error("batch-metrics.csv must be a string");
  }
  const lines = splitCsvLines(text);
  if (lines.length === 0) {
    return { rows: [], byCase: new Map(), columns: [] };
  }

  const header = lines[0].split(",").map((h) => h.trim());
  const indexOf = new Map();
  header.forEach((name, idx) => {
    if (!indexOf.has(name)) indexOf.set(name, idx);
  });
  for (const required of REQUIRED_METRICS_HEADERS) {
    if (!indexOf.has(required)) {
      throw new Error(`batch-metrics.csv is missing required column "${required}"`);
    }
  }

  const iCase = indexOf.get("case");
  const iStatus = indexOf.get("status");
  const iCol = indexOf.get("column");
  const iUnit = indexOf.get("unit");
  const iMin = indexOf.get("min");
  const iMax = indexOf.get("max");
  const iMaxAbs = indexOf.get("max_abs");
  const iLast = indexOf.get("last");

  const rows = [];
  const byCase = new Map();
  const columns = [];
  const seenColumns = new Set();

  for (let r = 1; r < lines.length; r += 1) {
    const cells = lines[r].split(",");
    const cell = (idx) => (idx < cells.length ? cells[idx].trim() : "");
    const caseName = cell(iCase);
    if (caseName === "") continue;

    const status = cell(iStatus);
    const column = cell(iCol);
    const unit = cell(iUnit);
    const min = parseOptionalNumber(cell(iMin));
    const max = parseOptionalNumber(cell(iMax));
    const max_abs = parseOptionalNumber(cell(iMaxAbs));
    const last = parseOptionalNumber(cell(iLast));

    const row = { case: caseName, status, column, unit, min, max, max_abs, last };
    rows.push(row);

    let caseRecord = byCase.get(caseName);
    if (!caseRecord) {
      caseRecord = { case: caseName, status, metrics: new Map(), rows: [] };
      byCase.set(caseName, caseRecord);
    }
    if (column !== "") {
      const metric = { column, unit, min, max, max_abs, last };
      caseRecord.metrics.set(column, metric);
      caseRecord.rows.push(metric);
      if (!seenColumns.has(column)) {
        seenColumns.add(column);
        columns.push({ name: column, unit });
      }
    }
  }

  return { rows, byCase, columns };
}

/// Parses an `events.csv` file into an array of event objects.
export function parseEventsCsv(text) {
  if (typeof text !== "string" || text.trim() === "") return [];
  const lines = splitCsvLines(text);
  if (lines.length <= 1) return [];

  const header = lines[0].split(",").map((h) => h.trim());
  const iTime = header.indexOf("time_s");
  const iComp = header.indexOf("component");
  const iAction = header.indexOf("action");
  const iCurr = header.indexOf("interrupted_current_a");
  if (iTime < 0 || iComp < 0 || iAction < 0) {
    throw new Error("events.csv is missing required columns (time_s, component, action)");
  }

  const events = [];
  for (let r = 1; r < lines.length; r += 1) {
    const cells = lines[r].split(",");
    const cell = (idx) => (idx >= 0 && idx < cells.length ? cells[idx].trim() : "");
    const time_s = parseOptionalNumber(cell(iTime));
    if (time_s === null) continue;
    events.push({
      time_s,
      component: cell(iComp),
      action: cell(iAction),
      interrupted_current_a: iCurr >= 0 ? parseOptionalNumber(cell(iCurr)) : null,
    });
  }
  return events;
}

/// Converts a browser FileList or array/iterable of File-like or mock entry
/// objects into normalized `{ path, name, readText }` entries.
export function entriesFromFileList(fileList) {
  if (!fileList) return [];
  const items = Array.from(fileList);
  return items.map((file, index) => {
    if (file === null || typeof file !== "object") {
      throw new Error(`Invalid file entry at index ${index}`);
    }
    const rawPath = file.webkitRelativePath || file.relativePath || file.path || file.name || "";
    const path = normalizePath(rawPath);
    const name = typeof file.name === "string" && file.name !== "" ? file.name : basename(path);
    let readText;
    if (typeof file.readText === "function") {
      readText = () => file.readText();
    } else if (typeof file.text === "function") {
      readText = () => file.text();
    } else if (typeof file.content === "string") {
      const content = file.content;
      readText = async () => content;
    } else {
      throw new Error(`Entry "${path || name}" has neither readText() nor text()`);
    }
    return { path, name, readText };
  });
}

function detectRootPrefix(normalizedEntries) {
  let best = null;
  for (const target of ["batch-summary.json", "batch-metrics.csv"]) {
    for (const item of normalizedEntries) {
      if (basename(item.normPath) === target) {
        const slash = item.normPath.lastIndexOf("/");
        const prefix = slash < 0 ? "" : item.normPath.slice(0, slash + 1);
        const depth = prefix === "" ? 0 : prefix.split("/").length - 1;
        if (!best || depth < best.depth) {
          best = { prefix, depth };
        }
      }
    }
    if (best) return best.prefix;
  }
  return "";
}

function findMatchingEntry(targetNormPath, externalEntries) {
  const exact = externalEntries.find((item) => item.normPath === targetNormPath);
  if (exact) return exact.entry;
  const suffix = `/${targetNormPath}`;
  const suffixMatches = externalEntries.filter((item) => item.normPath.endsWith(suffix));
  return suffixMatches.length === 1 ? suffixMatches[0].entry : null;
}

function isCaseOutputEntry(normPath, rootPrefix, caseNames) {
  if (rootPrefix !== "" && !normPath.startsWith(rootPrefix)) return false;
  const relPath = normPath.slice(rootPrefix.length);
  if (relPath === "batch-summary.json" || relPath === "batch-metrics.csv") return true;
  const parts = relPath.split("/");
  if (parts.length < 2) return false;
  if (parts[0] === "cases" && parts.length >= 3) return caseNames.has(parts[1]);
  return caseNames.has(parts[0]);
}

function resolveFallbackModelEntry(caseInfo, externalEntries) {
  const modelRef = caseInfo?.model;
  if (typeof modelRef !== "string" || modelRef.trim() === "") return null;
  const hashIdx = modelRef.indexOf("#");
  if (hashIdx > 0) {
    const batchPath = normalizePath(modelRef.slice(0, hashIdx));
    const inlineName = modelRef.slice(hashIdx + 1);
    const batchEntry = findMatchingEntry(batchPath, externalEntries);
    if (!batchEntry) return null;
    return {
      path: modelRef,
      name: `${caseInfo.name}.json`,
      readText: async () => {
        const batchJson = JSON.parse(await batchEntry.readText());
        const item = Array.isArray(batchJson?.cases)
          ? batchJson.cases.find((x) => x && x.name === inlineName)
          : null;
        if (!item || typeof item.model !== "object" || item.model === null) return null;
        return JSON.stringify(item.model, null, 2);
      },
    };
  }
  return findMatchingEntry(normalizePath(modelRef), externalEntries);
}

/// Indexes normalized `{ path, name, readText }` entries into batch root files
/// and per-case file slots (`summary`, `result`, `events`, `phasors`, `model`).
export function indexBatchFolder(entries) {
  const list = entriesFromFileList(entries);
  const normalized = list
    .map((entry) => ({
      entry,
      normPath: normalizePath(entry.path || entry.name),
    }))
    .filter((item) => item.normPath !== "");

  const rootPrefix = detectRootPrefix(normalized);
  let summary = null;
  let metrics = null;
  const cases = new Map();

  for (const { entry, normPath } of normalized) {
    if (rootPrefix !== "" && !normPath.startsWith(rootPrefix)) continue;
    const relPath = normPath.slice(rootPrefix.length);
    if (relPath === "") continue;

    if (relPath === "batch-summary.json") {
      summary = entry;
      continue;
    }
    if (relPath === "batch-metrics.csv") {
      metrics = entry;
      continue;
    }

    const slash = relPath.indexOf("/");
    if (slash < 0) continue;

    const parts = relPath.split("/");
    let caseName;
    let innerPath;
    if (parts[0] === "cases" && parts.length >= 3) {
      caseName = parts[1];
      innerPath = parts.slice(2).join("/");
    } else {
      caseName = parts[0];
      innerPath = parts.slice(1).join("/");
    }
    if (!caseName || !innerPath) continue;

    let slot = cases.get(caseName);
    if (!slot) {
      slot = emptyCaseFiles();
      cases.set(caseName, slot);
    }

    if (innerPath === "summary.json") {
      slot.summary = entry;
    } else if (innerPath === "result.csv") {
      slot.result = entry;
    } else if (innerPath === "events.csv") {
      slot.events = entry;
    } else if (innerPath === "phasors.csv") {
      slot.phasors = entry;
    } else if (innerPath === "model.json") {
      slot.model = entry;
    } else if (!innerPath.includes("/") && innerPath.endsWith(".json") && !slot.model) {
      slot.model = entry;
    }
    slot.extra.set(innerPath, entry);
  }

  return {
    root: rootPrefix.replace(/\/$/, ""),
    rootPrefix,
    summary,
    metrics,
    cases,
    normalized,
  };
}

/// Builds the initial batch model by reading ONLY `batch-summary.json` and
/// `batch-metrics.csv`. Per-case files are loaded lazily via `loadCase(name)`
/// and cached.
export async function buildBatchModel(entries) {
  const index = indexBatchFolder(entries);
  if (!index.summary) {
    throw new Error("batch-summary.json was not found in the selected folder");
  }

  const summaryText = await index.summary.readText();
  const summary = parseBatchSummary(summaryText);

  const metricsText = index.metrics ? await index.metrics.readText() : null;
  const metrics = metricsText !== null ? parseBatchMetrics(metricsText) : { rows: [], byCase: new Map(), columns: [] };

  const summaryCaseNames = new Set(summary.cases.map((c) => c.name));
  const externalEntries = index.normalized.filter(
    (item) => !isCaseOutputEntry(item.normPath, index.rootPrefix, summaryCaseNames),
  );

  const cases = [];
  const caseByName = new Map();
  for (const c of summary.cases) {
    const metricRecord = metrics.byCase.get(c.name);
    const files = index.cases.get(c.name) ?? emptyCaseFiles();
    if (!files.model) {
      const fallbackModel = resolveFallbackModelEntry(c, externalEntries);
      if (fallbackModel) files.model = fallbackModel;
    }
    const record = {
      name: c.name,
      status: c.status,
      model: c.model,
      out_dir: c.out_dir,
      elapsed_s: c.elapsed_s,
      warnings: c.warnings,
      errors: c.errors,
      metrics: metricRecord ? metricRecord.metrics : new Map(),
      files,
      hasSummary: Boolean(files.summary),
      hasResult: Boolean(files.result),
      hasEvents: Boolean(files.events),
      hasPhasors: Boolean(files.phasors),
      hasModel: Boolean(files.model),
    };
    cases.push(record);
    caseByName.set(c.name, record);
  }

  const caseCache = new Map();

  async function loadCase(caseName) {
    if (caseCache.has(caseName)) {
      return caseCache.get(caseName);
    }
    const record = caseByName.get(caseName);
    if (!record) {
      throw new Error(`Unknown batch case "${caseName}"`);
    }

    const promise = (async () => {
      const [caseSummaryText, resultText, eventsText] = await Promise.all([
        record.files.summary ? record.files.summary.readText() : Promise.resolve(null),
        record.files.result ? record.files.result.readText() : Promise.resolve(null),
        record.files.events ? record.files.events.readText() : Promise.resolve(null),
      ]);

      let caseSummary = null;
      if (caseSummaryText !== null) {
        try {
          caseSummary = JSON.parse(caseSummaryText);
        } catch (err) {
          throw new Error(`Invalid JSON in ${caseName}/summary.json: ${err.message}`);
        }
      }
      const result = resultText !== null ? parseNumericCsv(resultText) : null;
      const events = eventsText !== null ? parseEventsCsv(eventsText) : [];

      const units = new Map();
      for (const [colName, m] of record.metrics) {
        units.set(colName, m.unit ?? "");
      }
      const summaryCols = caseSummary?.result?.columns;
      if (Array.isArray(summaryCols)) {
        for (const col of summaryCols) {
          if (col && typeof col.name === "string") {
            units.set(col.name, typeof col.unit === "string" ? col.unit : "");
          }
        }
      }

      return {
        name: caseName,
        caseName,
        status: record.status,
        summary: caseSummary,
        result,
        events,
        units,
      };
    })();

    caseCache.set(caseName, promise);
    try {
      const resolved = await promise;
      caseCache.set(caseName, resolved);
      return resolved;
    } catch (err) {
      caseCache.delete(caseName);
      throw err;
    }
  }

  return {
    root: index.root,
    summary,
    metrics,
    cases,
    columns: metrics.columns,
    loadCase,
    isCaseLoaded: (caseName) => {
      const entry = caseCache.get(caseName);
      return Boolean(entry && typeof entry.then !== "function");
    },
    clearCache: () => caseCache.clear(),
  };
}

function matchesQuery(caseRecord, lowerQuery) {
  if (!lowerQuery) return true;
  if (caseRecord.name.toLowerCase().includes(lowerQuery)) return true;
  if (caseRecord.model && caseRecord.model.toLowerCase().includes(lowerQuery)) return true;
  if (Array.isArray(caseRecord.errors)) {
    for (const err of caseRecord.errors) {
      if (typeof err === "string") {
        if (err.toLowerCase().includes(lowerQuery)) return true;
      } else if (err && typeof err === "object") {
        for (const field of [err.code, err.path, err.message, err.hint]) {
          if (typeof field === "string" && field.toLowerCase().includes(lowerQuery)) return true;
        }
      }
    }
  }
  return false;
}

function evaluateThresholds(caseRecord, thresholds) {
  const violations = [];
  for (const rule of thresholds) {
    if (!rule || !rule.column) continue;
    const metric = caseRecord.metrics?.get(rule.column);
    if (!metric) continue;

    if (rule.field !== undefined && rule.field !== null) {
      if (!VALID_METRIC_FIELDS.has(rule.field)) {
        throw new Error(`Invalid threshold metric field ${JSON.stringify(rule.field)}`);
      }
      const value = metric[rule.field];
      if (typeof value !== "number" || !Number.isFinite(value)) continue;
      if (typeof rule.min === "number" && Number.isFinite(rule.min) && value < rule.min) {
        violations.push({ column: rule.column, field: rule.field, limit: rule.min, value, kind: "min" });
      }
      if (typeof rule.max === "number" && Number.isFinite(rule.max) && value > rule.max) {
        violations.push({ column: rule.column, field: rule.field, limit: rule.max, value, kind: "max" });
      }
      if (typeof rule.maxAbs === "number" && Number.isFinite(rule.maxAbs) && Math.abs(value) > rule.maxAbs) {
        violations.push({ column: rule.column, field: rule.field, limit: rule.maxAbs, value, kind: "maxAbs" });
      }
      continue;
    }

    if (typeof rule.min === "number" && Number.isFinite(rule.min) && typeof metric.min === "number" && metric.min < rule.min) {
      violations.push({ column: rule.column, field: "min", limit: rule.min, value: metric.min, kind: "min" });
    }
    if (typeof rule.max === "number" && Number.isFinite(rule.max) && typeof metric.max === "number" && metric.max > rule.max) {
      violations.push({ column: rule.column, field: "max", limit: rule.max, value: metric.max, kind: "max" });
    }
    if (typeof rule.maxAbs === "number" && Number.isFinite(rule.maxAbs) && typeof metric.max_abs === "number" && metric.max_abs > rule.maxAbs) {
      violations.push({ column: rule.column, field: "max_abs", limit: rule.maxAbs, value: metric.max_abs, kind: "maxAbs" });
    }
  }
  return violations;
}

/// Filters, sorts, and flags threshold violations for cases in a batch model.
export function filterAndSortCases(batchModel, options = {}) {
  const source = Array.isArray(batchModel) ? batchModel : batchModel?.cases ?? [];
  const {
    status = "all",
    query = "",
    column = null,
    metricField = "max_abs",
    direction = column ? "desc" : "asc",
    sortBy = null,
    threshold = null,
  } = options ?? {};

  if (column && !VALID_METRIC_FIELDS.has(metricField)) {
    throw new Error(`Invalid metricField ${JSON.stringify(metricField)}`);
  }
  if (direction !== "asc" && direction !== "desc") {
    throw new Error(`Invalid sort direction ${JSON.stringify(direction)}`);
  }

  const lowerQuery = typeof query === "string" ? query.trim().toLowerCase() : "";
  const thresholds = Array.isArray(threshold) ? threshold : threshold ? [threshold] : [];

  const filtered = [];
  for (let idx = 0; idx < source.length; idx += 1) {
    const c = source[idx];
    if (status && status !== "all" && c.status !== status) continue;
    if (!matchesQuery(c, lowerQuery)) continue;

    const metric = column ? (c.metrics?.get(column) ?? null) : null;
    const rawSortVal = metric ? metric[metricField] : null;
    const sortValue = typeof rawSortVal === "number" && Number.isFinite(rawSortVal) ? rawSortVal : null;
    const violations = evaluateThresholds(c, thresholds);

    filtered.push({
      ...c,
      metric,
      sortValue,
      exceeded: violations.length > 0,
      violations,
      _order: idx,
    });
  }

  const dir = direction === "desc" ? -1 : 1;
  if (column) {
    filtered.sort((a, b) => {
      const av = a.sortValue;
      const bv = b.sortValue;
      if (av === null && bv === null) return a._order - b._order;
      if (av === null) return 1;
      if (bv === null) return -1;
      if (av !== bv) return (av - bv) * dir;
      return a._order - b._order;
    });
  } else if (sortBy === "name") {
    filtered.sort((a, b) => a.name.localeCompare(b.name) * dir || a._order - b._order);
  } else if (sortBy === "status") {
    filtered.sort((a, b) => a.status.localeCompare(b.status) * dir || a._order - b._order);
  } else if (sortBy === "elapsed_s" || sortBy === "warnings") {
    filtered.sort((a, b) => {
      const av = typeof a[sortBy] === "number" && Number.isFinite(a[sortBy]) ? a[sortBy] : null;
      const bv = typeof b[sortBy] === "number" && Number.isFinite(b[sortBy]) ? b[sortBy] : null;
      if (av === null && bv === null) return a._order - b._order;
      if (av === null) return 1;
      if (bv === null) return -1;
      if (av !== bv) return (av - bv) * dir;
      return a._order - b._order;
    });
  }

  return filtered.map(({ _order, ...rest }) => rest);
}

/// Extracts `{ caseName, status, time, values, unit }` series for `columnName`
/// from already-loaded cases.
export function extractComparisonSeries(loadedCases, columnName) {
  if (typeof columnName !== "string" || columnName === "") {
    throw new Error("columnName must be a non-empty string");
  }
  const list = loadedCases instanceof Map ? Array.from(loadedCases.values()) : Array.from(loadedCases ?? []);
  const series = [];

  for (const lc of list) {
    if (!lc || !lc.result || !Array.isArray(lc.result.header)) continue;
    const timeColName = lc.summary?.result?.time_column || "time_s";
    const timeIdx = lc.result.header.indexOf(timeColName);
    const colIdx = lc.result.header.indexOf(columnName);
    if (timeIdx < 0 || colIdx < 0) continue;

    let unit = lc.units instanceof Map ? lc.units.get(columnName) : undefined;
    if (unit === undefined) {
      const colMeta = lc.summary?.result?.columns?.find((c) => c && c.name === columnName);
      unit = typeof colMeta?.unit === "string" ? colMeta.unit : "";
    }

    series.push({
      caseName: lc.caseName ?? lc.name,
      status: lc.status ?? lc.summary?.status ?? "unknown",
      time: lc.result.columns[timeIdx],
      values: lc.result.columns[colIdx],
      unit: unit ?? "",
    });
  }

  return series;
}
