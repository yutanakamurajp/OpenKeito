# openkeito-wasm

OpenKeito for the browser: `validate`, `run`, `scan`, `schema` and `components` of the
`openkeito` command as a WebAssembly module (wasm32-unknown-unknown, one thread, no files).
A model goes in as JSON text; the summary and the result files come back as text.

The work is done by the library behind the command (`openkeito-cli` built without its
feature `native`, that is without argument parsing, the thread pool and `batch`), so the
module and the command cannot drift apart. The page in [`web/`](../../web/README.md) is
built on this module; the model format and the diagnostics are described in
[docs/model.md](../../docs/model.md).

## Build

The JavaScript glue is written by the command `wasm-bindgen`, whose version must be exactly
the version of the crate `wasm-bindgen` in `Cargo.lock` (now 0.2.130; the workspace pins it
with `=`). To move to a newer version, change both.

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.130 --locked

cargo build --release -p openkeito-wasm --target wasm32-unknown-unknown
wasm-bindgen --target web --out-dir crates/openkeito-wasm/pkg \
    target/wasm32-unknown-unknown/release/openkeito_wasm.wasm
```

`crates/openkeito-wasm/pkg` (not in Git) then holds `openkeito_wasm_bg.wasm`, the ES module
`openkeito_wasm.js` and its TypeScript declarations.

Optional, to make the file smaller (binaryen 132; it does not change the results and not
measurably the computing time):

```sh
wasm-opt -O3 crates/openkeito-wasm/pkg/openkeito_wasm_bg.wasm -o crates/openkeito-wasm/pkg/openkeito_wasm_bg.wasm
```

## Use

```js
import init, * as openkeito from "./pkg/openkeito_wasm.js";
await init();                      // fetches openkeito_wasm_bg.wasm next to the module
// In Node: openkeito.initSync({ module: readFileSync(".../openkeito_wasm_bg.wasm") });

const report = JSON.parse(openkeito.validate(modelText));
if (!report.valid) show(report.errors);          // each with code, path, message, hint
show(report.warnings);                           // always there, also for an invalid model

const output = openkeito.run(modelText);
const summary = JSON.parse(output.summary);      // the content of summary.json
if (summary.status === "completed") {
  plot(output.file("result.csv"));               // also "events.csv", "phasors.csv"
}
output.free();                                    // releases the memory of the texts
```

| Function | Returns |
|---|---|
| `version()` | The version of OpenKeito (`openkeito --version`) |
| `schema()`, `scheduleSchema()` | The JSON Schema of the model format and of schedule files, as JSON text |
| `components()` | The component catalog, as JSON text |
| `validate(text, label?)` | The document of `openkeito validate` as JSON text. A model or a schedule file, by its `format`; a batch file is refused with E600 (there are no files to read the models of its cases from and no threads to run them on) |
| `topology(text)` | The document of `openkeito topology` as JSON text: components with terminals and nodes, buses, control blocks and signals, and what each diagnostic is about. Also for a model that is not valid (`valid` says so) |
| `run(text, label?)` | An `Output` |
| `scan(text, label?)` | An `Output` with `scan.csv` |
| `setClock(fn?)`, `resetClock()` | See "Timing" |

`Output`: `summary` (JSON text, the summary of a run without an output directory: no paths,
and all events in `all_events`), `code` (the exit code of the command: 0, 1 or 2), `files`
(the names of the result files there are) and `file(name)` (the text of one, byte for byte
what the command writes into its output directory; `undefined` when there is none). Read
`summary.status` before the CSV, as with the command. The rows computed before a failure stay
in `result.csv`. `label` names the model in the documents (default `"<inline>"`).

Nothing throws for a bad model: it comes back as diagnostics. A panic (a bug) is written to
`console.error` before the module traps.

A run blocks the thread it is called from until it is done: call it from a Web Worker to keep
a page responsive. `simulation.threads` is ignored (one thread; the results do not depend on
the number of threads).

## Timing

wasm32-unknown-unknown has no clock, and `std::time::Instant::now()` panics there. The solver
reads `openkeito_core::clock::Stopwatch` instead: `Instant` on every other target (chosen at
compile time, no indirection), and on wasm32-unknown-unknown a function the host sets. This
module sets `performance.now()` of the host when it starts, so `timing` and `elapsed_s` of the
summaries are filled in a browser and in Node. `setClock(fn)` sets another clock (a function
that returns milliseconds), `setClock()` takes the clock away (every time is then 0) and
`resetClock()` goes back to `performance.now()`. No result depends on the clock.

A run reads the clock twice per output row. Measured in Node 24, a reading of
`performance.now()` costs 0.04 to 0.13 µs and of a function given with `setClock` 0.3 to
0.9 µs. For a model with cheap steps and one row per step this is a visible part of the run
(rectifier example: about 0.25 of 1.1 µs per step); `setClock()` or a larger
`simulation.output_interval_steps` removes it.

## Test

`tests/compare.mjs` runs every model of `examples/` in Node from this module and with the
command, and compares `validate`, the summaries, `result.csv`, `events.csv`, `phasors.csv` and
`scan.csv`, `topology` (the same text, byte for byte), and `schema` and `components`. It also checks that the timing is there, that it
is 0 without a clock, and that the results do not depend on the clock.

```sh
cargo build --release -p openkeito-cli
node crates/openkeito-wasm/tests/compare.mjs            # --times prints the computing times
```

`cargo test -p openkeito-wasm` runs the same functions on the build machine (without the
JavaScript side).

### Where the results are the same, and where not

The arithmetic of the solver (dense and sparse LU, integration, switching) gives the same
bits in WebAssembly and in the command: the examples that use nothing else (`buck`,
`buck_interpolated`, `buck_pi`, `current_control`, `rlc`, and `rlc` with the sparse LU) give
the same files byte for byte, and the test requires that. (`arrester` was also the same on
the machine of the measurement below; the test does not require it.)

The elementary functions do not. WebAssembly has no `sin`, `cos`, `exp`, `atan2`, ...; Rust
compiles its own (the crate `libm`) into the module, while the command calls the C library of
its system. Both are accurate to the last bit or so, but round differently: of 2000 random
arguments, 1 to 20 % give a result that differs in the last bit (measured against the C
library of Windows: `sin` 1.7 %, `cos` 1.3 %, `exp` 9 %, `ln` 5 %, `atan2` 19 %, `hypot` 16 %;
`sqrt` and `mul_add` never; `acos` and `asin` of `expression` are such functions too, while
`floor`, `ceil`, `trunc` and `round` are exact and give the same bits). Where an `expression`
compares the result of such a function with a bound (`sin(x) > 0.5`), a difference in the
last bit can turn the comparison the other way at the bound, and the two results then differ
by the jump; the example `expression_logic` keeps its comparisons away from that. Every model with a sine source, a machine, a transformation or a
PLL therefore differs from the last digit on, and the difference grows where the circuit is
sensitive. Measured over the examples (largest difference of a number relative to the largest
absolute value of its column, Windows 11, x86-64):

| Example | Largest difference |
|---|---|
| `control_blocks`, `rl_breaker`, `saturable_reactor`, `expression_logic` | 1e-16 to 4e-16 |
| `pll`, `converter_voltage_support`, `line_areas`, `converter`, `line_energization`, `rectifier`, `dependent_sources` | 7e-15 to 1.2e-12 |
| `transformer_inrush`, `transition_three_times`, `synchronous_machine` | 2e-12 to 5e-12 |
| `held_injection`, `machine_avr_governor` | 5e-10 to 1e-9 |
| `three_phase_fault` | 2e-9 (the scan: a transfer impedance of 1e6 ohm; the run 3e-13) |
| `ideal_transformer_vcvs_cccs` | 1.6e-8 (the current interrupted at a turn-off in `events.csv`, near zero; `result.csv` 4.9e-13) |

The rows, columns and events are the same; the times of events placed within a step differ
by the same order. The counts of iterations in the summary can differ slightly
(`machine_avr_governor`: 91842 instead of 91856 operating-point iterations). The test
requires 1e-6 for these models and 1 % for the counts. For the same reason the command
itself does not give the same last digits on Windows, Linux and macOS for these models.

### Size and computing time

Measured on Windows 11 (x86-64), Rust 1.100, Node 24.15, one core (0.7.0; with `topology`
the release build is 3.34 MB, 0.84 MB compressed, the other rows were not measured again):

| Build | `.wasm` | gzip |
|---|---|---|
| debug, after `wasm-bindgen` | 11.0 MB | 1.77 MB |
| release, after `wasm-bindgen` | 3.22 MB | 0.78 MB |
| release and `wasm-opt -O3` | 2.38 MB | 0.73 MB |
| release and `wasm-opt -Oz` | 2.33 MB | 0.73 MB |

The glue `openkeito_wasm.js` is 18 kB.

Time per step without the output (`run_s - output_s`, the smallest of 15 runs):

| Example | Unknowns | Command (µs/step) | WebAssembly | Ratio |
|---|---|---|---|---|
| `rlc` | 4 | 0.107 | 0.160 | 1.5 |
| `buck_pi` | 4 | 0.183 | 0.276 | 1.5 |
| `rectifier` | 4 | 0.205 | 0.304 | 1.5 |
| `machine_avr_governor` | 9 | 1.06 | 1.32 | 1.25 |
| `line_areas` | 25 | 1.06 | 1.51 | 1.4 |
| `three_phase_fault` | 29 | 0.89 | 1.61 | 1.8 |

Writing the rows as text into memory takes 0.7 to 0.8 of the time the command takes to write
them into a file.
