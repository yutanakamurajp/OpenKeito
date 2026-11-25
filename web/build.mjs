// Assembles the browser demo into one directory of static files.
//
//   node web/build.mjs [--pkg <dir>] [--out <dir>] [--no-check]
//
// --pkg       the output of wasm-bindgen (default: crates/openkeito-wasm/pkg)
// --out       the directory to write (default: web/dist; it is emptied first)
// --no-check  do not run the examples
//
// The page has no build step of its own: its files are copied as they are.
// What this script adds is what the page must not keep copies of: the
// WebAssembly module and the example models of `examples/`.
//
// Unless --no-check is given, every example of web/examples.json is then run
// from the module (in Node), and the script fails when one does not complete,
// when a column the list names is not in its result, when a column of its
// result has no place in the diagram, or when the list and `examples/` have
// drifted apart. Exit code 0: the directory is ready.
// Needs Node 18 or later and nothing else.

import { cpSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync, statSync } from "node:fs";
import { dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { gzipSync } from "node:zlib";

const web = dirname(fileURLToPath(import.meta.url));
const root = resolve(web, "..");
const args = process.argv.slice(2);

function fail(message) {
  console.error(`build: ${message}`);
  process.exit(1);
}

function option(name, fallback) {
  const i = args.indexOf(name);
  if (i < 0) return fallback;
  const value = args[i + 1];
  if (value === undefined || value === "" || value.startsWith("--")) {
    fail(`${name} needs a directory`);
  }
  return resolve(value);
}
const pkg = option("--pkg", join(root, "crates/openkeito-wasm/pkg"));
const out = option("--out", join(web, "dist"));
const check = !args.includes("--no-check");

/// The files of the page, relative to web/. Everything else in web/ (this
/// script, the server, the README) stays out of the published directory.
const PAGE = ["index.html", "404.html", "style.css", "favicon.svg", "config.js", "examples.json", "_headers", "src"];

/// The files of the module the page loads (the TypeScript declarations stay).
const MODULE = ["openkeito_wasm.js", "openkeito_wasm_bg.wasm"];

for (const file of MODULE) {
  if (!existsSync(join(pkg, file))) {
    fail(`${join(pkg, file)} does not exist. Build the module first (web/README.md).`);
  }
}
const relToRoot = relative(root, out);
const insideRoot = !isAbsolute(relToRoot) && relToRoot !== ".." && !relToRoot.startsWith(`..${sep}`);
const isWebDist = relative(join(web, "dist"), out) === "";
if (insideRoot && !isWebDist) {
  fail(`--out ${out} is inside the repository; use web/dist or a directory outside`);
}
if (existsSync(out)) {
  if (!statSync(out).isDirectory()) {
    fail(`--out ${out} exists and is not a directory`);
  }
  const entries = readdirSync(out);
  const previousBuild = existsSync(join(out, "index.html")) && existsSync(join(out, "pkg", "openkeito_wasm.js"));
  if (entries.length > 0 && !previousBuild) {
    fail(`--out ${out} is not empty and does not contain a previous build`);
  }
}
const { config } = await import(pathToFileURL(join(web, "config.js")).href);
if (config.repositoryUrl && !/^https:\/\//.test(config.repositoryUrl)) {
  fail(`config.repositoryUrl must start with https://`);
}

// The list of examples against examples/.
const manifest = JSON.parse(readFileSync(join(web, "examples.json"), "utf8"));
const listed = manifest.examples.map((e) => e.file);
const excluded = (manifest.excluded ?? []).map((e) => e.file);
const present = readdirSync(join(root, "examples")).filter((f) => f.endsWith(".json")).sort();
const problems = [];
for (const file of [...listed, ...excluded]) {
  if (!present.includes(file)) problems.push(`examples.json names ${file}, which is not in examples/`);
}
for (const file of present) {
  if (!listed.includes(file) && !excluded.includes(file)) {
    problems.push(`examples/${file} is neither in "examples" nor in "excluded" (with a reason) of web/examples.json`);
  }
}
if (new Set(listed).size !== listed.length) problems.push("examples.json lists a file twice");
const groups = new Set(manifest.groups.map((g) => g.id));
for (const e of manifest.examples) {
  if (!groups.has(e.group)) problems.push(`${e.file}: unknown group ${e.group}`);
  if (!e.title || !e.description) problems.push(`${e.file}: title and description are required`);
}
if (!listed.includes(manifest.default)) problems.push(`the default ${manifest.default} is not listed`);
if (problems.length > 0) fail(problems.join("\n       "));

rmSync(out, { recursive: true, force: true });
mkdirSync(join(out, "pkg"), { recursive: true });
mkdirSync(join(out, "examples"), { recursive: true });
for (const item of PAGE) cpSync(join(web, item), join(out, item), { recursive: true });
for (const file of MODULE) cpSync(join(pkg, file), join(out, "pkg", file));
for (const file of listed) cpSync(join(root, "examples", file), join(out, "examples", file));

// Run the examples from the assembled directory, as the page will.
if (check) {
  const wasm = await import(pathToFileURL(join(out, "pkg", "openkeito_wasm.js")).href);
  wasm.initSync({ module: readFileSync(join(out, "pkg", "openkeito_wasm_bg.wasm")) });
  // The diagram: a symbol for every type of the catalog, and every example
  // drawn as its topology says, without a wire through a symbol.
  const { circuitFace, controlsFace, place, compareWithTopology, wiresThroughSymbols } = await import(
    pathToFileURL(join(out, "src", "layout.js")).href
  );
  const { typesWithoutSymbol } = await import(pathToFileURL(join(out, "src", "symbols.js")).href);
  const { bindColumns } = await import(pathToFileURL(join(out, "src", "overlay.js")).href);
  const bare = typesWithoutSymbol(JSON.parse(wasm.components()));
  if (bare.length > 0) problems.push(`no symbol in web/src/symbols.js for: ${bare.join(", ")}`);
  for (const e of manifest.examples) {
    const topology = JSON.parse(wasm.topology(readFileSync(join(out, "examples", e.file), "utf8")));
    const faces = [circuitFace(topology), controlsFace(topology)];
    const found = [...compareWithTopology(topology, ...faces), ...faces.flatMap((face) => wiresThroughSymbols(place(face)))];
    if (found.length > 0) problems.push(`${e.file}: the diagram: ${found.slice(0, 3).join("; ")}`);
  }
  console.log(`openkeito ${wasm.version()} (WebAssembly, in Node ${process.versions.node})`);
  console.log("example                          steps    rows  run (ms)  scan (ms)");
  for (const e of manifest.examples) {
    const text = readFileSync(join(out, "examples", e.file), "utf8");
    const started = performance.now();
    const output = wasm.run(text, e.file);
    const runMs = performance.now() - started;
    const summary = JSON.parse(output.summary);
    const header = (output.file("result.csv") ?? "").split("\n", 1)[0].split(",");
    output.free();
    if (summary.status !== "completed") {
      problems.push(`${e.file}: status ${summary.status}: ${JSON.stringify(summary.errors)}`);
      continue;
    }
    for (const column of e.columns ?? []) {
      if (!header.includes(column)) problems.push(`${e.file}: no column ${column} (there are: ${header.slice(1).join(", ")})`);
    }
    // Every column of the result is shown somewhere in the diagram.
    const lost = bindColumns(JSON.parse(wasm.topology(text)), summary.result.columns, header).unbound;
    if (lost.length > 0) problems.push(`${e.file}: no place in the diagram for the columns ${lost.join(", ")}`);
    let scanMs = "";
    if (JSON.parse(text).frequency_scan !== undefined) {
      const scanStarted = performance.now();
      const scan = wasm.scan(text, e.file);
      scanMs = (performance.now() - scanStarted).toFixed(1);
      const status = JSON.parse(scan.summary).status;
      const scanHeader = (scan.file("scan.csv") ?? "").split("\n", 1)[0].split(",");
      scan.free();
      if (status !== "completed") problems.push(`${e.file}: scan status ${status}`);
      for (const column of e.scanColumns ?? []) {
        if (!scanHeader.includes(column)) problems.push(`${e.file}: the scan has no column ${column}`);
      }
    } else if (e.scanColumns) {
      problems.push(`${e.file}: scanColumns, but the model has no frequency_scan`);
    }
    console.log(
      `${e.file.padEnd(30)} ${String(summary.solver.steps).padStart(7)} ${String(summary.result.rows).padStart(7)} ` +
        `${runMs.toFixed(1).padStart(9)} ${scanMs.padStart(10)}`,
    );
  }
  if (problems.length > 0) fail(problems.join("\n       "));
}

// What a visitor downloads.
let total = 0;
let count = 0;
function sizes(dir) {
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) sizes(path);
    else {
      total += statSync(path).size;
      count += 1;
    }
  }
}
sizes(out);
const wasmBytes = readFileSync(join(out, "pkg", "openkeito_wasm_bg.wasm"));
const mb = (n) => `${(n / 1e6).toFixed(2)} MB`;
console.log(
  `${relative(root, out)}: ${count} files, ${mb(total)}; the module ${mb(wasmBytes.length)} ` +
    `(${mb(gzipSync(wasmBytes).length)} gzip), ${listed.length} examples`,
);
