// The solver, off the page's thread. A run blocks the thread it is called
// from until it is done, so the page keeps it here and, to stop a long run,
// terminates this worker and starts another.
//
// In:  { id, kind: "validate" | "topology" | "metadata" | "run" | "scan", text, label }
// Out: { type: "ready", version }
//      { type: "failed", message }                 the module did not load
//      { id, kind, report }                        validate
//      { id, kind, topology }                      topology
//      { id, kind, catalog, schema }               metadata
//      { id, kind, summary, code, wallSeconds, table, files }   run and scan
//      { id, error }                               the call threw (a bug)

import init, * as openkeito from "../pkg/openkeito_wasm.js";
import { parseNumericCsv } from "./csv.js";

// "?clock=off" takes the clock of the solver away (every time in the summary
// is then 0): the page uses it to measure what reading the clock costs.
const clockOff = new URL(self.location.href).searchParams.get("clock") === "off";

const ready = init().then(
  () => {
    if (clockOff) openkeito.setClock();
    self.postMessage({ type: "ready", version: openkeito.version() });
    return true;
  },
  (e) => {
    self.postMessage({ type: "failed", message: String(e?.message ?? e) });
    return false;
  },
);

/// The numeric file of each command, parsed here so that the page does not
/// stall on a large one.
const TABLE = { run: "result.csv", scan: "scan.csv" };

self.onmessage = async ({ data }) => {
  const { id, kind, text, label } = data;
  if (!(await ready)) {
    self.postMessage({ id, error: "The solver is not available." });
    return;
  }
  try {
    if (kind === "validate") {
      self.postMessage({ id, kind, report: JSON.parse(openkeito.validate(text, label)) });
      return;
    }
    if (kind === "topology") {
      self.postMessage({ id, kind, topology: JSON.parse(openkeito.topology(text)) });
      return;
    }
    if (kind === "metadata") {
      self.postMessage({
        id,
        kind,
        catalog: JSON.parse(openkeito.components()),
        schema: JSON.parse(openkeito.schema()),
      });
      return;
    }
    const started = performance.now();
    const output = kind === "scan" ? openkeito.scan(text, label) : openkeito.run(text, label);
    try {
      const wallSeconds = (performance.now() - started) / 1000;
      const summary = JSON.parse(output.summary);
      const files = {};
      let table = null;
      const transfer = [];
      for (const name of output.files) {
        const content = output.file(name);
        files[name] = new Blob([content], { type: "text/csv" });
        if (name === TABLE[kind]) {
          table = parseNumericCsv(content);
          for (const column of table.columns) transfer.push(column.buffer);
        }
      }
      const code = output.code;
      self.postMessage({ id, kind, summary, code, wallSeconds, table, files }, transfer);
    } finally {
      output.free();
    }
  } catch (e) {
    // A panic of the module: it wrote its message to the console before the
    // trap. The module is not usable after it; the page starts a new worker.
    self.postMessage({ id, error: String(e?.message ?? e) });
  }
};
