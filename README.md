# OpenKeito

[![DOI](https://zenodo.org/badge/DOI/10.5281/zenodo.24523022.svg)](https://doi.org/10.5281/zenodo.24523022)

OpenKeito (Keito, 系統 = "power system" in Japanese) is a free electromagnetic transient (EMT)
simulator for power systems, running on Windows (with CI on Windows, macOS and Linux, and
Chromium and Firefox checked for the browser page) and meant to be driven by people and by AI agents alike.

- **A model is one JSON document** with a published schema; a run writes a CSV and a JSON
  summary. No GUI is needed.
- **It starts where the planning tools stop.** A network of pandapower or PowSyBl (CIM / CGMES)
  converts into a model whose steady state is the power flow; schedules from an optimal
  power flow run as time slices and as transitions from one time to the next.
- **Every component is checked against a closed-form solution**, and the tests say where
  each tolerance comes from.

Status: before 1.0. The model format and the APIs can still change between minor versions;
[CHANGELOG.md](CHANGELOG.md) (in Japanese) lists the changes. See [Limits](#limits) for what
the models do not represent and [Roadmap](#roadmap) for what comes next.

## Why

Instantaneous-value simulation of power systems is done almost entirely with closed,
GUI-centred programs. Power flow, optimal power flow and CIM / CGMES exchange already have
good open-source tools (pandapower, PyPSA, PowSyBl); the EMT step does not have one that
connects to them. OpenKeito writes that step and leaves the rest to the existing tools:

- **Text in, text out.** A model is one JSON document with a published schema. A run
  writes a CSV and a JSON summary.
- **Units in the field names** (`resistance_ohm`, `time_step_s`), unknown fields are
  errors, and a number without its exponent letter (`1.1141-6`) is an error whose hint
  gives the number as it should be written (`1.1141e-6`), not a silently different value.
- **Errors say what to change.** Every diagnostic has a code, the location in the model,
  a message and a hint; a misspelled field name gets the name that was meant.
- **No GUI needed, no operating-system dependency.** One self-contained binary, a Python
  package, a WebAssembly module and an MCP server, all on the same core.

## What it simulates

Linear and nonlinear R, L and C (saturation, surge arresters), sources, switches and
diodes, three-phase sources, coupled RL branches, pi lines and travelling-wave lines,
transformers with saturation, faults, synchronous machines, grid-following converters
(averaged) and a library of control blocks (transfer functions, limiters, delays, abc/dq0,
phase-locked loop). Around them: steady-state initialization from a phasor solution,
frequency scans, a sparse solver, switching instants within a time step, and the parallel
solution of the parts of a network that lines separate.

## Install

OpenKeito is built from source; it is not on crates.io or PyPI. You need a
[Rust toolchain](https://rustup.rs) (1.85 or later) and, for the Python package, Python
3.10 or later. Clone [the repository](https://github.com/yutanakamurajp/OpenKeito) and, in its directory:

```sh
git clone https://github.com/yutanakamurajp/OpenKeito.git
cd OpenKeito
cargo build --release                  # the command: target/release/openkeito
python -m pip install .                # the Python package (optional)
```

## First run

```sh
./target/release/openkeito validate examples/rlc.json
./target/release/openkeito run examples/rlc.json --out results/rlc-001
```

`run` writes `result.csv` (`time_s` and one column per output), `events.csv` (every
switching action) and `summary.json` into the output directory, and prints the summary.
A run is good when the summary's `status` is `"completed"`; then look at its `warnings` and
`events` before reading the CSV. Use a new `--out` directory for every run: an existing
result is not overwritten unless `--force` is given.

| Command | Output |
|---|---|
| `openkeito validate <file>` | `valid`, `errors`, `warnings`. The file is a model, a schedule or a batch file (told apart by `format`) |
| `openkeito topology <model> [--out <dir>]` | the connections of a model as JSON: components with terminals, nodes, buses, control blocks, signals and diagnostics (with `--out`, also `<dir>/topology.json`) |
| `openkeito run <model> --out <dir> [--threads N]` | the result files and the summary: status, result columns with their statistics, switching events, solver counts, time spent per part |
| `openkeito scan <model> --out <dir>` | impedances over frequency (`frequency_scan` of the model) in `scan.csv` |
| `openkeito batch <batch.json> --out <dir> [--jobs N]` | the cases of a batch file in parallel, each into `<dir>/<case>`, with `batch-summary.json` and `batch-metrics.csv` |
| `openkeito schema [--batch \| --schedule]` | JSON Schema of the model format (or of the batch or the schedule format) |
| `openkeito components` | every component and control block with parameters, units, sign conventions and an example |

Exit codes: 0 success, 1 the simulation failed, 2 the model or the request is invalid.
`openkeito --version` reports the version a binary was built from.
[docs/model.md](docs/model.md) describes the result files, batch files and the
[diagnostics](docs/model.md#diagnostics).

## A model

```json
{
  "format": "openkeito-model",
  "version": "0.1",
  "simulation": {"time_step_s": 1e-6, "end_time_s": 0.01, "method": "2s-dirk"},
  "components": [
    {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {"type": "dc", "value": 1.0}},
    {"type": "resistor", "name": "R1", "nodes": ["n1", "n2"], "resistance_ohm": 10.0},
    {"type": "inductor", "name": "L1", "nodes": ["n2", "n3"], "inductance_h": 0.01},
    {"type": "capacitor", "name": "C1", "nodes": ["n3", "gnd"], "capacitance_f": 1e-6}
  ],
  "outputs": [
    {"type": "node_voltage", "node": "n3"},
    {"type": "branch_current", "component": "L1"}
  ]
}
```

`openkeito schema` and `openkeito components` are the reference for field names; read them
instead of guessing. [docs/model.md](docs/model.md) has the conventions (signs, three-phase
buses, phasors, when a switching action takes effect), the simulation settings and a
section for each family of components. The current format version is `"0.8"` (set
`"version": "0.8"` when using 0.8 additions such as the `expression` control block,
`"0.7"` for `piecewise_linear` or signal-name limits, `"0.6"` for `tap_changes`,
`impedance_changes` or `zero_sequence_inverted`, or `"0.5"` for `bergeron_line`,
`switching`, machines and converters); models of older versions are read as before.

Switching instants lie on the time grid by default (`"switching": "grid"`). **For power
electronics, set `"switching": "interpolated"`**: every instant is then placed within its
time step, and a 10 kHz buck converter at 10 µs is more accurate than at 1 µs on the grid
(compare `examples/buck_interpolated.json` with `examples/buck.json`).

### Examples

| Model in `examples/` | Shows |
|---|---|
| `rlc.json` | series RLC step response: the smallest model |
| `rl_breaker.json` | a breaker that interrupts at a current zero |
| `three_phase_fault.json` | 66 kV source, pi line, YNd11 transformer and a ground fault, started from the steady state; has a `frequency_scan` |
| `line_energization.json` | a source switched onto an open 100 km line with travelling waves (`bergeron_line`); has a `frequency_scan` |
| `line_areas.json` | three areas joined by two travelling-wave lines, solved part by part |
| `synchronous_machine.json` | a machine against an infinite bus: terminal fault and swing |
| `machine_avr_governor.json` | an islanded generator with excitation system and governor, and a load step |
| `transition_three_times.json` | a generator, a load and a grid followed through three times of a schedule, as `openkeito.pandapower.transition(times="all")` writes it |
| `converter.json`, `converter_voltage_support.json` | the grid-following converter: a step of its power; its current limit during a fault |
| `wecc_ibr.json` | generic inverter-based resource controls (REPCA1 / REECA1 / REGCA1) built from `piecewise_linear` curves and signal-limited control blocks, with steady start and fault ride-through |
| `held_injection.json` | a controlled three-phase current source that starts in the steady state |
| `buck.json`, `buck_pi.json`, `buck_interpolated.json` | a 10 kHz buck converter: open loop, with PI voltage control, and with switching instants within the step |
| `rectifier.json` | diode bridge with capacitor input |
| `arrester.json`, `saturable_reactor.json`, `transformer_inrush.json` | nonlinear elements: a surge arrester, a saturable reactor with a diode, the inrush current of a transformer |
| `control_blocks.json`, `current_control.json`, `pll.json` | control blocks: a set point shaped by blocks, PI control of a current, a phase-locked loop |
| `expression_control.json` | mathematical expressions (`expression` control block, format `0.8`) driving a controlled current source |

## In the browser

The same solver runs in a browser as a WebAssembly module: a static page runs the examples,
draws the circuit and control diagrams (with three-phase rail expansion, voltage/current
magnitude coloring, drag-adjusted layout sidecars, and values and switching states linked to
the waveform cursor and diagnostics), lets you edit the model in `JSON` or structured `Form`
mode while validating as you type, plots waveforms and frequency scans, and loads
`openkeito batch` output folders in the `Batch` tab for filtering and case comparison.
Nothing is sent to a server. The page is not hosted publicly yet; build and open it locally
(Node 18 or later, and `wasm-bindgen`):

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.130 --locked
cargo build --release -p openkeito-wasm --target wasm32-unknown-unknown
wasm-bindgen --target web --out-dir crates/openkeito-wasm/pkg target/wasm32-unknown-unknown/release/openkeito_wasm.wasm
node web/build.mjs && node web/serve.mjs      # http://127.0.0.1:8080/
```

[web/README.md](web/README.md) describes the page and how to host it;
[crates/openkeito-wasm/README.md](crates/openkeito-wasm/README.md) the module (`validate`,
`topology`, `run`, `scan`, `schema` and `components` on JSON text, one thread, no files) and how its
results compare with those of the command.

## Python

```python
import openkeito
result = openkeito.run("examples/rlc.json")     # a dict, JSON text or a path
result.status, result.time, result["V(n3)"]     # "completed", numpy arrays

import pandapower.networks as pn
from openkeito.pandapower import from_pandapower
conv = from_pandapower(pn.case39(), end_time_s=0.2)   # runs pandapower's power flow first
result = openkeito.run(conv.model)                    # starts from that power flow
conv.compare(result)["max_abs_dv_pu"]                 # about 1e-10
```

The package runs the same core in process. It converts networks of pandapower and of
PowSyBl (CIM / CGMES, with short-circuit data and phase-angle clocks), with generators as sources
or synchronous machines and inverters as current injections or converters; runs a schedule
as time slices with disturbances in parallel; and follows a schedule from one time to the
next, or through several times in one run, with excitation systems, governors and
stabilizers on the machines. `openkeito.layout` generates `openkeito-layout` `"0.1"`
diagram sidecar JSON from bus coordinates or pandapower geodata without changing the model
format. Install the converters with
`python -m pip install ".[pandapower,powsybl]"`. See [docs/python.md](docs/python.md).

## MCP server

`openkeito-mcp` (from `python -m pip install ".[mcp]"`) is an MCP server on stdio for AI
agents: schema and catalog, validation, model topology, runs, queries of results, conversion,
schedules, time slices and transitions. Register it with a client, for example
`claude mcp add openkeito -- openkeito-mcp --root <folder>`. Its tools read and write only
under the root folder (the working directory unless `--root` or `OPENKEITO_MCP_ROOT` names
another); `--allow-any-path` lifts that limit. See [docs/mcp.md](docs/mcp.md) and
[SECURITY.md](SECURITY.md). [AGENTS.md](AGENTS.md) is the guide for agents that use or change
the tool.

## Documentation

| Document | Content |
|---|---|
| [docs/model.md](docs/model.md) | the model format: conventions, settings, components, results, batch files, diagnostics |
| [docs/python.md](docs/python.md) | the Python package: running models, conversion from pandapower and PowSyBl, schedules, time slices, transitions |
| [docs/mcp.md](docs/mcp.md) | the MCP server: tools and the root folder |
| [docs/numerical-method.md](docs/numerical-method.md) | the methods, with their references and how each is verified |
| [docs/performance.md](docs/performance.md) | measured run times |
| [docs/clean-room.md](docs/clean-room.md) | where the code comes from |
| [web/README.md](web/README.md), [crates/openkeito-wasm/README.md](crates/openkeito-wasm/README.md) | the browser page and the WebAssembly module |
| [paper/README.md](paper/README.md) | scripts that reproduce figures and tables from the tests' cases (in Japanese) |

The repository:

| Path | What it is |
|---|---|
| `crates/openkeito-model` | the model format: structures, reading, validation, catalog, JSON Schemas |
| `crates/openkeito-core` | the solver |
| `crates/openkeito-cli` | the command `openkeito` and the library behind it (run, scan, batch). Its feature `native` (on by default) holds the command and `batch`; the browser builds the library without it |
| `crates/openkeito-py` | the native module of the Python package (`openkeito._native`) |
| `crates/openkeito-wasm` | the WebAssembly module |
| `python/openkeito` | the Python package: run API, converters, schedules, transitions, MCP server |
| `web/` | the browser page |
| `examples/` | example models |
| `paper/` | figures and tables for a paper |

## Verification

`cargo test --workspace` compares simulated waveforms with closed-form solutions, hand
calculations and textbook cases, for both integration methods, and checks second-order
convergence: linear circuits and switching; a buck converter and diode bridges against
exact piecewise solutions; fault currents of all fault types against symmetrical
components; transformers, pi lines and the travelling-wave line against their equivalent
circuits and the lattice diagram; nonlinear elements against the solutions of their
piecewise-linear characteristics; the control blocks against their step responses; the
synchronous machine against Park's equations integrated separately; the averaged converter
against its design and against a switching bridge under the same control.

`python -m pytest` compares the steady state of converted networks with the power flow
they come from: pandapower's case9, case39 and case118 (bus voltages to about 1e-10 pu) and
PowSyBl's examples, among them the CGMES MicroGrid test configurations (about 1e-12 pu);
it checks that parallel and sequential runs agree bit for bit, compares transitions with
the power flow of the next time, and drives every MCP tool through the protocol.

[docs/numerical-method.md](docs/numerical-method.md#verification) lists the cases with
their numbers; measured run times are in [docs/performance.md](docs/performance.md).

## Limits

- Switching instants lie on the time grid unless `"switching": "interpolated"` is set.
  Diodes and controlled switches are resistors with two values.
- Transformer saturation has no hysteresis and no remanence, and a three-phase transformer
  is a bank of three single-phase units. Lines are pi sections or travelling-wave lines with
  constant parameters (no frequency dependence).
- Synchronous machines have no magnetic saturation. The converter is an averaged model with
  a constant dc voltage.
- Controls act on the circuit one time step late.
- With 2S-DIRK, the current of a capacitor directly across a voltage source is of first
  order only.
- The time step is fixed.

## Roadmap

Done: the circuit solver with two integration methods; power-electronics switching;
three-phase networks with steady-state initialization and frequency scans; the Python
package with conversion from pandapower and PowSyBl, schedules and the MCP server; control
blocks, machines, nonlinear elements, travelling-wave lines, converters, transitions and
the parallel solution of divided networks; the WebAssembly module, the browser page, and
the circuit and control diagrams linked with diagnostics and results.

Not decided, and open for discussion in the issues:

- Publishing the crates and the Python package on crates.io and PyPI, and hosting the
  browser page.
- Long transitions of large networks (coupling with a phasor-domain simulation, several
  time steps in one run).
- Frequency-dependent lines, saturation of machines, hysteresis, and switching models of
  converters beside the averaged one.
- Returning what an EMT run finds (overvoltages, current limits) to the optimization that
  made the schedule.

## Contributing

Contributions are welcome. [CONTRIBUTING.md](CONTRIBUTING.md) has the checks to run, the
rules for commits and the clean-room policy that every contribution must follow. Report
security problems as [SECURITY.md](SECURITY.md) says, not in a public issue.

## Citing

If you use OpenKeito in work that you publish, cite it as [CITATION.cff](CITATION.cff)
says (GitHub shows it under "Cite this repository").

## Origin of the code

OpenKeito is written from published literature only. See
[docs/clean-room.md](docs/clean-room.md).

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT) at
your option.
