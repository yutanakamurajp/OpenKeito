# Model format

A model is one JSON document: the simulation settings, the components of the circuit, the
control blocks and the outputs. `openkeito schema` prints its JSON Schema and
`openkeito components` every component and control block with its parameters, units, sign
convention and an example; both are the reference for field names. This page describes the
conventions that the two cannot say:

- [components](#components), [results of a run](#results-of-a-run),
  [batch files](#batch-files) and the [topology](#topology) of a model;
- [conventions](#conventions) and the simulation settings;
- [three-phase circuits](#three-phase-circuits),
  [steady-state initialization](#steady-state-initialization), [controls](#controls) with
  [controlled sources](#controlled-sources) and the
  [grid-following converter](#grid-following-converter), the
  [frequency scan](#frequency-scan), [nonlinear elements](#nonlinear-elements) and
  [synchronous machines](#synchronous-machines);
- [diagnostics](#diagnostics).

The methods behind them are in [numerical-method.md](numerical-method.md).

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

The top level has `format` (`"openkeito-model"`), `version` (`"0.8"` is the current version;
`"0.1"` to `"0.7"` are read as before), the free texts `name` and
`description`, `simulation`, `components`, `controls`, `outputs` and, for `openkeito scan`,
`frequency_scan`. A field that the schema does not have is an error.

## Components

| Type | What it is |
|---|---|
| `resistor`, `inductor`, `capacitor` | linear R, L and C between two nodes |
| `voltage_source`, `current_source` | ideal sources with a waveform (`dc`, `sine`, `step`) |
| `switch` | ideal switch with closing and opening times; opens at a current zero by default |
| `diode`, `controlled_switch` | resistors with two values; the diode by its voltage and current, the switch by a gate signal |
| `nonlinear_resistor`, `nonlinear_inductor` | piecewise-linear characteristics: surge arresters, saturation |
| `transformer` | single-phase two-winding transformer, with optional saturation |
| `controlled_voltage_source`, `controlled_current_source` | sources whose value is a control signal |
| `three_phase_source` | balanced voltage source behind a coupled impedance |
| `three_phase_current_source` | balanced current injection |
| `three_phase_load` | constant impedance in star for a given P and Q |
| `coupled_rl` | three coupled series branches |
| `pi_line` | line as cascaded pi sections |
| `bergeron_line` | line with distributed constant parameters (travelling waves) |
| `three_phase_transformer` | bank of three single-phase units with a vector group, with optional saturation |
| `three_phase_ideal_transformer` | ideal ratio and symmetric phase shift |
| `fault` | fault between phases and to ground, cleared at current zeros |
| `three_phase_controlled_source`, `three_phase_controlled_current_source` | three-phase sources whose values are control signals |
| `synchronous_machine` | Park's model with rotor windings and the swing equation |
| `grid_following_converter` | averaged inverter with its phase-locked loop, current control and limits |

The control blocks are listed under [Controls](#controls). `openkeito components` is the
complete list (26 components and 32 control blocks in format version `"0.8"`).

## Results of a run

`openkeito run <model> --out <dir>` writes into the output directory:

| File | Content |
|---|---|
| `result.csv` | `time_s` and one column per output |
| `events.csv` | every switching action: time, component, `close`/`open`/`step`, interrupted current |
| `phasors.csv` | with steady-state initialization: the phasors of every node voltage and recorded quantity |
| `summary.json` | the document that the command also prints |

- The summary has `status` (`completed` or `failed`), `errors` and `warnings`, the result
  columns (`result.columns`: for each its `name`, its `kind`, `node_voltage`,
  `branch_current`, `branch_voltage` or `signal`, its `unit` and its `target`, the node,
  component or signal it belongs to; the diagram of the browser page puts the values of a
  run at their places by these) with the smallest, largest, largest absolute and last value
  of each (`result.statistics`), the first 100 switching events with `event_count` and counts per
  component, the solver counts (factorizations, solutions, re-solutions for diode states,
  iterations) and the time spent per part (`timing`).
- Exit codes: 0 success, 1 the simulation failed, 2 the model or the request is invalid.
  Exit code 0 is necessary but not sufficient: read `status`, then the events and the
  warnings, before the CSV.
- A run that fails keeps its summary: `status` is `failed`, `errors` says why, and the
  events and solver counts up to the failure are there (a switch that opened just before a
  singular matrix is usually its cause). The rows computed before the failure stay in
  `result.csv`.
- A model or a request that is invalid (exit code 2) writes nothing into `--out`: the
  document on standard output carries the errors and, under `warnings`, the warnings found
  beside them. `errors` holds errors only, and `warnings` is always there, for
  `validate`, `run`, `scan` and `batch` alike.
- A run directory is not overwritten unless `--force` is given; with it, the files of the
  old result are removed before the run starts (other files in the directory stay).
- `openkeito scan <model> --out <dir>` writes `scan.csv` and `summary.json` in the same way
  (see [Frequency scan](#frequency-scan)).

## Batch files

`openkeito batch <batch.json> --out <dir> [--jobs N]` runs the cases of a batch file in
parallel, each valid case into `<dir>/<case>` (writing `model.json` beside `summary.json`,
`result.csv`, `events.csv`, and with steady-state initialization `phasors.csv`; if writing
`model.json` fails, warning W300 is added), and writes `batch-summary.json` (status and errors
of every case) and `batch-metrics.csv` (min, max, max abs and last value of every recorded
column of every case). Invalid cases do not create a case directory. `openkeito schema --batch`
prints the schema.

```json
{"format": "openkeito-batch", "version": "0.1", "name": "faults",
 "cases": [{"name": "f1", "model": "f1.json"}, {"name": "f2", "model": {"format": "openkeito-model", "...": "..."}}]}
```

A model is a path (relative to the batch file) or written out. Every case is independent,
so a parallel batch gives the same results, bit for bit, as `--jobs 1`; an invalid case is
reported and does not stop the others (a mistake in a written-out model is a diagnostic of
its case). A case name is the name of its directory: letters, digits, `_`, `-` and `.`, not
ending with `.`, and unique also when upper and lower case are not told apart (E402). With
`--force`, the directories of cases that the new batch does not have are left as they are;
they are not in the new summary. The `sha256` of a written-out model is that of its JSON
text as the tool writes it again, not of its lines in the batch file. Cases that run at the
same time use one thread each.

## Topology

`openkeito topology <model> [--out <dir>]` prints the connections of a model as one JSON
document: which component is at which node, which nodes are the phases of a bus, which
control block reads which signal and what in the circuit a signal acts on. It is what the
diagram of the browser page is drawn from, and it answers "what is connected to `b2`?"
without following the names through the model. With `--out`, the document is also written
into that directory as `topology.json` (a file of that name that is already there is refused
unless `--force` is given). The WebAssembly module (`topology(text)`) and the Python package
(`openkeito.topology(model)`) return the same document: it depends on the text of the model
only, and has neither a path nor a time in it.

```json
{"tool": "openkeito", "version": "0.8.0", "command": "topology", "valid": true, "errors": [], "warnings": [],
 "model": {"name": "buck converter, open loop", "sha256": "bc54cc42..."},
 "components": [
   {"name": "S1", "type": "controlled_switch", "path": "components[1]",
    "terminals": [{"name": "nodes[0]", "path": "components[1].nodes[0]", "nodes": ["n1"]},
                  {"name": "nodes[1]", "path": "components[1].nodes[1]", "nodes": ["sw"]}],
    "signals": [{"field": "gate", "path": "components[1].gate", "signal": "pwm"}]}],
 "nodes": [{"name": "gnd", "ground": true, "components": ["E1", "D1", "C1", "R1"]},
           {"name": "sw", "components": ["S1", "D1", "L1"]}],
 "buses": [],
 "controls": [
   {"name": "pwm", "type": "comparator", "path": "controls[2]", "outputs": ["pwm"],
    "inputs": [{"field": "inputs[0]", "path": "controls[2].inputs[0]", "signal": "duty"},
               {"field": "inputs[1]", "path": "controls[2].inputs[1]", "signal": "carrier"}]}],
 "signals": [{"name": "pwm", "from": "pwm", "to": [{"component": "S1", "field": "gate"}, {"output": "outputs[4]"}]}],
 "diagnostics": []}
```

(of `examples/buck.json`, with only some of its entries.)

| Field | Content |
|---|---|
| `valid`, `errors`, `warnings` | as in the document of `validate` |
| `model` | `name` and the `sha256` of the text |
| `components` | every component in the order of the model: `name`, `type`, `path` (its place in the model, written as the path of a diagnostic), `terminals`, and where it has them `signals` (the control signals it reads, each with the `field` it is written in) and `offers` (the internal quantities that a `component_signal` measurement can read) |
| `terminals` of a component | `name` is the field that names the terminal (`nodes[0]`, `bus`, `buses[1]`, `windings[0][1]`, `neutral`, `hv_neutral`), `path` its place in the model, `nodes` one node or one per phase. A three-phase terminal has `phases` (`["a", "b", "c"]`, or the faulted phases of a `fault`) and, when it is at a bus, `bus` |
| `nodes` | every node in the order of first use: `name`, `ground` (true for `gnd`), `bus` and `phase` for a phase node of a bus, and `components`, the components with a terminal there |
| `buses` | every bus: `name`, `nodes` (its three phase nodes) and `components`, the components with a three-phase terminal there |
| `controls` | every control block: `name`, `type`, `path`, `inputs` (`field`, `path`, `signal`), `outputs` (its signals) and, for a measurement block, `measures` (its `quantity` as the model writes it). The blocks that a component brings with it (the control of a `grid_following_converter`) follow those of the model, each with its `owner` and the path of that component |
| `signals` | every control signal: `name`, `from` (the block that gives it; `null` for a name that no block gives) and `to`, what reads it: `{"control", "field"}`, `{"component", "field"}` or `{"output": "outputs[3]"}` |
| `diagnostics` | for every error and warning, errors first, in the order of `errors` and `warnings`: `severity`, `code`, `path` and the `components`, `controls`, `nodes` and `buses` it is about, by their names. A diagram marks these |

- A model that can be read has its connections also when it is not valid: `valid` is false,
  the exit code is 2, and `diagnostics` says where the errors are. A text that cannot be read
  (E001), a batch file and a schedule file have only `errors`.
- A terminal at gnd that the model leaves at its default is listed with the path it would
  have: `neutral` of a source or a load, the star point of a `YN` or `yn` winding
  (`hv_neutral`, `lv_neutral`), the star point of a `synchronous_machine` (`neutral`), the
  fault point of a `fault` with `"ground": true` (`ground`). What a component connects to
  gnd inside without a terminal (the capacitances of a line) is not listed.
- Three nodes written out for a terminal (`["b1.a", "b1.b", "b1.c"]`) are the bus `b1` when
  they are its phase nodes in their order; otherwise the terminal has no `bus`, and each node
  stands for itself (a star point `["n", "n", "n"]`).
- What a diagnostic is about follows from its path (`components[2].nodes[1]`: the component
  and that node; `outputs[0]`: what the output records) and from what the diagnostic says
  itself: E141 names the nodes that are not connected to gnd, E142 and W142 the components
  of a loop of ideal branches, E143 and W143 the nodes whose voltage nothing holds and the
  components at them.
- The terminals are those of the circuit that is solved: a test compares them, for every
  example and for networks converted from pandapower and PowSyBl, with the nodes the solver
  asks for when it builds the circuit, and the order of two terminals with the sign of the
  branch voltage `U(<name>)`.
- Optional diagram layout overrides use a separate sidecar JSON file (`"format": "openkeito-layout"`,
  `"version": "0.1"`, with per-face `rails` `dx` and `items` `dx`/`dy` offsets), leaving the
  model JSON (`"0.8"`), `openkeito schema` and `openkeito topology` unchanged. Unlisted
  components fall back to the deterministic automatic layout.

## Conventions

- The reference node is `gnd`.
- `nodes` is `[from, to]`. A branch current (`I(name)`) is positive from `nodes[0]` to
  `nodes[1]` through the component; a branch voltage (`U(name)`) is
  `V(nodes[0]) - V(nodes[1])`. A voltage source that delivers power therefore has a
  negative branch current.
- A sine waveform is `offset + amplitude * sin(2*pi*frequency_hz*t + phase_deg)`;
  `amplitude` is the peak value.
- The row at time `t` is the solution at `t`. The first row is `t = 0+`.
- A switching action takes effect at the first grid point at or after its time, right
  after the output of that grid point. With `open_at_current_zero` (default) a switch
  opens at the first grid point after the command at which its current has crossed zero.
  A direct current has no zero: a switch (or a fault) still waiting at the end of the run
  is reported as warning W221.
- A `step` waveform acts like a switch: the source takes its `after` value at the first
  grid point at or after `time_s`, right after the output of that grid point (the row at
  that time still shows `before`), and the change is listed in the events as `step`.
- `diode` (nodes `[anode, cathode]`) and `controlled_switch` are resistors with two values,
  `on_resistance_ohm` (default 1e-3) and `off_resistance_ohm` (default 1e6). A diode whose
  current has crossed zero, or whose voltage has become positive, changes its state at the
  next grid point, right after its output; the diode states are then settled together
  (see [numerical-method.md](numerical-method.md#diode-states)).
- Choosing the two resistances: the defaults are the order of one device at its rated
  current (a press-pack thyristor or diode, an IGBT module). From a data sheet,
  `on_resistance_ohm` = n·(V_T0 + r_T·I)/I for n devices in series with threshold voltage
  V_T0 and slope resistance r_T, at the current I the element carries, and
  `off_resistance_ohm` = rated voltage / leakage current, or the grading resistance across
  a series connection. The conduction loss is R_on·I² and the leak loss V²/R_off. A resistor
  with two values has no threshold voltage, no switching loss and no snubber; add a source
  or an RC branch where they matter. A `diode` or `controlled_switch` that the model leaves
  at a default and that blocks more than 10 kV (more than one device does) is listed in
  warning W271 with its blocked voltage, V²/R_off and R_on·I; writing the values in the
  model, also the default ones, is taken as a decision and gives no warning.
- `controls` is a list of control blocks (see [Controls](#controls)); the name of a block
  is the name of its output signal, and names are unique across components and blocks. A
  `controlled_switch` is on while its `gate` signal is >= 0.5. Blocks are evaluated at every
  grid point on the circuit solution there; their outputs act on the circuit over the next
  step. A signal output `{"type": "signal", "signal": "pwm"}` gives the column `S(pwm)`.
- PWM edges fall on grid points: the duty realized differs from the reference by less than
  `time_step_s × frequency`. Choose a duty or carrier whose crossings do not fall exactly
  on grid points (D = 0.4 with 100 steps per period does), or rounding decides them.
- `simulation.switching`: `grid` (default) is what the four items above describe.
  `interpolated` places every switching instant within its time step and splits the step
  there: a `switch`, a `fault` and a `step` waveform act at their times, a diode and a
  contact that opens at a current zero at the zero crossing, a `controlled_switch` at the
  instant its gate signal crosses 0.5 (the controls are still evaluated at grid points: a
  constant reference gives exact PWM edges, a measured one is held for a step). The output
  rows stay on the grid; the times in `events.csv` and in the summary are the instants
  themselves, no longer grid points. The error of a switching instant falls from first to
  second order in `time_step_s`: a 10 kHz buck converter at 10 µs is 1000 times more
  accurate than with `grid` at 1 µs, for about 20 % more time per step. The summary counts
  the instants (`solver.instants_within_steps`). `grid` stays the default so that the events
  of existing models keep their times; **for power electronics (PWM, rectifiers), use
  `interpolated`** (`examples/buck_interpolated.json` against `examples/buck.json`). See
  [numerical-method.md](numerical-method.md#switching-instants-within-a-step)
  for the method and its limits (W270).
- `simulation.output_interval_steps` (default 1) records one row every n steps.
- `simulation.linear_solver`: `auto` (default: dense LU below 16 unknowns, sparse LU from
  there on), `dense` or `sparse`. The summary reports the solver used and the largest
  condition estimate of its factorizations.
- `simulation.partitioning` and `simulation.threads` (format version `"0.5"`): a
  `bergeron_line` has no matrix entry between its two ends, so a circuit whose areas only
  such lines join is solved area by area, each part with its own matrix.
  `partitioning` is `auto` (default: divided at these lines, where parts that a line joins
  stay together up to 48 named nodes, 16 three-phase buses), `every_line` (divided at
  every such line) or `off` (one matrix). A line whose buses another component also joins
  (a `pi_line` beside it, a transformer) does not divide. `threads` (default 1) is the
  largest number of threads that solve the parts side by side; the solver uses fewer when
  the parts are few or small, and `openkeito run --threads N` overrides the model. The
  rows, events and counts do not depend on the number of threads (to the last bit);
  divided and undivided runs agree to rounding. A switching action factorizes only the
  matrix of its part. The summary reports `solver.parts`, `solver.largest_part_unknowns`,
  `solver.links` (lines between parts) and `solver.threads` (the number used); W301 says
  when `threads` asks for more than one thread in a circuit that is one part. In
  `openkeito batch`, cases that run at the same time use one thread each. See
  [numerical-method.md](numerical-method.md#parts-of-a-circuit-that-are-solved-on-their-own)
  and the times in [performance.md](performance.md); `examples/line_areas.json`
  is three areas joined by two lines.
- `openkeito validate` rejects two kinds of circuits whose equations have no single
  solution, which a run would only find as E200. E142: a loop of ideal branches (voltage
  sources, three-phase sources without `impedance`, closed switches, faults without
  resistance, ideal transformers), in which nothing divides the current: put a small
  resistor in series or give the source an impedance. E143: nodes that only current
  sources, open switches and faults that have not started connect to the rest, so that
  nothing holds their voltage: add a resistor to gnd or across the switch. Both look at
  the state at t = 0 (a `switch` with `close_time_s` 0 is closed, a fault with
  `start_time_s` 0 is on). A state that the switches and faults reach later in the run by
  their times is checked the same way and gives the warnings W142 and W143, which name
  the component that acts and the time; a contact that opens at a current zero opens
  later than its command, so a loop that lasts only until then is not seen. A component
  with a resistance, an inductance or a capacitance holds its nodes: a `diode` and a
  `controlled_switch` by their off resistance, and a node between capacitors has the
  voltage that their charges give it.
- Format version `"0.2"` adds diodes, controlled switches, controls, signal outputs and
  `output_interval_steps`; `"0.3"` adds the three-phase components below, `bus_voltage`
  outputs, `simulation.initialization`, `simulation.linear_solver` and `frequency_scan`;
  `"0.4"` adds `three_phase_load`, `three_phase_current_source` and
  `three_phase_ideal_transformer`; `"0.5"` adds `bergeron_line`,
  the [nonlinear elements](#nonlinear-elements), `synchronous_machine`,
  `grid_following_converter`, `simulation.switching`, `simulation.partitioning` and
  `simulation.threads`, and the control blocks, measurements of three-phase quantities and
  controlled sources of [Controls](#controls); `"0.6"` adds
  `tap_changes` on `transformer`, `three_phase_transformer` and
  `three_phase_ideal_transformer`, `impedance_changes` on `coupled_rl`, and
  `zero_sequence_inverted` on `three_phase_ideal_transformer`; `"0.7"` adds the
  `piecewise_linear` control block, signal-name limits on `integrator`, `lag`, `pi` and
  `limiter`, `anti_windup` (`"conditional"` or `"state"`) on `pi`, and
  `"initialization": "exclude"` on circuit components; `"0.8"` (the current version) adds
  the `expression` control block. Models of an older version are read as before.

## Three-phase circuits

```json
{"type": "three_phase_source", "name": "G1", "bus": "src", "line_voltage_rms_v": 66000.0, "frequency_hz": 50.0,
 "impedance": {"type": "sequence", "r1_ohm": 0.5, "l1_h": 0.0159, "r0_ohm": 1.0, "l0_h": 0.0477}},
{"type": "pi_line", "name": "LINE1", "buses": ["src", "b2"], "length_km": 50.0, "sections": 2,
 "per_km": {"type": "sequence", "r1_ohm_per_km": 0.03, "l1_h_per_km": 0.0009, "c1_f_per_km": 1.25e-8,
            "r0_ohm_per_km": 0.2, "l0_h_per_km": 0.003, "c0_f_per_km": 6.0e-9}},
{"type": "three_phase_transformer", "name": "T1", "buses": ["b2", "b3"], "vector_group": "YNd11",
 "rated_voltage_kv": [66.0, 6.6], "rated_power_mva": 30.0, "rated_frequency_hz": 50.0,
 "impedance_percent": 10.0, "resistance_percent": 0.5},
{"type": "fault", "name": "F1", "bus": "b2", "phases": ["a"], "ground": true, "start_time_s": 0.1, "clear_time_s": 0.2},
{"type": "three_phase_load", "name": "LD1", "bus": "b3", "rated_voltage_kv": 6.6, "rated_frequency_hz": 50.0,
 "active_power_mw": 5.0, "reactive_power_mvar": 1.5},
{"type": "three_phase_current_source", "name": "PV1", "bus": "b3", "current_rms_a": 400.0, "frequency_hz": 50.0,
 "angle_deg": -10.0}
```

- A bus `b2` stands for the phase nodes `b2.a`, `b2.b` and `b2.c`; `"gnd"` as a bus is gnd
  in every phase. A terminal can also be written as three node names, such as
  `["n", "n", "n"]` for a star point. Single-phase components connect to a phase by its
  node name (`"nodes": ["b2.a", "gnd"]`).
- Phasors are rms with a cosine reference: x(t) = √2·|X|·cos(ωt + ∠X). The phase sequence
  is a, b, c (b lags a by 120°). `three_phase_source` gives
  v_a = √(2/3)·V_LL·cos(ωt + angle_deg); `angle_deg` is the angle of a power-flow voltage.
- Coupled impedances are written either with positive- and zero-sequence values
  (`{"type": "sequence", ...}`: self (Z0 + 2 Z1)/3, mutual (Z0 − Z1)/3) or as 3×3 matrices
  (`{"type": "matrix", ...}`, symmetric; L and C positive definite).
- `pi_line` has `sections` cascaded pi sections, each with half its shunt capacitance at
  each end. `transformer` (single-phase) and `three_phase_transformer` (a bank of three
  single-phase units) are ideal transformers with the leakage impedance split equally
  between the windings and the magnetizing branch on the side of winding 0 (T equivalent).
  `impedance_percent`, `resistance_percent`, `no_load_current_percent` and
  `no_load_loss_kw` are u_k, u_r, i_0 and P_fe on the rating. The clock number of
  `vector_group` means that the low-voltage side lags by n·30° (Yy0/6, Yd1/5/7/11,
  Dy1/5/7/11, Dd0/6, with N/n for a star point brought out).
- `bergeron_line` is a line with distributed constant parameters, solved with travelling
  waves (format version 0.5). It has the fields of `pi_line` without `sections`:
  `{"type": "bergeron_line", "name": "LINE1", "buses": ["b1", "b2"], "length_km": 100.0, "per_km": {...}}`.
  The line is split into three modes, each a lossless line with Zc = √(L/C) and the travel
  time ℓ·√(L·C), with its resistance lumped as R/4 at each end and R/2 in the middle:
  sequence values give the zero-sequence mode and two line modes, matrices the modes of
  L·C (the resistance matrix must be nearly diagonal in them: E231, W231).
  - Every mode needs a travel time of at least `time_step_s` (E230 names the travel times
    and the largest step); a shorter line is a `pi_line`.
  - A travel time that is a whole number of time steps is reproduced without error. Any
    other one is interpolated linearly between the stored values, which is of second order
    but smooths: a component of frequency f loses up to (2πf·`time_step_s`)²/8 of its
    amplitude per travel time, and a steep front is spread over one more step each time
    it is read. Where fronts matter, choose `time_step_s` so that it divides the travel
    times.
  - The lumped resistance is accurate to about (R/Zc)²/8 (W230 when R exceeds Zc); the
    parameters do not depend on frequency.
  - The columns are those of a pi line: `I(LINE1.0.a)` … `I(LINE1.1.c)` from the bus into
    the line, `U(LINE1.a)` etc. Both ends count as connected to gnd for the check that
    every node has a reference (E141), not to each other: within a time step each end is a
    resistance to gnd and a source that the other end set at least one step earlier.
  - `examples/line_energization.json` switches a source onto an open 100 km line.
- `fault` connects the listed phases through `fault_resistance_ohm` to a fault point,
  which `ground` connects to gnd through `ground_resistance_ohm`. It starts in all phases at
  `start_time_s`; after `clear_time_s` each phase opens at its own current zero.
- Columns: `{"type": "bus_voltage", "bus": "b2"}` gives `V(b2.a)`, `V(b2.b)`, `V(b2.c)`. A
  branch current of a source or coupled RL is `I(Z1.a)` etc. (from `buses[0]` to
  `buses[1]`; for a source, from the bus into the source); a pi line or transformer has
  `I(LINE1.0.a)` … `I(LINE1.1.c)`, the current from the bus into the component at terminal
  0 or 1; a fault has `I(F1.a)` etc. from the bus into the fault and, when grounded,
  `I(F1.g)` to ground. Fault phases appear as `F1.a` etc. in `events.csv`.
- `three_phase_load` is a constant impedance in star (to `neutral`, default gnd): per phase
  R = V²/P in parallel with L = V²/(ωQ) (or C = −Q/(ωV²) for Q < 0), so that it takes
  `active_power_mw` and `reactive_power_mvar` at `rated_voltage_kv` (line to line).
  `I(LD1.a)` etc. flow from the bus into the load.
- `three_phase_current_source` injects a balanced current into the bus:
  i_a = √2·`current_rms_a`·cos(ωt + `angle_deg`), b and c lagging by 120° and 240°.
  `I(PV1.a)` etc. are the injections.
- `three_phase_ideal_transformer` (`buses`, `rated_voltage_kv` [v0, v1], `phase_shift_deg`)
  is an ideal transformer without leakage or magnetizing: V(side 1) = (v1/v0)·T·V(side 0)
  in phase voltages to ground, where T turns the positive sequence by +α, the negative
  sequence by −α and keeps the zero sequence (a symmetric phase shifter; with α = 0 an ideal
  YNyn0 bank). `I(PST1.0.a)` … `I(PST1.1.c)` flow from each bus into it.
- The windings of a transformer are not connected to each other: a delta side without any
  grounded component floats and is rejected (E141). Ground it through a large resistor.

## Steady-state initialization

`"simulation": {..., "initialization": "steady_state"}` starts the run from the
sinusoidal steady state instead of from rest. The circuit is solved once with phasors at the
frequency of the sources (every source must be a sine of that frequency without offset; a
dc source of 0 is allowed), with switches in their initial state and faults not started;
inductor currents and capacitor voltages at t = 0 are taken from it. Diodes and controlled
switches are not allowed (their steady state is not a sine). Control blocks are not part of
the phasor solution: they start from their own initial values on the circuit's steady state
(warning W183; see [Controls](#controls) for how to make them start in theirs). Actions at
t = 0 take effect after the row at t = 0 as usual.

Each circuit component also accepts `"initialization"` (`"include"`, default, or
`"exclude"`, format version `"0.7"`). When `simulation.initialization` is
`"steady_state"`, a component with `"initialization": "exclude"` is omitted from the
phasor solution (open-circuited in the nodal phasor equations), does not require a
sinusoidal waveform or linear steady state (E180, E181, E182 and W240 do not apply to it),
starts at t = 0 with zero state or its own initial fields, and enters the time-domain
circuit from the first time step (t = 0+). This is useful, for example, for energizing shunt
magnetizing branches or starting nonlinear or switching branches from rest. Excluding
components must not exclude all components, disconnect any remaining (included)
component's nodes from `gnd`, or leave any included node without a voltage-holding path in
the phasor solution (E183). Excluding a series component (such as a transformer, line, or a
branch with neither terminal at `gnd`) omits the entire element from the phasor solution and
gives warning W185: keep series transformers and lines included, and exclude only shunt
branches connected to `gnd` (for example, a separate magnetizing branch to `gnd`).

`summary.json` then has an `initialization` block: the frequency, the time up to which the
waveforms were compared with the steady state (the first switching action or the end), and
the largest departure of the node voltages and the recorded currents from it, absolute and
relative to the largest amplitude. It is of the size of the integration error,
(2πf·time_step_s)² or less; above 1e-3 a warning W220 is given. `phasors.csv` lists the
phasors (rms, angle with cosine reference, real and imaginary parts) of every node voltage
and every recorded quantity.

## Controls

```json
"controls": [
  {"type": "three_phase_measurement", "name": "v", "quantity": {"type": "bus_voltage", "bus": "b1"}},
  {"type": "pll", "name": "pll", "inputs": ["v.a", "v.b", "v.c"], "nominal_frequency_hz": 50.0,
   "rated_amplitude": 5388.9, "proportional_gain": 175.93, "integral_gain_per_s": 15791.4},
  {"type": "measurement", "name": "i_a", "quantity": {"type": "branch_current", "component": "LINE1", "terminal": 0, "phase": "a"}},
  {"type": "step", "name": "p_ref", "time_s": 0.1, "before": 0.5, "after": 0.8},
  {"type": "lag", "name": "p_filtered", "input": "p_ref", "time_constant_s": 0.02}
]
```

`openkeito components` lists every block with its parameters and an example (format
version `"0.5"` for all but the first seven, `"0.7"` for `piecewise_linear`, and `"0.8"` for
`expression`).

| Blocks | |
|---|---|
| `constant`, `gain`, `sum`, `product`, `quotient`, `function`, `limiter`, `dead_band`, `comparator`, `selector`, `piecewise_linear`, `expression` | without a state (direct feedthrough). `function` is sin, cos, tan, atan, atan2, abs, sqrt, exp, ln, sign, min, max or hypot (angles in radians); `selector` gives `inputs[0]` while `condition` >= 0.5, otherwise `inputs[1]`; `piecewise_linear` (`"0.7"`) takes `x` (strictly ascending finite breakpoints, at least one point) and `y` (finite values of the same length, E242 otherwise), interpolating linearly between adjacent `(x[k], y[k])` and holding `y[0]` for `input <= x[0]` and `y[last]` for `input >= x[last]` (a single-point table holds `y[0]` everywhere); `expression` (`"0.8"`) evaluates an algebraic expression string over its input signals (see below) |
| `step`, `ramp`, `sine_wave`, `triangle_wave`, `time_series` | functions of time. `time_series` holds each value until the next time (`"interpolation": "step"`, default) or joins them by straight lines (`"linear"`) |
| `integrator`, `lag`, `lead_lag`, `transfer_function`, `pi`, `delay`, `rate_limiter`, `initial_value` | with a state, integrated with the trapezoidal rule. `transfer_function` has `numerator` and `denominator` in descending powers of s (`[1, 2]` over `[1, 3, 2]` is (s + 2)/(s² + 3s + 2)) |
| `abc_to_dq0`, `dq0_to_abc`, `pll` | the rotating frame and the phase-locked loop |
| `dq_limiter`, `pi_dq` | a vector in the rotating frame: the limit of its magnitude with a priority (`"d"`, `"q"` or `"proportional"`), and two PI controllers with feedforward and a common limit of their output, which holds the integrals at the limit |
| `measurement`, `three_phase_measurement` | circuit quantities |

- **Signals.** A block with one output gives the signal `<name>`. `abc_to_dq0` gives
  `<name>.d`, `<name>.q` and `<name>.zero`; `dq0_to_abc` and `three_phase_measurement` give
  `<name>.a`, `<name>.b` and `<name>.c`; `pll` gives `<name>.theta` (rad, in [0, 2π)),
  `<name>.frequency_hz`, `<name>.d` and `<name>.q`. An input, a `gate` or an output names
  one of them: `{"type": "signal", "signal": "pll.theta"}` gives the column `S(pll.theta)`.
- **Loops.** Blocks may form a loop when it passes through a block whose output does not
  depend on its input at the same instant: an `integrator`, a `lag`, a `transfer_function`
  whose numerator has a lower order than its denominator, a `delay` of at least one time
  step. Such a loop is solved at every grid point (with the trapezoidal rule for the whole
  loop). Any other loop—including one through `piecewise_linear`, `expression` or through a
  signal-name limit on `integrator`, `lag`, `pi` or `limiter`, which depend on that signal at
  the same instant—is an error (E171); a loop through the circuit (a measurement of what the
  controls act on) is always possible and has one time step of delay.
- **Limits.** An `integrator` and a `lag` with `minimum` or `maximum` hold their state
  within the limits, so that it leaves a limit as soon as the input drives it back (no
  wind-up). An `integrator` with `"wrap": true` is folded back into `[minimum, maximum)`
  instead, as an angle between 0 and 2π (`wrap: true` requires numeric limits; a signal
  name is rejected with E132). A `pi` block has `"anti_windup"`: `"conditional"` (default)
  clamps its output and holds the integral whenever `Kp*input + integral + delta` would
  move further beyond an active limit; `"state"` (format version `"0.7"`) clamps the
  integral state itself into `[output_min, output_max]` (matching IEEE 421.5 non-windup
  integrator limits) and then clamps `Kp*input + integral`. In format version `"0.7"`,
  `minimum`/`maximum` on `integrator` (unless `wrap` is true), `lag` and `limiter`, and
  `output_min`/`output_max` on `pi` accept either a constant number or the name of a
  control signal evaluated at every grid point (direct feedthrough; E171 if a limit forms
  a direct loop); non-windup and conditional integration are preserved with both constant
  and dynamic limits, `min` overrides `max` if signal limits transiently invert
  `min > max`, and a non-finite limit stops the run with E251. If `pi`'s `initial_integral`
  (number or signal at t = 0) starts outside `[output_min, output_max]`, the initial
  integral state itself is clamped into `[output_min, output_max]` at t = 0 (matching
  `integrator` and `lag`, which clamp their t = 0 state into `[minimum, maximum]`),
  preventing wind-up before moving off the limit.
- **Start.** A `lag`, a `lead_lag` and a `transfer_function` start in the steady state of
  their input at t = 0, or of a constant input given as `initial_input` (0: at rest). An
  integrator (`initial_value`) and a PI block (`initial_integral`) start at a number or, with
  the name of a signal in its place, at the value of that signal at t = 0. A `pll` starts
  locked on its inputs unless `initial_angle_rad` is given. An `initial_value` block keeps
  the value its input had at t = 0: with it, a set point is "what the quantity is at the
  start", and a controller starts without an error.
- **Time functions.** `step` has its `after` value from the first grid point at or after
  `time_s` on (the row at that time shows it; the circuit sees it one step later). A `delay`
  gives input(t − delay_s), with the input joined by straight lines between grid points; a
  delay shorter than the time step depends on the present input and cannot close a loop.
- **Errors while running.** A `quotient` whose denominator is 0 stops the run (E251) unless
  `minimum_denominator` is given (a denominator of smaller magnitude is replaced by it, with
  its sign); so does a `function` outside its domain, a non-finite result from `expression`
  (such as division by zero, `sqrt` of a negative value, or `ln` <= 0) or a non-finite
  signal limit. A loop that does not settle within
  100 passes stops the run with E250: reduce `time_step_s`. The summary counts the passes
  beyond the first of each grid point (`solver.control_loop_iterations`).
- **Expression.** An `expression` block (`"0.8"`) has `name`, optional `inputs` (array of
  signal names), optional `variables` (map from identifier to signal name, for aliases or
  signal names containing `.` such as `"v.d"`), and `expression` (expression string).
  - *Variables and constants:* any signal in `inputs` whose name is a valid identifier
    (other than a built-in function name) can be referenced directly by name, and positional
    names `u0`, `u1`, ... refer to the entries of `inputs` in order. `variables` binds
    explicit identifiers to signal names. The built-in constants `pi` and `e` resolve to π
    and Euler's number only when no variable of the same name is defined via `inputs` or
    `variables` (so a signal named `"e"` takes precedence). Syntax errors, undefined
    variables or functions, wrong argument counts and conflicting variable bindings are
    reported as E243.
  - *Operators and precedence (highest to lowest):* parentheses `(...)` and function calls
    > exponentiation `^` (right-associative, and binding tighter than unary `+`/`-`, so
    `-x^2` is `-(x^2)` and `x^-2` is `x^(-2)`) > unary `+`, `-` > `*`, `/` > `+`, `-`.
  - *Functions (14, angles in radians):* single-argument `sin`, `cos`, `tan`, `atan`, `abs`,
    `sqrt`, `exp`, `ln`, `sign`; two-argument `atan2(y, x)`, `hypot(x, y)`, `pow(base, exp)`;
    variadic (one or more arguments) `min`, `max`.
  - *Evaluation:* stateless direct feedthrough (included in the E171 loop check); evaluated
    at every grid point and previewed at sub-step switching instants under
    `"switching": "interpolated"`. Non-finite outputs stop the run with E251.
- **Rotating frame.** θ is the angle of the d axis ahead of the axis of phase a, the q axis
  is 90° ahead of the d axis, and amplitudes are kept:

  ```
  d    =  2/3 (a cos θ + b cos(θ − 2π/3) + c cos(θ + 2π/3))
  q    = −2/3 (a sin θ + b sin(θ − 2π/3) + c sin(θ + 2π/3))
  zero =  1/3 (a + b + c)
  ```

  For balanced phases with the phasor X of phase a (rms, cosine reference) and
  θ = ωt + θ0: d + jq = √2·X·e^{−jθ0}. A `pll` that is locked has q = 0, θ the angle of the
  phase-a cosine and d its peak value; `proportional_gain` Kp and `integral_gain_per_s` Ki
  act on q/`rated_amplitude` and give the natural frequency √Ki rad/s and the damping
  Kp/(2√Ki).
- **Measurements.** `measurement` reads one quantity with the sign of its column:
  `node_voltage`, `branch_current`, `branch_voltage`, or `component_signal` (an internal
  quantity that a component offers, such as the flux linkage of a nonlinear inductor:
  `{"type": "component_signal", "component": "L1", "signal": "flux_wb"}`; of a transformer
  with saturation `"signal": "mag.flux_wb"`, of a three-phase one `"mag.a.flux_wb"`). A current or voltage of a component with several is
  chosen with `terminal` and `phase`, the parts of its column name: `I(LINE1.0.a)` is
  `"component": "LINE1", "terminal": 0, "phase": "a"`; `I(Z1.b)` has only `"phase": "b"`,
  `I(T1.1)` of a single-phase transformer only `"terminal": 1`, the current of a fault to
  ground is `"phase": "g"`. `three_phase_measurement` reads the three phases at once:
  `{"type": "bus_voltage", "bus": "b1"}` or
  `{"type": "branch_current", "component": "LINE1", "terminal": 1}`. A choice that names no
  column is an error that lists the columns of the component (E173).

### Controlled sources

```json
{"type": "controlled_voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "signal": "v_command"},
{"type": "controlled_current_source", "name": "J1", "nodes": ["gnd", "n1"], "signal": "i_command"},
{"type": "three_phase_controlled_source", "name": "VSC1", "bus": "b3", "signals": ["e.a", "e.b", "e.c"],
 "impedance": {"type": "sequence", "r1_ohm": 0.05, "l1_h": 0.005, "r0_ohm": 0.05, "l0_h": 0.005},
 "steady_state": {"line_voltage_rms_v": 6600.0, "angle_deg": 2.5}},
{"type": "three_phase_controlled_current_source", "name": "INV1", "bus": "b3", "signals": ["i.a", "i.b", "i.c"],
 "steady_state": {"current_rms_a": 200.0, "angle_deg": -15.0}}
```

- The value of a controlled source is a control signal in volt or ampere, with the signs and
  columns of the source of the same shape (`voltage_source`, `current_source`,
  `three_phase_source`, `three_phase_current_source`). The three-phase voltage source has
  its `signals` between the bus and `neutral`, behind `impedance`.
- The source follows its signal one time step late and in straight lines:
  source(t) = signal(t − `time_step_s`), with the signal joined by straight lines between
  grid points. A jump of the signal is a ramp over one step in the circuit. At t = 0 the
  source has the value of the signal at t = 0.
- With `"initialization": "steady_state"`, `steady_state` gives the phasor of the source for
  the phasor solution (rms and `angle_deg`, cosine reference; for the three-phase sources
  as `three_phase_source` and `three_phase_current_source` give theirs). Without it the
  source is 0 there (warning W240). The run stays in the steady state when the signal gives
  the same waveform one step ahead: turn a set point in the rotating frame back into phase
  quantities with the angle θ + 2π·f·`time_step_s` (`examples/held_injection.json`).

### Grid-following converter

```json
{"type": "grid_following_converter", "name": "PV1", "bus": "b3",
 "rated_power_mva": 2.0, "rated_voltage_kv": 6.6, "rated_frequency_hz": 50.0,
 "reference": {"type": "power", "active_power_mw": 1.5, "reactive_power_mvar": "q_cmd"},
 "operating_point": {"active_power_mw": 1.5, "reactive_power_mvar": 0.0, "voltage_kv": 6.53, "angle_deg": 1.6}}
```

The averaged model of an inverter of a photovoltaic plant, a battery or a wind turbine: a
three-phase voltage source behind its reactor, with the control that makes it a current
source to the grid. The switching is not represented, and the dc side is a constant voltage
that only limits the voltage of the source. The phase currents `I(PV1.a)` ... flow from the
converter into the bus, as those of a `three_phase_current_source`; the converter carries no
zero-sequence current (its reactor has 1000 times the impedance for it).

- **Per unit.** The values in per unit refer to `rated_power_mva` and `rated_voltage_kv`:
  voltages to the peak phase voltage √(2/3)·V, currents to the peak rated current, so that
  p = v_d·i_d + v_q·i_q and q = v_q·i_d − v_d·i_q. The d axis is the terminal voltage (v_q = 0
  when the phase-locked loop is locked), the q axis 90° ahead: i_d is the active current, and
  a positive i_q absorbs reactive power. Powers are those injected into the bus.
- **Control.** A phase-locked loop on the terminal voltage (`pll_natural_frequency_hz`,
  default 20 Hz; `pll_damping`, default 0.707); the current references from the
  powers, i_d = P/v and i_q = −Q/v, with v the d voltage behind a lag
  (`voltage_filter_time_constant_s`, default 20 ms) and at least `minimum_voltage_pu`
  (default 0.1); the limit `current_limit_pu` (default 1.1) of their magnitude, in which the
  current with `priority` (`"reactive"`, the default, or `"active"`) keeps its value and the
  other one gets √(limit² − it²); and a PI controller on each axis with the terminal
  voltage fed forward and the voltage across the reactance compensated:

  ```
  e_d = v_d − x·i_q + Kp·(i_d,ref − i_d) + Ki·∫(i_d,ref − i_d) dt
  e_q = v_q + x·i_d + Kp·(i_q,ref − i_q) + Ki·∫(i_q,ref − i_q) dt
  Kp = L/τ,  Ki = R/τ      (τ = current_time_constant_s, default 1 ms)
  ```

  The zero of the controller cancels the pole of the reactor (`reactor_reactance_pu`, default
  0.15; `reactor_resistance_pu`, default 0.005), so each current follows its reference as
  1/(τs + 1) and the axes do not act on each other. The magnitude of (e_d, e_q) is limited
  to `modulation_limit`·`dc_voltage_kv`/2 (1.2 per unit when `dc_voltage_kv` is omitted;
  `modulation_limit` is 1, or 1.155 with third-harmonic injection), and the integrals are
  held while the limit acts.
- **References.** `{"type": "power", "active_power_mw": .., "reactive_power_mvar": ..}` or
  `{"type": "current", "d_pu": .., "q_pu": ..}`; each value is a number or the name of a
  signal. With signals, an outer loop is a few control blocks: a voltage control, a frequency
  droop, reactive current during a voltage dip (`examples/converter_voltage_support.json`).
- **Signals.** The control is a set of ordinary control blocks named `PV1.<...>`, which
  outputs and the blocks of the model can read (they are recorded only when listed in
  `outputs`). In per unit: `PV1.v_d`, `PV1.v_q`, `PV1.i_d`, `PV1.i_q`, `PV1.p`, `PV1.q`
  (measured); `PV1.p_ref`, `PV1.q_ref`, `PV1.v_filtered`, `PV1.i_d_cmd`, `PV1.i_q_cmd`
  (before the limit), `PV1.i_ref.d`, `PV1.i_ref.q` (after it); `PV1.e.d`, `PV1.e.q` (the
  voltage of the source). Also `PV1.pll.theta` (rad), `PV1.pll.frequency_hz`, the terminal
  voltages `PV1.v_a` ... in volt, the currents `PV1.i_abc.a` ... in ampere and the phase
  voltages of the source `PV1.e_abc.a` ... in volt. A block of the model must not have a name
  that starts with `PV1.` (E290).
- **Time step.** The controls reach the circuit one time step late. The converter turns its
  voltage back into phase quantities with the angle of the next grid point, so a steady state
  is not disturbed; what remains is a dead time of one step in the current loop,
  τs + e^{−s·h} = 0. Keep `time_step_s` below 0.2·τ: the response then stays within
  0.74·(h/τ) of the step of the first-order lag and has no overshoot. Above 0.37·τ it
  overshoots (warning W290), and from 1.57·τ on the loop is unstable. A step of the d current
  moves the q current by about 0.4·ω·h of the step (0.6 % at 50 µs).
- **Limits and slow modes.** A step that asks for more voltage than the limit rises with the
  slope the remaining voltage allows, (limit − v)·ω/x per unit per second, and arrives
  without overshoot. After any change of the operating point the integrals take up the
  resistive voltage with L/R (95 ms for the defaults): a current error of at most
  R·Δi/Kp = 1 % of the change decays with that time constant.
- **Steady-state initialization.** In the phasor solution the converter is a current source:
  it injects conj(S/(3V)) for the powers and the bus voltage (`voltage_kv`, `angle_deg`) of
  `operating_point`, the values a power flow gives. The voltage of the source, the angle of
  the phase-locked loop and the integrals then follow from the solution at t = 0, and the run
  stays at the operating point (within 5e-5 per unit of the powers at 50 µs, without drift)
  when the references are the powers of the operating point. References that differ are
  followed from t = 0 on (warning W292 when both are numbers; `summary.json` then reports the
  departure from the phasor solution, W220). Without `operating_point` the references are
  taken at the rated voltage and the angle 0; when they are signals, the converter injects
  nothing in the phasor solution (W291). In a frequency scan the converter is an open circuit.
- **Cost.** One converter is 38 control blocks and 6 unknowns: 0.3 to 0.4 µs per time step
  (a `three_phase_current_source` in its place: 0.1 µs).

## Frequency scan

```json
"frequency_scan": {"start_hz": 10, "stop_hz": 2500, "points": 250, "spacing": "log",
                   "nodes": ["b2.a", "b3.a"], "buses": ["b2"]}
```

`openkeito scan model.json --out dir` computes, at each frequency, the impedances of the
network with every source set to zero (voltage sources shorted, current sources open,
three-phase sources reduced to their internal impedance), switches in their initial state,
faults not started and diodes and controlled switches off. For `nodes` it gives the self
impedance of each node (`Zmag(n)` in ohm, `Zdeg(n)` in degrees) and the mutual impedance of
each pair (`Zmag(n;m)`: the voltage at n per ampere into m); for `buses` the positive-,
negative- and zero-sequence impedances (`Z1mag(b)`, `Z2mag(b)`, `Z0mag(b)` and angles), the
diagonal of the bus's 3×3 phase impedance matrix transformed to symmetrical components.
Frequencies are `start_hz` to `stop_hz` with `points` values (`"spacing": "linear"` or
`"log"`), or a list `frequencies_hz`. A frequency at which the equations are singular gets
an empty row and warning W193.

## Nonlinear elements

```json
{"type": "nonlinear_resistor", "name": "MOV1", "nodes": ["n1", "gnd"],
 "characteristic": {"type": "power_law", "reference_voltage_v": 100e3, "max_current_a": 40e3,
   "segments": [{"coefficient_a": 0.01, "exponent": 12, "from_voltage_pu": 0.6},
                {"coefficient_a": 0.0018, "exponent": 30, "from_voltage_pu": 1.1}]}},
{"type": "nonlinear_inductor", "name": "L1", "nodes": ["n2", "gnd"],
 "current_a": [1.0, 3.0, 8.0], "flux_linkage_wb": [0.1, 0.15, 0.2]},
{"type": "transformer", "name": "T1", "windings": [["p1", "gnd"], ["s1", "gnd"]],
 "rated_voltage_kv": [10.0, 1.0], "rated_power_mva": 1.0, "rated_frequency_hz": 50.0,
 "impedance_percent": 6.0, "no_load_current_percent": 0.5,
 "saturation": {"current_pu": [0.006, 0.756], "flux_pu": [1.2, 1.5]}}
```

Format version `"0.5"`. The characteristics are piecewise linear; the segment is found
within every stage of a time step (the stage is solved again until every element is in the
segment of its solution), so there is no delay of a step. The summary counts the additional
solutions (`solver.operating_point_iterations`); segment changes are not events.

- `nonlinear_resistor`: a characteristic i(v), such as a surge arrester. As a table,
  `{"type": "table", "voltage_v": [...], "current_a": [...]}`: voltages strictly increasing,
  currents not decreasing, the point 0 V, 0 A included; with `"symmetric": true` only the
  points of positive voltages are given (the origin is implied and i(−v) = −i(v)). As a
  `power_law`: segments i = `coefficient_a` · (v / `reference_voltage_v`)^`exponent`, each
  from its `from_voltage_pu`; below the first one a straight line through the origin;
  symmetric. It is converted into a table up to `max_current_a` whose voltage at any current
  is within `relative_voltage_tolerance` (default 1e-3) of the power law. Segments that do
  not meet give warning W261 with the voltage at which they would.
- `nonlinear_inductor`: points of flux linkage (Wb = V·s, peak) against current, both > 0
  and strictly increasing; the origin is implied and the characteristic is symmetric.
  `initial_flux_linkage_wb` sets the flux at t = 0. There is no hysteresis and no remanence.
- `saturation` of `transformer` and `three_phase_transformer`: the same for the magnetizing
  branch, in per unit of the peak rated values of winding 0 (of one unit of a three-phase
  bank): flux on √2·V/(2πf), current on √2·S/V. The first point lies on the unsaturated
  branch: `flux_pu[0] / current_pu[0]` must be the magnetizing reactance that
  `no_load_current_percent` and `no_load_loss_kw` give (E264 tells the value). The slope
  of the last segment is the air-core reactance. A transformer without the field is
  linear, as before.
- Beyond its last point a characteristic continues with its last segment.
- The flux linkage of a nonlinear inductor is the element signal `<name>.flux_wb`
  (`<name>.mag.flux_wb` for a transformer, `<name>.mag.a.flux_wb` etc. for a three-phase
  one). The current and the voltage are recorded as for any component with two nodes.
- Steady-state initialization and frequency scans take the segment through the origin (the
  unsaturated inductance; for a resistor with a dead band, an open circuit). When the steady
  state leaves that segment, warning W260 says by how much: start such a run with
  `"initialization": "none"`.

Examples: `examples/arrester.json`, `examples/saturable_reactor.json` (with a diode) and
`examples/transformer_inrush.json`.

Integration methods (`simulation.method`): `2s-dirk` (default; two-stage diagonally
implicit Runge-Kutta, second order, L-stable) and `trapezoidal`. See
[numerical-method.md](numerical-method.md#integration-as-one-stage-formula).

## Synchronous machines

```json
{"type": "synchronous_machine", "name": "G1", "bus": "b1",
 "rated_power_mva": 555.0, "rated_voltage_kv": 24.0, "frequency_hz": 60.0,
 "xd_pu": 1.81, "xq_pu": 1.76, "xd_transient_pu": 0.30, "xq_transient_pu": 0.65,
 "xd_subtransient_pu": 0.23, "xq_subtransient_pu": 0.25, "xl_pu": 0.15, "ra_pu": 0.003,
 "td0_transient_s": 8.0, "td0_subtransient_s": 0.03, "tq0_transient_s": 1.0, "tq0_subtransient_s": 0.07,
 "inertia_h_s": 3.5, "damping_pu": 0.0,
 "field_voltage_signal": "efd", "mechanical_torque_signal": "tm",
 "operating_point": {"active_power_mw": 450.0, "reactive_power_mvar": 150.0, "voltage_pu": 1.0, "angle_deg": 0.0}}
```

Format version `"0.5"`. Park's model in the dq axes of the rotor: the field and one damper
winding on the d axis, two damper windings on the q axis (IEEE Std 1110 model 2.2), or one
when `xq_transient_pu` and `tq0_transient_s` are omitted (model 2.1, a salient-pole
machine), with the swing equation 2H dω/dt = T_m − T_e − D(ω − 1).

- Parameters are the standard ones in per unit of `rated_power_mva` and `rated_voltage_kv`,
  unsaturated, with open-circuit time constants in seconds (from short-circuit values:
  Td0′ = Td′·Xd/Xd′, Td0″ = Td″·Xd′/Xd″). Each reactance must be smaller than the one before:
  Xd > Xd′ > Xd″ > Xl > 0 and the same on the q axis (E280); Td0′ > Td0″ > 0 (E281). Xd″ and
  Xq″ may differ. Magnetic saturation is not modelled.
- `neutral` is the star point of the stator: `{"type": "grounded"}` (default; with
  `resistance_ohm` and `inductance_h` through an impedance) or `{"type": "ungrounded"}`. The
  zero sequence has `x0_pu` (default `xl_pu`).
- Inputs: `field_voltage_signal` (Efd; 1 pu gives rated voltage at no load) and
  `mechanical_torque_signal` (T_m on the rated power, positive for a generator). The machine
  follows them one time step late, as a controlled source. Without a signal the input keeps
  its value at t = 0.
- The machine starts from `operating_point`: the powers it delivers to the bus and its
  terminal voltage, as a power flow gives them (default: no load at rated voltage). With
  `"initialization": "steady_state"` the phasor solution takes the machine as a source behind
  Ra + j(Xd″ + Xq″)/2 that gives this operating point; the rotor angle, the fluxes, the field
  voltage and the torque then follow from the solved terminal voltage and current, so the
  run starts in a steady state even when the circuit is not exactly that of the power flow.
  An unbalanced phasor solution at the terminals is a warning (W280). With `"none"` the
  machine starts with the fluxes and currents of `operating_point` at rated speed.
- Currents `I(G1.a)` etc. flow from the bus into the machine, as those of a
  `three_phase_source`. The signals `G1.<signal>`, read by a `measurement` of a
  `component_signal`, use the generator convention: `speed_pu`, `angle_rad` (the q axis
  against cos(ω0 t): the power-flow angle of the internal voltage), `electrical_torque_pu`,
  `active_power_pu`, `reactive_power_pu`, `terminal_voltage_pu`, `vd_pu`, `vq_pu`, `id_pu`,
  `iq_pu`, `field_current_pu`, `field_voltage_pu`, `mechanical_torque_pu`, and
  `initial_field_voltage_pu` and `initial_mechanical_torque_pu`, the inputs that hold the
  initial operating point.
- An excitation system and a governor are control blocks. Their references are
  `initial_value` blocks, so that the controls hold the operating point the machine starts
  from: `examples/machine_avr_governor.json` has a static exciter (a lag with gain and
  ceiling, Vref = Vt + Efd0/KA at t = 0) and a governor with droop and a turbine lag
  (Pref = Tm0), and a load step; `examples/synchronous_machine.json` is a machine against an
  infinite bus with a fault.
- To the circuit the machine is a voltage behind its subtransient reactance: a constant
  conductance, and a source that is iterated within every stage of a step until the machine
  equations and the circuit agree (speed voltages, rotor angle and the difference of Xd″ and
  Xq″ are not one step late). The run summary counts the further solutions
  (`operating_point_iterations`): about one per stage during a fault, none in a steady state.

## Diagnostics

Every error and warning has a `code`, the `path` of the place in the model
(`components[1].resistance_ohm`, or `line 14 column 4` where the text cannot be read), a
`message` and a `hint` that says what to change. What `validate` reports, `run` and `scan`
report too; what only a run can find comes from the run. Codes that start with E are
errors, with W warnings.

| Codes | Found by | About |
|---|---|---|
| E001 | reading | the text is not a model: JSON syntax, duplicate keys, an unknown field or value, a missing field, a number without its exponent letter. The hint is made from the text itself: `Write "capacitance_f" in place of "capacitance"`, `Write 4.7e-9, not 4.7-9`; `path` points at `"line X column Y"`, and `json_path` (when valid JSON syntax) at the JSON path such as `components[1]`. Syntax errors stop at the first error; duplicate keys and per-section / per-component schema errors are reported together |
| E100–E102 | validation | `format` and `version`; a model that uses what its version does not have |
| E110, E111, E113, W112, W114 | validation | `time_step_s`, `end_time_s`, the number of steps and of recorded rows |
| E120–E125 | validation | names of components, nodes and buses |
| E130–E138 | validation | values of components: not finite or out of range, orders of times and values, matrices that are not symmetric or positive definite, vector groups, phases of a fault |
| E139 | validation | a coupled branch whose resistances and inductances differ by too many orders of magnitude between the sequences to be solved, checked for every stage coefficient that the run uses |
| E140–E143, W142, W143 | validation | topology: no reference, nodes not connected to `gnd`, loops of ideal branches, nodes whose voltage nothing holds (now, or later in the run) |
| E150–E155 | validation | outputs: nodes, components and buses that do not exist, quantities that are not defined, an output listed twice |
| E160 | validation | a model without components |
| E170–E173 | validation | controls: a signal that does not exist, a loop without a state, inputs and signs, a measurement that names no column |
| E180–E183, W183–W185 | validation | steady-state initialization: sources that are not sines of one frequency, components without a phasor, excluding all components or disconnecting/ungrounding the steady-state subcircuit (E183); controls start on their own (W183); no sine source (W184); series component excluded from the phasor solution (W185) |
| E190, E191, E194, W192, W193 | validation, scan | the frequency scan. W192 (diodes and controlled switches are taken as off) is reported by `validate`, `run` and `scan` alike for a model that has a `frequency_scan` and such components; W193 names the frequencies at which the network is singular |
| E200–E205 | run | the equations are singular (E200; E203 for the phasor solution), a value is not finite (E201), the diode states or the operating points did not settle (E202, E205), a component cannot be solved (E204) |
| E210 | run | writing the result failed |
| W220, W221 | run | the waveforms left the steady state (W220); a switch still waits for a current zero at the end (W221) |
| E230, E231, W230, W231 | validation | `bergeron_line`: travel time below the time step, resistance that couples the modes or exceeds the characteristic impedance |
| E240–E244, W240 | validation | control blocks and controlled sources: names of signals, transfer functions, time series, lookup tables (`piecewise_linear`), `expression` blocks (E243: syntax errors, undefined variables or functions, wrong argument counts, variable conflicts), signals that a component offers (E244); a controlled source without `steady_state` (W240) |
| E250, E251 | run | a loop of control blocks did not settle; a control signal is not finite |
| E260–E264, W260, W261 | validation, run | nonlinear characteristics and saturation |
| W270, W271 | run | more switching instants within a step than are placed; default diode resistances that block more than one device does |
| E280–E282, W280 | validation, run | synchronous machines: order of reactances and time constants, q-axis windings; an unbalanced start |
| E290, W290–W293 | validation | grid-following converters: names of their signals, time step against the current control, operating point and references, dc voltage |
| E300, W300 | command | the request: a file that cannot be read or written, an output directory that already holds a result (or a `topology.json`), a batch or schedule file given to `topology`; failed to write `model.json` into a batch case directory (W300) |
| E301, W301 | validation, run | `threads` |
| E400–E403 | validation | batch files |
| E500–E507 | validation | schedule files (see [python.md](python.md#schedules-and-time-slices)) |
| E600 | browser | a batch file given to the WebAssembly module, which has no files and no threads |
| E700 | transition | a synchronous machine stops and returns to service during a transition without both a governor and an excitation system (see [python.md](python.md#transitions)) |
