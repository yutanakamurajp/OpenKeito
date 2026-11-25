// Runs the example models in WebAssembly (in Node) and with the command
// `openkeito`, and compares the results.
//
// The arithmetic of the solver gives the same bits on both. The elementary
// functions (sin, cos, exp, atan2, ...) do not: WebAssembly has none, so
// Rust compiles its own (the crate libm) into the module, while the command
// calls the C library of its system, and the two round the last bit
// differently for some arguments. So:
//
// - the models of EXACT, which call no such function, must give the same
//   files byte for byte;
// - every other model must give the same rows, columns and events, with
//   numbers within TOLERANCE of the scale of their column. The script
//   prints which of them are the same byte for byte and the largest
//   difference of the others.
//
//   node crates/openkeito-wasm/tests/compare.mjs [--pkg <dir>] [--native <openkeito>]
//                                                [--examples <dir>] [--times]
//
// --pkg       the output of wasm-bindgen (default: crates/openkeito-wasm/pkg)
// --native    the command to compare with (default: target/release/openkeito)
// --examples  the models (default: examples)
// --times     also print the computing time of both per example
//
// Build both first (crates/openkeito-wasm/README.md). Exit code 0: every
// check passed.

import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { isDeepStrictEqual } from "node:util";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../../..");
const args = process.argv.slice(2);
function option(name, fallback) {
  const i = args.indexOf(name);
  return i < 0 ? fallback : resolve(args[i + 1]);
}
const pkg = option("--pkg", join(root, "crates/openkeito-wasm/pkg"));
const exe = process.platform === "win32" ? "openkeito.exe" : "openkeito";
const native = option("--native", join(root, "target/release", exe));
const examples = option("--examples", join(root, "examples"));
const printTimes = args.includes("--times");

for (const [what, path] of [["--pkg", join(pkg, "openkeito_wasm_bg.wasm")], ["--native", native]]) {
  if (!existsSync(path)) {
    console.error(`${what}: ${path} does not exist. Build it first (crates/openkeito-wasm/README.md).`);
    process.exit(2);
  }
}

// The glue of `wasm-bindgen --target web`, as a page loads it; here the
// bytes come from the file instead of a fetch.
const wasm = await import(pathToFileURL(join(pkg, "openkeito_wasm.js")).href);
wasm.initSync({ module: readFileSync(join(pkg, "openkeito_wasm_bg.wasm")) });

/// Models without elementary functions (DC sources, switches, linear
/// control): their files must be the same byte for byte on every system.
const EXACT = ["buck", "buck_interpolated", "buck_pi", "current_control", "rlc"];

/// Largest difference of a number, relative to the largest absolute value of
/// its column, in the other models. The differences start in the last bit
/// (1e-16) and grow where the circuit is sensitive (the instant of a
/// switching event moves, a control loop amplifies); what was measured over
/// the examples is in crates/openkeito-wasm/README.md.
const TOLERANCE = 1e-6;

/// The scale of a column is at least this times the largest value of its
/// file: a column that is zero but for rounding (the current into an open
/// line end: 1e-13 A beside 1e5 V) has no scale of its own.
const SMALL = 1e-6;

/// Largest relative difference of the counts of the solver that depend on
/// where an iteration stops (`operating_point_iterations`, `solves`, ...):
/// a difference in the last bit can add or save a pass.
const COUNT_TOLERANCE = 0.01;

let failures = 0;
let checks = 0;
function check(ok, message) {
  checks += 1;
  if (!ok) {
    failures += 1;
    console.error(`FAIL ${message}`);
  }
}

/// Runs the command; returns the text it prints (a JSON document).
function command(...commandArgs) {
  try {
    return execFileSync(native, commandArgs, { encoding: "utf8", maxBuffer: 1 << 28 });
  } catch (e) {
    // Exit codes 1 and 2 also print a document.
    return e.stdout;
  }
}

/// The cells of a CSV text: the header and the rows.
function table(text) {
  const lines = text.split("\n");
  if (lines.at(-1) === "") lines.pop();
  return { header: lines[0].split(","), rows: lines.slice(1).map((l) => l.split(",")) };
}

/// A column of angles in degrees: `angle_deg` of phasors.csv, `Zdeg(...)`,
/// `Z1deg(...)` of scan.csv.
const isAngle = (name) => name.endsWith("_deg") || /^Z\d?deg\(/.test(name);

/// Compares two CSV texts cell by cell. Returns `{error}` when they differ
/// in shape or in a cell that is not a number, else `{largest}`: the largest
/// difference of a number relative to the largest absolute value of its
/// column, or SMALL times that of the file if that is larger (angles:
/// relative to 360 degrees, and around the circle).
function compareCsv(ours, theirs) {
  const a = table(ours);
  const b = table(theirs);
  if (!isDeepStrictEqual(a.header, b.header)) return { error: "the headers differ" };
  if (a.rows.length !== b.rows.length) return { error: `${a.rows.length} rows instead of ${b.rows.length}` };
  const n = a.header.length;
  const scale = new Array(n).fill(0);
  const numeric = (cell) => cell !== undefined && cell !== "" && Number.isFinite(Number(cell));
  for (const rows of [a.rows, b.rows]) {
    for (const row of rows) {
      for (let k = 0; k < n; k += 1) {
        if (numeric(row[k])) scale[k] = Math.max(scale[k], Math.abs(Number(row[k])));
      }
    }
  }
  const floor = SMALL * Math.max(...scale.filter((_, k) => !isAngle(a.header[k])));
  let largest = 0;
  for (let i = 0; i < a.rows.length; i += 1) {
    for (let k = 0; k < n; k += 1) {
      const x = a.rows[i][k];
      const y = b.rows[i][k];
      if (x === y) continue;
      if (!numeric(x) || !numeric(y)) return { error: `row ${i + 1}, ${a.header[k]}: "${x}" instead of "${y}"` };
      let d = Math.abs(Number(x) - Number(y));
      if (isAngle(a.header[k])) {
        // The angle of a phasor that is zero means nothing; phasors.csv
        // also has every phasor as real and imaginary part.
        if (a.header[k] === "angle_deg") continue;
        d = Math.min(d, 360 - d) / 360;
      } else {
        d /= Math.max(scale[k], floor);
      }
      largest = Math.max(largest, d);
    }
  }
  return { largest };
}

/// Whether two JSON values are the same, with numbers within TOLERANCE of
/// the larger of the two.
function close(a, b) {
  if (typeof a === "number" && typeof b === "number") {
    return a === b || Math.abs(a - b) <= TOLERANCE * Math.max(Math.abs(a), Math.abs(b));
  }
  if (a === null || b === null || typeof a !== "object" || typeof b !== "object") return a === b;
  if (Array.isArray(a) !== Array.isArray(b)) return false;
  const ka = Object.keys(a);
  return ka.length === Object.keys(b).length && ka.every((k) => k in b && close(a[k], b[k]));
}

/// A summary without what differs by design between a run in memory and a
/// run into a directory: paths, times, and the threads (one in WebAssembly).
/// With `numbers` false, also without the numbers of the solution, which
/// the comparison of the files covers: only counts, names and texts stay.
function comparable(summary, numbers) {
  const s = structuredClone(summary);
  delete s.elapsed_s;
  delete s.timing;
  delete s.all_events;
  delete s.events_csv_path;
  if (s.model) delete s.model.path;
  if (s.result) delete s.result.csv_path;
  if (s.simulation) delete s.simulation.threads;
  if (s.solver) delete s.solver.threads;
  if (s.initialization) delete s.initialization.phasors_csv_path;
  if (!numbers) {
    if (s.result) delete s.result.statistics;
    if (s.result) delete s.result.last_time_s;
    // Of the solver only what is not a number stays (its counts are
    // compared within COUNT_TOLERANCE).
    for (const [k, v] of Object.entries(s.solver ?? {})) {
      if (typeof v === "number") delete s.solver[k];
    }
    delete s.initialization;
    delete s.singular_frequencies_hz;
    for (const e of s.events ?? []) {
      delete e.time_s;
      delete e.interrupted_current_a;
    }
  }
  return s;
}

const TIMING = ["build_s", "factorize_s", "solve_s", "output_s", "run_s"];
function timingIsThere(summary, name) {
  const t = summary.timing ?? {};
  check(
    TIMING.every((k) => typeof t[k] === "number" && t[k] >= 0) && typeof summary.elapsed_s === "number",
    `${name}: the summary has no timing: ${JSON.stringify(t)}`,
  );
  return t;
}

/// Compares the files of an output of the module with those in `dir`.
/// Returns whether all are the same byte for byte, and the largest
/// difference.
function sameFiles(name, output, dir, expected, exact) {
  check(
    isDeepStrictEqual([...output.files].sort(), [...expected].sort()),
    `${name}: files ${output.files} instead of ${expected}`,
  );
  let largest = 0;
  let identical = true;
  for (const file of expected) {
    const ours = output.file(file);
    const theirs = readFileSync(join(dir, file), "utf8");
    if (ours === theirs) {
      checks += 1;
      continue;
    }
    identical = false;
    if (ours === undefined) {
      check(false, `${name}: ${file} is missing`);
      continue;
    }
    const c = compareCsv(ours, theirs);
    if (c.error) {
      check(false, `${name}: ${file}: ${c.error}`);
    } else if (exact) {
      check(false, `${name}: ${file} must be the same byte for byte; largest difference ${c.largest.toExponential(2)}`);
    } else {
      check(c.largest <= TOLERANCE, `${name}: ${file}: largest difference ${c.largest.toExponential(2)} > ${TOLERANCE}`);
    }
    largest = Math.max(largest, c.largest ?? Infinity);
  }
  return { identical, largest };
}

/// Compares two summaries: the same as a whole for an exact model; else the
/// same without the numbers of the solution, and those within TOLERANCE.
function sameSummaries(name, ours, theirs, exact) {
  if (exact) {
    check(
      isDeepStrictEqual(comparable(ours, true), comparable(theirs, true)),
      `${name}: the summaries differ beyond paths, times and threads`,
    );
    return;
  }
  check(
    isDeepStrictEqual(comparable(ours, false), comparable(theirs, false)),
    `${name}: the summaries differ in more than paths, times, threads and the numbers of the solution`,
  );
  // The statistics, relative to the largest absolute value of the column
  // (or SMALL times the largest of all columns).
  const a = ours.result?.statistics ?? [];
  const b = theirs.result?.statistics ?? [];
  const floor = SMALL * Math.max(0, ...a.map((c) => c.max_abs ?? 0));
  const within =
    a.length === b.length &&
    a.every((c, i) =>
      ["min", "max", "max_abs", "last"].every((k) => {
        const d = Math.abs(c[k] - b[i][k]);
        return c[k] === b[i][k] || d <= TOLERANCE * Math.max(c.max_abs, floor);
      }),
    );
  check(within, `${name}: the statistics of the summaries differ by more than ${TOLERANCE}`);
  check(
    close(ours.singular_frequencies_hz, theirs.singular_frequencies_hz) &&
      close(ours.result?.last_time_s, theirs.result?.last_time_s),
    `${name}: numbers of the summaries differ by more than ${TOLERANCE}`,
  );
  const counts = Object.entries(ours.solver ?? {}).filter(([, v]) => typeof v === "number");
  const off = counts.filter(([k, v]) => {
    const w = theirs.solver?.[k];
    return typeof w !== "number" || Math.abs(v - w) > COUNT_TOLERANCE * Math.max(Math.abs(v), Math.abs(w));
  });
  check(
    off.length === 0,
    `${name}: numbers of the solver differ by more than ${COUNT_TOLERANCE}: ${off.map(([k, v]) => `${k} ${v} and ${theirs.solver?.[k]}`).join(", ")}`,
  );
}

const scratch = mkdtempSync(join(tmpdir(), "openkeito-wasm-"));
const rows = [];
let runSeconds = 0;
try {
  // The version: a stale build of either side would compare two programs.
  const nativeVersion = execFileSync(native, ["--version"], { encoding: "utf8" }).trim();
  check(nativeVersion === `openkeito ${wasm.version()}`, `versions: ${nativeVersion} and ${wasm.version()}`);

  // schema and components: the documents of the command.
  check(wasm.schema() + "\n" === command("schema"), "schema differs from `openkeito schema`");
  check(
    wasm.scheduleSchema() + "\n" === command("schema", "--schedule"),
    "scheduleSchema differs from `openkeito schema --schedule`",
  );
  check(wasm.components() + "\n" === command("components"), "components differs from `openkeito components`");

  const models = readdirSync(examples).filter((f) => f.endsWith(".json")).sort();
  check(models.length > 0, `no models in ${examples}`);
  for (const file of models) {
    const path = join(examples, file);
    const text = readFileSync(path, "utf8");
    const name = file.replace(/\.json$/, "");
    const exact = EXACT.includes(name);

    // topology: the document of the command, to the byte (it has no path
    // and no time in it).
    check(wasm.topology(text) + "\n" === command("topology", path), `${name}: topology differs from \`openkeito topology\``);

    // validate
    const report = JSON.parse(wasm.validate(text, path));
    check(
      isDeepStrictEqual(report, JSON.parse(command("validate", path))),
      `${name}: validate differs from the command`,
    );

    // run
    const out = join(scratch, name);
    const theirs = JSON.parse(command("run", path, "--out", out));
    const output = wasm.run(text);
    const ours = JSON.parse(output.summary);
    check(
      ours.status === "completed" && output.code === 0,
      `${name}: status ${ours.status}: ${JSON.stringify(ours.errors)}`,
    );
    sameSummaries(name, ours, theirs, exact);
    const expected = ["result.csv", "events.csv"];
    if (theirs.initialization?.phasors_csv_path) expected.push("phasors.csv");
    let same = sameFiles(name, output, out, expected, exact);
    const timing = timingIsThere(ours, name);
    runSeconds += timing.run_s ?? 0;
    output.free();

    // scan, for the models that have one
    if (JSON.parse(text).frequency_scan) {
      const scanOut = join(scratch, `${name}-scan`);
      const theirScan = JSON.parse(command("scan", path, "--out", scanOut));
      const scan = wasm.scan(text);
      const ourScan = JSON.parse(scan.summary);
      check(ourScan.status === "completed", `${name}: scan status ${ourScan.status}`);
      sameSummaries(`${name} (scan)`, ourScan, theirScan, false);
      const s = sameFiles(`${name} (scan)`, scan, scanOut, ["scan.csv"], false);
      same = { identical: same.identical && s.identical, largest: Math.max(same.largest, s.largest) };
      timingIsThere(ourScan, `${name} (scan)`);
      scan.free();
    }
    rows.push({
      model: name,
      steps: ours.solver?.steps,
      unknowns: ours.solver?.unknowns,
      wasm_run_s: timing.run_s,
      native_run_s: theirs.timing?.run_s,
      ...same,
    });
  }
  for (const name of EXACT) {
    check(rows.some((r) => r.model === name), `the exact model ${name} is not among the examples`);
  }

  // The sparse LU (which the examples only use from 16 unknowns, all of
  // them with sine sources) on a model without elementary functions: the
  // same bytes.
  {
    const model = JSON.parse(readFileSync(join(examples, "rlc.json"), "utf8"));
    model.version = "0.3"; // the first version with simulation.linear_solver
    model.simulation.linear_solver = "sparse";
    const path = join(scratch, "rlc_sparse.json");
    writeFileSync(path, JSON.stringify(model));
    const out = join(scratch, "rlc_sparse");
    const theirs = JSON.parse(command("run", path, "--out", out));
    const output = wasm.run(JSON.stringify(model));
    check(
      theirs.solver?.linear_solver === "sparse",
      `rlc with the sparse LU: the command used ${theirs.solver?.linear_solver}`,
    );
    sameSummaries("rlc with the sparse LU", JSON.parse(output.summary), theirs, true);
    sameFiles("rlc with the sparse LU", output, out, ["result.csv", "events.csv"], true);
    output.free();
  }

  // The clock: `performance.now()` of the host by default, so the runs
  // above took time; taken away, every time is 0; a clock of the caller is
  // used. No result depends on it.
  check(runSeconds > 0, "the timing of the runs is 0: the clock of the host was not found");
  const text = readFileSync(join(examples, "rlc.json"), "utf8");
  wasm.setClock();
  let o = wasm.run(text);
  let t = JSON.parse(o.summary);
  check(
    TIMING.every((k) => t.timing[k] === 0) && t.elapsed_s === 0,
    `without a clock the timing is not 0: ${JSON.stringify(t.timing)}`,
  );
  const withoutClock = o.file("result.csv");
  o.free();
  let ticks = 0;
  wasm.setClock(() => (ticks += 1) * 1000); // one second per reading
  o = wasm.run(text);
  t = JSON.parse(o.summary);
  check(ticks > 0 && t.timing.run_s >= 1, `the clock of the caller was not used: ${JSON.stringify(t.timing)}`);
  check(o.file("result.csv") === withoutClock, "the result depends on the clock");
  o.free();
  wasm.resetClock();
  o = wasm.run(text);
  t = JSON.parse(o.summary);
  check(t.timing.run_s > 0 && t.timing.run_s < 1, `resetClock did not bring the clock of the host back: ${JSON.stringify(t.timing)}`);
  o.free();

  // topology of a model that is not valid, and of a text that is no model.
  {
    const model = JSON.parse(readFileSync(join(examples, "rlc.json"), "utf8"));
    model.components[1].resistance_ohm = -1;
    for (const [what, bad] of [["a model that is not valid", JSON.stringify(model)], ["a text that is no model", "{"]]) {
      const path = join(scratch, "bad.json");
      writeFileSync(path, bad);
      check(wasm.topology(bad) + "\n" === command("topology", path), `topology of ${what} differs from the command`);
    }
    const marks = JSON.parse(wasm.topology(JSON.stringify(model))).diagnostics;
    check(marks?.[0]?.components?.[0] === "R1", "topology does not say which component an error is about");
  }

  // Invalid input: diagnostics with hints, no files, no exception.
  o = wasm.run("{");
  t = JSON.parse(o.summary);
  check(o.code === 2 && t.status === "failed" && o.files.length === 0, "an invalid model did not fail as it should");
  check(typeof t.errors?.[0]?.hint === "string", "the diagnostic of an invalid model has no hint");
  o.free();
  const batch = JSON.parse(wasm.validate('{"format": "openkeito-batch", "version": "0.5", "cases": []}'));
  check(batch.valid === false && batch.errors[0].code === "E600", "a batch file is not refused with E600");
} finally {
  rmSync(scratch, { recursive: true, force: true });
}

const same = rows.filter((r) => r.identical);
const others = rows.filter((r) => !r.identical);
console.log(`the same byte for byte (${same.length}): ${same.map((r) => r.model).join(", ")}`);
console.log(`within ${TOLERANCE} (${others.length}), largest difference relative to the scale of the column:`);
for (const r of others) console.log(`  ${r.model}: ${r.largest.toExponential(2)}`);
if (printTimes) {
  console.log("| Model | Steps | Unknowns | wasm run_s | native run_s | ratio |");
  console.log("|---|---|---|---|---|---|");
  for (const r of rows) {
    const ratio = r.native_run_s > 0 ? (r.wasm_run_s / r.native_run_s).toFixed(2) : "";
    console.log(
      `| ${r.model} | ${r.steps} | ${r.unknowns} | ${r.wasm_run_s.toExponential(3)} | ${r.native_run_s.toExponential(3)} | ${ratio} |`,
    );
  }
}
console.log(`${rows.length} models, ${checks} checks, ${failures} failed`);
process.exit(failures === 0 ? 0 : 1);
