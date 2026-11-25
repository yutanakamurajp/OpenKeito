# Python package

The package `openkeito` runs the same core as the command in process (Python 3.10 or
later), converts networks of pandapower and PowSyBl into models that start from their power
flow, and runs schedules as time slices and transitions. This page describes:

- [the run API](#running-models);
- conversion [from pandapower](#from-a-power-flow-pandapower) and
  [from PowSyBl and CGMES](#from-a-load-flow-powsybl-cgmes), with
  [short-circuit data](#short-circuit-data) and [phase-angle clocks](#phase-angle-clocks);
- [schedules and time slices](#schedules-and-time-slices);
- [transitions](#transitions) from one time of a schedule to the next;
- [diagram layout sidecars](#diagram-layout-sidecars) from bus coordinates or pandapower geodata.

The MCP server that comes with the package has [a page of its own](mcp.md). The model
format is described in [model.md](model.md), the rules of the conversion and their checks
in [numerical-method.md](numerical-method.md#from-a-power-flow).

## Install

The package is not on PyPI. Build it from the repository; this needs a Rust toolchain
(1.85 or later), and pip builds the native module with maturin:

```sh
python -m pip install .                              # the package and numpy
python -m pip install ".[pandapower,powsybl,mcp]"    # with the converters and the MCP server
```

The extras are `pandapower`, `powsybl`, `pandas`, `mcp`, `paper` (for the figure scripts,
[paper/README.md](../paper/README.md)), and for development `test` and `lint`
([CONTRIBUTING.md](../CONTRIBUTING.md)). `maturin develop --release` in a virtual
environment builds the module in place instead.

## Running models

```python
import openkeito
result = openkeito.run("examples/rlc.json")     # a dict, JSON text or a path
result.status, result.time, result["V(n3)"]     # "completed", numpy arrays
result.summary["result"]["statistics"]          # the summary of `openkeito run`
openkeito.scan("examples/three_phase_fault.json")["Z1mag(b2)"]
batch = openkeito.run_batch({"a": model_a, "b": model_b}, jobs=8)
```

`run` and `scan` write the files of the command too when given `out` (`run(..., threads=N)`
overrides `simulation.threads`, as `openkeito run --threads N`). Invalid models and
failed runs raise `openkeito.OpenKeitoError`, whose `diagnostics` are the errors of the
summary (code, path, message, hint); the warnings found beside them are in its `warnings`,
and `summary` is the whole document. With `check=False`, `run` and `scan` return the failed
result instead (`result.status`, `result.summary["errors"]`, `result.warnings`). `validate`, `schema`
and `components` return the documents of the command, and so does `topology`:

```python
t = openkeito.topology("examples/three_phase_fault.json")    # a dict, JSON text or a path
[b["name"] for b in t["buses"]]                               # ["src", "b2", "b3"]
[c["name"] for c in t["components"] if any(x.get("bus") == "b2" for x in c["terminals"])]
[(d["code"], d["components"], d["nodes"]) for d in t["diagnostics"]]    # where the errors are
```

`topology` does not raise for a model that is not valid: the document has `valid`, `errors`
and `warnings`, and the connections of every model that can be read
([model.md](model.md#topology)). The names of the cases of `run_batch` are directory names, as in a batch file;
names that are not, or an `out` that already holds a batch result (without `force=True`),
raise `OpenKeitoError` before anything runs.

## From a power flow (pandapower)

```python
import pandapower.networks as pn
from openkeito.pandapower import from_pandapower
conv = from_pandapower(pn.case39(), end_time_s=0.2)   # runs pandapower's power flow first
result = openkeito.run(conv.model)
conv.compare(result)["max_abs_dv_pu"]                  # about 1e-10
```

The converter chooses the components so that the steady state of the EMT model is the
power flow, and the model starts from it (`"initialization": "steady_state"`):

| pandapower | OpenKeito |
|---|---|
| `ext_grid`, `gen` | `three_phase_source` behind its impedance Z, with E = V + Z·I and I = conj(S/3V) of the power flow. Z: short-circuit power (`s_sc_max_mva`, `rx_max`) or `xdss_pu`; otherwise 10 000 MVA with R/X 0.1, or 0.005 + j0.2 pu on the rating (a warning). With `generators="machine"`, every `gen` is a `synchronous_machine` at its power-flow output (see below) |
| `load`, `shunt` | `three_phase_load`: the constant impedance that takes the power-flow P and Q at the power-flow voltage (negative P: a current injection) |
| `sgen`, `storage` | `three_phase_current_source` with I = conj(S/3V); with `inverter_model="converter"` a `grid_following_converter` (the averaged inverter with its phase-locked loop, current control and current limit) whose references are the power-flow P and Q and whose operating point is the power-flow voltage, rated at `sn_mva` (missing: the apparent power / 0.9, a warning). `converter_parameters={"current_limit_pu": 1.2}` sets more of its fields |
| `line` | `pi_line` with one section (`parallel` circuits combined; no capacitance: `coupled_rl`). Without zero-sequence data R0 = 3 R1, X0 = 3 X1, C0 = 0.6 C1 (a warning). `g_us_per_km` is a resistance to ground at each end (`line<k>_g0`, `line<k>_g1`) |
| `trafo` | `three_phase_transformer`; the tap changes the rated voltage of its side where pandapower's power flow applies it (`tap_changer_type` Ratio or Symmetrical with `tap_side` hv or lv; otherwise it is left out, with a warning), `shift_degree` the clock number, `vector_group` the connections (YNyn for an even clock and Dyn for an odd one when missing). Ideal taps and `tap_step_degree` add to the phase shift, which must come to a multiple of 30°; `tap_dependency_table` is refused |
| `switch` | closed bus-bus switches join the buses (with `z_ohm` > 0: a coupled RL as in pandapower); an open line or transformer switch leaves that end on a node of its own, as does an out-of-service bus at one end of a line (`line<k>_open`): the branch stays energized from its other end, as in the power flow. A branch without a closed end is left out. A transformer open at a delta or ungrounded-star winding is represented by its no-load power at the closed side (a warning) |

The bus with pandapower index k becomes `bk` (buses joined by a switch take the smallest
index) and row k of table t the component `tk`; `conv.mapping` lists both and `conv.bus(k)`
gives the name. `trafo3w`, `impedance`, `ward`, `xward`, `dcline`, `svc`, the DC and VSC
tables and similar ones, and asymmetric elements raise `ConversionError` with the list of
what to change. `conv.warnings` lists every assumption.

`from_pandapower(net, generators="machine")` (and `from_powsybl`) converts the generators
to synchronous machines with their rotor windings and inertia instead of sources; they start
from the power flow with a constant field voltage and torque. External grids stay sources.
The option combines with `inverter_model="converter"` (machines beside converters); for PowSyBl
a generator named in `converter_generators` is a converter, and `generators` decides the others.
`machine_parameters` gives fields of `synchronous_machine` for all machines
(`{"inertia_h_s": 6.0}`) or for one (`{"gen/3": {...}}`; for PowSyBl the generator id). What
neither it nor the network gives (pandapower: `sn_mva`, `xdss_pu`, `rdss_ohm`; PowSyBl:
`rated_s` and the reactances of `generatorShortCircuit`) comes from
`openkeito.convert.TYPICAL_MACHINE`, a round-rotor unit within the typical ranges of Kundur's
tables 3.2 and 4.2, and is named in `conv.warnings`. A generator without a rating gets the
one that its power-flow output (or its largest active power, unless that is more than ten
times the output) loads to 85 %. The default, `generators="source"`, is unchanged.

`from_pandapower` runs `pandapower.runpp` and so replaces `net.res_*`. Pass
`power_flow_options` (keyword arguments of `runpp`) to change how, or `run_power_flow=False`
to keep a power flow that is already in the network. The model needs the power flow's
defaults `calculate_voltage_angles=True` and `trafo_model="t"`; a power flow without them
is refused where it matters.

## From a load flow (PowSyBl, CGMES)

```python
import pypowsybl as pp
from openkeito.powsybl import from_powsybl, from_cgmes
conv = from_powsybl(pp.network.create_micro_grid_be_network())  # or from_cgmes("grid.zip")
conv.compare(openkeito.run(conv.model))["max_abs_dv_pu"]        # about 1e-14
```

The converter runs pypowsybl's AC load flow (OpenLoadFlow) and writes IIDM's equivalent
circuits:

| IIDM | OpenKeito |
|---|---|
| generator | `three_phase_source` behind 0.005 + j0.2 pu on `rated_s` (or the short-circuit data, below), E = V + Z·I; with `generators="machine"` a `synchronous_machine`, as for pandapower |
| load, shunt compensator, static var compensator, battery | constant impedance at the load-flow voltage (negative P: a current injection). With `inverter_model="converter"` a battery is a `grid_following_converter` at its load-flow powers and voltage, rated at the larger magnitude of `min_p` and `max_p`; `converter_generators=[ids]` makes the same of the generators named (inverters; they do not hold the voltage) |
| line | `coupled_rl` (R + jX; zero sequence 3×, or the short-circuit data) with the shunts g1 + jb1 and g2 + jb2 as `three_phase_load` at the ends |
| two-winding transformer | `three_phase_ideal_transformer` with ρ·e^{jα} at side 1, the magnetizing g + jb at its other terminal, R + jX to side 2 (all at the current tap; phase shifters included) |
| three-winding transformer | three such legs (ratio rated_u0/rated_ui and the leg's taps) to a star node `tr3_<k>_star` |
| dangling (boundary) line | shunt g + jb at the bus, R + jX to a boundary node `dl<k>_x` that takes p0 + jq0 at the load-flow boundary voltage |

- By default the load flow keeps the generators' set points (no distributed slack) and
  takes the largest generator as the slack, with OpenLoadFlow's tolerances tightened to
  1e-10 per equation and 1e-6 MW at the slack bus (`tight_load_flow_parameters()`; pass
  `load_flow_parameters` to change it). What the load flow leaves unbalanced at a bus (the
  slack's losses, a residual) goes to a generator there, or to an injection `balance<k>`, and
  is listed in `conv.warnings`: the load-flow voltages then solve the model exactly.
- The buses (bus view) that have a load-flow voltage become `b0`, `b1`, … in the order of
  `network.get_buses()`; `conv.mapping` lists the bus ids and element ids (CGMES mRIDs)
  with their components, and `conv.bus(id)` gives a model bus. Buses without a voltage (not
  connected to a generator) are left out with what is connected to them, and a warning
  names them; a network without any load-flow result raises `ConversionError`, as do HVDC
  lines, converter stations and tie lines.
- A branch that is open at one end stays energized from the other, with the open end on a
  node of its own (`line<k>_open`), as in the load flow.
- Without the distributed slack, OpenLoadFlow does not converge on some networks that its
  defaults solve; the error then says to pass
  `load_flow_parameters=pypowsybl.loadflow.Parameters()`.
- Without short-circuit data, zero sequence passes through transformers as through YNyn0,
  lines get Z0 = 3 Z1 and generators 0.005 + j0.2 pu (warnings).
- PowSyBl's IIDM network does not carry the nominal frequency; pass `frequency_hz` (default
  `50.0`, e.g. `frequency_hz=60.0`) to `from_powsybl` and `from_cgmes`.

### Short-circuit data

A load flow does not need subtransient reactances, zero-sequence impedances, winding
connections or grounding, and most networks do not hold them. Where they are, the converter
uses them in place of its assumptions, without a warning; `conv.mapping["short_circuit"]`
lists, per element, the values used and where they came from. There are three sources, the
later winning over the earlier:

1. **IIDM extensions**, read by `from_powsybl` (`openkeito.powsybl.short_circuit_data(network)`
   shows them): `generatorShortCircuit` and the phase-angle clocks from pypowsybl's tables;
   `generatorFortescue`, `lineFortescue`, `twoWindingsTransformerFortescue` and
   `threeWindingsTransformerFortescue`, which pypowsybl 1.16 keeps when it reads an XIIDM
   file but does not give as tables or let you create, from the network written as XIIDM.
2. **CGMES files**: pypowsybl's import leaves the short-circuit attributes of the equipment
   profile out (only the phase-angle clock arrives, with the post-processor
   `PhaseAngleClock`). `from_cgmes` therefore reads them from the file itself
   (`cgmes_short_circuit_data`, standard library only): of `SynchronousMachine`
   `satDirectSubtransX`, `satDirectTransX`, `satDirectSyncX`, `r`, `r0`, `x0`, `r2`, `x2`,
   `earthing`, `earthingStarPointR/X`; of `ACLineSegment` `r0`, `x0`, `g0ch`, `b0ch`; of
   `PowerTransformerEnd` `connectionKind`, `grounded`, `rground`, `xground`,
   `phaseAngleClock`, `r0`, `x0`, `g0`, `b0`. `short_circuit_profile=False` turns that off.
   The units follow the version of the file, which the converter tells from the CIM
   namespace: `r0`, `x0`, `r2` and `x2` of a synchronous machine are per unit in CGMES
   2.4.15 and ohm in CGMES 3.0 (`r` is ohm and the three reactances `satDirect...X` are per
   unit in both).
3. **`short_circuit_data={id: {field: value}}`** of `from_powsybl` and `from_cgmes`, for
   data from elsewhere. The fields (`openkeito.powsybl.SHORT_CIRCUIT_FIELDS`) have the names
   and units of the extensions, in ohm and siemens:

```python
conv = from_powsybl(net, short_circuit_data={
    "GEN": {"direct_sub_trans_x": 0.9, "xz": 0.4, "grounded": True, "grounding_r": 2.0},
    "LINE1": {"rz": 12.0, "xz": 100.0, "bz": 2e-4},
    "T1": {"connection_type1": "DELTA", "connection_type2": "Y_GROUNDED", "xz": 10.0, "rz": 0.2}})
```

| Element | Fields | In the model |
|---|---|---|
| generator | `direct_sub_trans_x`, `direct_trans_x`, `direct_sync_x`, `stator_r`, `step_up_transformer_x`; `rz`, `xz`, `grounded`, `grounding_r`, `grounding_x`; `rn`, `xn` | source: Z1 = `stator_r` + j(x″ + step-up) (x′ when there is no x″), Z0 = `rz` + j`xz` plus three times the grounding impedance, or a star point of its own when not grounded; Z2 = Z1, so `rn`, `xn` are not used (warning). Machine: x″d = x″q, x′d, xd, ra, x0 and `neutral`; a reactance that breaks xd > x′d > x″d is left out (warning), `machine_parameters` win |
| line | `rz`, `xz`; `gz`, `bz` (whole line) | Z0 of the `coupled_rl`; the shunt as a grounded star y0 and a star y1 − y0 with a star point of its own. Without `gz`, `bz`: y0 = y1 (warning) |
| dangling line | `rz`, `xz` | Z0 of the `coupled_rl` |
| two-winding transformer | `connection_type1`, `connection_type2` (`Y`, `Y_GROUNDED`, `DELTA`), `rz`, `xz` (at side 2, as r and x), `grounding_r1` … `grounding_x2`, `free_fluxes`, `xm` (the zero-sequence magnetizing reactance, at side 2), `phase_angle_clock` | see below |
| three-winding transformer | the same per leg: `connection_type1` … `3`, `rz1` …, `free_fluxes1` …, `phase_angle_clock_leg2`, `_leg3` | see below |

Transformers keep IIDM's circuit, so the positive sequence, and with it the load flow, is
untouched (1e-12 pu, as before); only the zero sequence follows the windings:

- YNyn: passes, with `rz` + j`xz` and three times both grounding impedances in the series
  branch.
- YNd (and Dyn): the series branch is blocked (Z0 = 10⁶ Z1, `ZERO_SEQUENCE_BLOCK`), and on
  the star side a component `<name>_z0` connects the zero sequence to gnd through the
  transformer's impedance: a `three_phase_transformer` YNd without magnetizing branch and
  with nothing at its delta, which takes no positive- or negative-sequence current.
- Any other pair (a star that is not grounded on either side, Dd): blocked.
- A leg of a three-winding transformer passes between its bus and the star node if its
  winding is a grounded star, is blocked otherwise, and a delta leg connects the star node
  to gnd through its impedance.
- The magnetizing admittance takes zero-sequence current only at a grounded star at side 1
  with `free_fluxes` false; otherwise its star point is a node of its own. Where it takes
  it, the zero-sequence branch is 1/(j·`xm`) when `xm` is given (two-winding transformers),
  and the positive-sequence magnetizing admittance otherwise.
- With connections missing for a winding, the transformer stays YNyn0 (warning), with `rz`,
  `xz` when given.

`three_phase_transformer` in place of this circuit would move the load flow (6e-5 pu on the
Eurostag example, 1e-4 pu on MicroGrid BE): its magnetizing branch sits in the middle of the
leakage, IIDM's directly behind the ratio, and the difference grows with the magnetizing
admittance.

Not represented: zigzag windings with a grounded star point (`Zn` in CGMES: the transformer
stays YNyn0, with a warning), the zero-sequence shunt of dangling lines, the mutual coupling
between parallel lines, and the zero-sequence magnetizing admittance seen from side 2.

### Phase-angle clocks

IIDM's load flow does not turn the voltages by the clock numbers of the transformers, and by
default neither does the model: with `phase_angle_clocks="ignore"` every transformer has the
clock 0, and a warning names those whose data say otherwise. Sequence impedances and the
fault current at the fault are right either way; the phase currents on the far side of a Yd
transformer are then those of a Yy one.

```python
conv = from_powsybl(net, phase_angle_clocks="rotate")
conv.mapping["bus_phase_shift_deg"]      # what was added to the load-flow angle of each bus
```

With `"rotate"`, side 2 of a two-winding transformer lags side 1 by 30° times its clock in
the positive sequence and leads it by as much in the negative sequence (legs 2 and 3 of a
three-winding transformer against leg 1), as a transformer of that vector group does and as
the pandapower converter has it. The clocks come from the extensions, from
`PowerTransformerEnd.phaseAngleClock` of a CGMES file or from `phase_angle_clock` in
`short_circuit_data`.

- The transformers divide the network into zones. The zone of the bus with the highest
  nominal voltage keeps the angles of the load flow, and every other zone is turned by the
  clocks on the way to it, so the turned load flow is still the steady state of the model
  (1e-12 pu for the Eurostag example, IEEE 14 and MicroGrid BE). `conv.power_flow` and
  `conv.compare` hold the turned angles.
- Clocks that do not add up to the same angle on every way to a bus (parallel transformers
  with different clocks) have no such steady state: `ConversionError` names the
  transformers. A phase tap changer adds its angle to the clock.
- Where the connections are not known, an odd clock is taken as Dyn (the delta at the
  higher rated voltage) and an even one as YNyn (warnings).
- For a line-to-ground fault behind a Dyn1 transformer, the phase currents of the generator
  are those of symmetrical components, in the proportion (1, 0, −1); without the clocks
  they are in the proportion (2, −1, −1).
- Between two grounded stars whose connections are both known as `Y_GROUNDED`, a clock of 6
  (YNyn6) sets `zero_sequence_inverted: true` on the `three_phase_ideal_transformer`, which
  reverses the zero-sequence turns ratio without warning; clocks other than 0 and 6 or unknown
  connections warn and pass the zero sequence as through YNyn0.

## Schedules and time slices

A schedule file holds values of network elements at a list of times
(`openkeito schema --schedule`; `openkeito validate` checks it):

```json
{"format": "openkeito-schedule", "version": "0.1", "interpolation": "step",
 "times": ["2026-11-21T00:00:00+09:00", "2026-11-21T01:00:00+09:00"],
 "series": [{"target": "gen/0", "quantity": "active_power_mw", "values": [250.0, 230.0]},
            {"target": "load/3", "quantity": "reactive_power_mvar", "values": [12.0, 10.5]}]}
```

- Times are RFC 3339 date-times with an offset, strictly ascending. Each series gives one
  value per time; `interpolation` is `step` (default) or `linear` (tap positions and
  `in_service` always step).
- Quantities: `active_power_mw`, `reactive_power_mvar` (with the sign of the element's
  table), `voltage_pu` (set points of generators and external grids), `tap_position`
  (integers) and `in_service` (0 or 1). For pandapower the target is `table/index`; for
  PowSyBl the element id (the CGMES mRID) of a generator, load, battery, shunt, line or
  two-winding transformer (`openkeito.powsybl.KIND_QUANTITIES`), with IIDM's signs. A
  tap position needs a transformer with a ratio tap changer and a position it has.
- A scheduled power is the power that enters the power flow: for pandapower it is divided
  by the element's `scaling` (a shunt's `step`) when it is written into the network.
- Every value is a finite number: `Schedule.from_values` and `from_frame` raise
  `ValueError` for a NaN or a gap.
- Diagnostics: E500 format or version, E501 no times or series, E502 a time that is not
  RFC 3339 with an offset, E503 times not ascending, E504 not one value per time, E505 a
  quantity of an element given twice, E506 a value out of range, E507 an empty target. A
  target the network does not have is reported by the converter.

```python
from openkeito.schedule import Schedule
from openkeito.pandapower import schedule_from_opf, run_time_slices, fault
inputs = Schedule.load("loads.json")                    # loads at every hour
plan, notes = schedule_from_opf(net, inputs, opf="ac")  # generator set points from runopp
run = run_time_slices(net, plan, [fault(16, 0.05, 0.12)], end_time_s=0.3, jobs=8)
run.to_pandas()        # case, time, disturbance, status, max_abs_dv_pu, ...
```

`schedule_from_opf` applies the inputs at each time, solves pandapower's optimal power
flow and adds the generator outputs and voltages and, for controllable static generators,
storage and loads, their active and reactive power, so that the power flow of a time slice
is the OPF's (elements that are out of service are skipped). With `opf="dc"` the reactive power and
voltages come from the AC power flow of each slice; a warning lists the times at which the
slack moved by more than 1 % of the generation from the plan. `run_time_slices` applies the
schedule at each time, solves the AC power flow, converts the network, adds each
disturbance (a component's `bus` may be a bus of the source network: a pandapower index
or an IIDM bus id) and runs all cases in parallel; each row compares the steady state with
the power flow. `openkeito.powsybl.run_time_slices` does the same for an IIDM network.
The case of time k and a disturbance is named `t<k>.<name>`, so the disturbances need
names of their own: give `fault(..., name=...)` to each of several faults at one bus. A
time whose power flow does not converge, a fault at a bus the network does not have and
equal names raise `ConversionError` (with the time) before anything runs.

## Transitions

A time slice stays at the operating point of one time. A transition follows the schedule
from one time to the next: the run starts from the power flow of `from_time`, and the set
points of the controls move to the values of `to_time`.

```python
from openkeito.pandapower import transition, run_transitions
tr = transition(net, plan, "2026-11-21T08:00:00+09:00", "2026-11-21T09:00:00+09:00",
                start_time_s=1.0, ramp_time_s=5.0, settle_time_s=10.0)
tr.model, tr.changes, tr.warnings     # the model, what moves, what could not be followed
run = run_transitions(net, plan, jobs=8, out="moves")   # every pair of neighbouring times, in parallel
run.to_pandas()        # case, status, max_abs_dv_pu, end_max_abs_dv_pu, frequency_min_hz, ...
run = run_transitions(net, plan, chain=True, out="day") # one run through all times
run.time_rows          # one row per time, at the end of its settling time
```

The network is converted at both times (by default with `generators="machine"` and
`inverter_model="converter"`), and the two models are compared:

| In the network | In the run |
|---|---|
| generator (a synchronous machine) | gets an excitation system, a governor and a power system stabilizer made of control blocks (`openkeito.transition.add_machine_controls`; typical values `EXCITER`: gain 200, 0.02 s, ceilings −6.4 and 7 pu; `GOVERNOR`: droop 5 %, 0.2 s, turbine 0.3 s, valve 0 to 1.1 pu; `PSS`: gain 9.5, washout 1.4 s, lead 0.154 s over 0.033 s, output −0.1 to 0.1 pu; `exciter=`, `governor=`, `pss=` and `machine_controls={"gen/0": {...}}` change them, `False` leaves the input constant or the stabilizer out). A machine that delivers less than 10 % of its rating as active power at every time gets no stabilizer, which a warning says. The load reference and the voltage reference move by what the two power flows ask for |
| static generator, storage (a converter) | the active and reactive power references are signals that move from the one power flow to the other |
| load, shunt | the impedance that both times have stays; the rest is switched out or in, in `load_steps` (default 4) equal parts over the ramp; a part that is an inductance alone carries 1 W, which holds its node while its switch is open, and a change of less than 1 W per part is not switched |
| external grid (and a generator converted as a source) | a `three_phase_controlled_source` whose internal voltage moves to that of the second power flow; it stays at the rated frequency. `sources="hold"` keeps the voltage of `from_time` |
| `in_service` of a line, transformer, generator; a bus-bus switch | breakers (ideal switches that open at current zeros) at `switch_time_s` (default: `start_time_s`). A machine that stops is tripped with its load reference going to zero; a machine or converter that starts is connected from t = 0 without load |
| tap position | kept at the value of `from_time` (a transformer has a constant ratio), with a warning; a current injection (`inverter_model="current_source"`) also keeps its value |

- **Time.** The run has `start_time_s` (default 1 s) of steady state, the ramps, and
  `settle_time_s` (default 10 s) after the last change (or ends at `end_time_s`).
  `ramp_time_s` (default 5 s) is a number for all or `{"gen/0": 20.0, "default": 5.0}`; 0 is a
  step. `ramp_rate_mw_per_min` gives a machine or a converter the time that its change of
  active power takes at that rate instead. The times of a schedule are far apart, so the ramp
  is the caller's choice; only for a schedule with `"interpolation": "linear"`,
  `ramp_time_s="schedule"` takes the real time between `from_time` and `to_time`, which may
  be any two times within the schedule (30 s of an hour, say).
- **The result.** `run_transitions` gives one row per interval (`openkeito.transition.measure`):
  the start against the power flow of `from_time` (`max_abs_dv_pu`, `max_abs_dva_degree`) and
  the drift before the change; the end against the power flow of `to_time`
  (`end_max_abs_dv_pu`, `end_max_abs_dva_degree`, `end_max_abs_dp_mw`) and how much voltage and
  frequency still move there (`end_voltage_swing_pu`, `end_frequency_swing_hz`); and on the
  way the smallest and largest bus voltage and machine frequency and the largest machine and
  converter currents. `tr.start` and `tr.end` are the conversions at the two times, with
  their power flows.
- **How close the end comes.** With everything following, the end is the power flow of
  `to_time` within the error of the time step (1e-5 pu at 50 µs), once the swings have
  decayed. The stabilizer is what makes them decay: with it (the default of a transition)
  the IEEE 39-bus case is 0.017 MW from the second power flow 10 s after the change;
  without it (`pss=False`) and with the typical governor it is 20 MW away then and still
  3 MW after 60 s. The typical values are those of a stabilizer for a local mode near 1 Hz;
  for slow modes between areas give a longer washout
  (`pss={"washout_time_constant_s": 10.0}`). `measure(tr, result, end_time_s=...)` gives the
  distance from the second power flow at any time of the run. What a warning names as not
  followed stays as a difference.
- **More than two times.** With `chain=True` (or `times=[...]`, or
  `transition(net, plan, times=[...])`) the transition goes through all the times in one
  run: steady state, then for each time its changes and its settling time. The time between
  two times of the schedule is not run. `ramp_time_s`, `settle_time_s`,
  `ramp_rate_mw_per_min` and `switch_delay_s` (the time from the start of an interval's
  changes to its breakers) take one value or a list with one entry per interval.
  `run.time_rows` has one row per time, measured at the end of its settling time against its
  power flow (`measure(tr, result, time=...)`), with `settled` (`True` when the voltage and
  frequency swings and the active-power difference at the end of the settling time are within
  their tolerances; otherwise a warning names the time and suggests a longer `settle_time_s`).
  `tr.estimate(calibrate=True)` (or `openkeito.transition.estimate(tr)`) gives the number of
  steps, stages, components, controls and switches, and by running a short calibration slice
  estimates `step_wall_us` and `estimated_wall_s` before the full run starts.
  Loads and breakers go in and out as often as
  the schedule says; a machine may start at any time, stop, and re-synchronize to the bus
  through its governor and excitation system when it returns to service (re-synchronizing without
  both `governor` and `exciter` is refused with `E700`); transformer taps (`tap_changes`) and
  associated series impedances (`impedance_changes`) step at the switching time of the interval.
  The run is as long as all changes and settling times together
  and uses one core, and its model grows with the number of times (a smaller `load_steps`
  keeps it small); the intervals as cases of their own (the default) run in parallel.
  `examples/transition_three_times.json` is such a model, as the tool writes it.
- **Ratings.** A generator without a rating in the data is rated for all times of the
  transition: its largest apparent power is 85 % of the rating, and the reactive power it
  absorbs at most 85 % of V²/X_d. The warnings list these machines;
  `machine_parameters={"gen/3": {"rated_power_mva": ...}}` sets a rating instead. Time
  slices rate a machine by the one time they run.
- **Without an external grid** the machines run as an island, and nothing but the governors
  holds the frequency; it returns to the rated one because every load reference moves by what
  the second power flow gives that machine.
- **Cost.** One second of a transition of case118 (machines and converters, 50 µs) takes
  about 3 s on one core; [performance.md](performance.md#transitions-of-converted-networks-1-s-m4) has the measurements. The intervals run in
  parallel, one per core.

`openkeito.powsybl.transition` and `run_transitions` take the same arguments for an IIDM
network. `add_machine_controls(conv)` also gives the machines of any conversion an
excitation system and a governor, for time slices with a disturbance; `pss=True` adds the
stabilizer, which is off by default there.

## Diagram layout sidecars

`openkeito.layout` generates optional diagram layout sidecar documents
(`"format": "openkeito-layout"`, `"version": "0.1"`) for the browser diagram without
modifying the model format (`"0.7"`) or `conv.model`:

```python
from openkeito.layout import (
    LAYOUT_FORMAT,
    LAYOUT_VERSION,
    empty_layout,
    layout_from_bus_coordinates,
    layout_from_pandapower,
    serialize_layout,
)

sidecar = layout_from_pandapower(net, conv, width=1200.0)
Path("model.layout.json").write_text(serialize_layout(sidecar), encoding="utf-8")
```

- `LAYOUT_FORMAT` (`"openkeito-layout"`) and `LAYOUT_VERSION` (`"0.1"`) name the sidecar
  format, and `empty_layout()` returns an empty document with `circuit` and `controls`
  faces (`{"rails": {}, "items": {}}`).
- `layout_from_bus_coordinates(bus_coords, *, scale=1.0)` maps finite x coordinates (given
  as a number, `(x, y)` sequence, `{"x": ...}` / `{"dx": ...}` mapping, or GeoJSON Point)
  to relative horizontal rail offsets `dx = (x - min_x) * scale` under
  `faces.circuit.rails["bus:<name>"]`. If no finite coordinate is present, it returns
  `empty_layout()`.
- `layout_from_pandapower(net, conv, *, width=1200.0)` reads `net.bus["geo"]` (GeoJSON
  Point) or `net.bus_geodata` (`x`), maps pandapower bus indices through
  `conv.mapping["buses"]` to model bus names (averaging merged buses), and scales their
  horizontal span to `width`.
- `serialize_layout(layout)` serializes the sidecar document as deterministically key-sorted
  JSON with a trailing newline. The browser page reads this file via `Import layout` without
  changing the model JSON (`"0.7"`).
