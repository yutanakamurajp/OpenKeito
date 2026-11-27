# Performance

Run times of reference cases, measured locally (times are not compared in CI). Report the
accuracy of a case together with its time.

## Machine

| Item | Value |
|---|---|
| CPU | Intel Core Ultra 9 285K: 8 fast cores (P) and 16 slower ones (E), 24 threads |
| Memory | 63 GB |
| OS | Windows 11 Pro |
| Rust | 1.100.0, `--release` |
| Date | 2026-11-22 |

## How the times are measured

The processor has two kinds of cores, and the system moves a thread that runs for long to
the slower kind: the same run takes 1.3 times longer there (a fixed loop of arithmetic:
1.25 ms on a P core, 1.65 ms on an E core). Times taken without choosing the cores are
therefore not comparable with each other; the first version of this page had such numbers
for the larger systems.

- **One thread**: the process is pinned to one P core,
  `cmd /c start /b /wait /affinity 400 <test executable> --ignored --nocapture <test>`
  (the mask is a hexadecimal set of logical processors; on this machine the P cores are
  the processors 0, 1, 10 to 13, 22 and 23, found by running `reference_speed` of
  `tests/benchmark.rs` on each processor in turn).
- **Several threads**: the process is pinned to the cores it may use, and the table says
  which: the 8 P cores (mask `C03C03`), or all 24 (P + E).
- Configurations that are compared are run in turn, several times (5 rounds for the tables
  of the lines, 9 for the examples), and the best time of each is taken. Before and after
  a change, the two builds are run in turn three times.
- A time per step is the time of the run without the output, divided by the steps.

One measurement moves by 1 to 2 % from run to run; differences below that are not
findings.

## Buck converter, 10 kHz, 1 s (M1.5)

The buck converter of `examples/buck.json` (100 V, L = 1 mH, C = 100 µF, R = 10 Ω,
triangular carrier at 10 kHz, duty 0.405, 2S-DIRK, time step 1 µs) over 1 s: 10^6 steps,
4 unknowns, 20 000 periods with a switch and a diode changing twice in each. One P core.

| Run | Wall clock | Factorize | Solve (rest of the loop) | Output |
|---|---|---|---|---|
| Solver only (`power_electronics`, test `time_of_one_second_of_a_10_khz_buck_converter`), 3 runs | 0.213–0.218 s (213–218 ns per step) | 0.007 s | 0.185–0.189 s | 0.021 s (sink that only counts rows) |
| `openkeito run`, every step in `result.csv` (10^6 rows, 101 MB), 3 runs | 1.00–1.01 s | 0.008 s | 0.20 s | 0.80–0.81 s |
| `openkeito run`, `output_interval_steps` = 10 (10^5 rows, 10 MB), 3 runs | 0.26–0.27 s | 0.008 s | 0.17 s | 0.08–0.09 s |

Counts: 40 007 factorizations, 2 040 006 solutions (two stages per step), 40 004
re-solutions to settle diode states (two per switching action), 40 003 switching events.

- Factorization takes 3 % of the solver time: caching LU factors per combination of switch
  states would not pay off for this circuit size.
- Writing the CSV dominates a run that records every step. Formatting each value with the
  shortest exact representation costs about 0.15 µs.
- Accuracy of the same case: the integration error against the exact solution for the
  same switching instants is 1.5e-6 A in the inductor current at h = 1 µs (second order);
  the average output differs from the textbook value for the duty realized by 1.2e-4 V
  (`tests/power_electronics.rs`).
- The solver-only time was 208 ns per step before the circuit could be divided into parts
  (see [Changes that must not slow down other models](#changes-that-must-not-slow-down-other-models)).

## Three-phase ring systems, 1 s (M2)

`crates/openkeito-core/tests/benchmark.rs` builds a ring of N buses at 66 kV joined by 10 km
pi lines (one section each), with a grounded star load of 10 MW + 3 Mvar on every bus and,
on every tenth bus, an 11 kV source behind a 100 MVA YNd11 transformer with magnetizing
branch. A three-phase fault to ground on bus N/2 starts at 0.1 s and is cleared at 0.2 s
(current zeros). The run starts from the steady state, records 4 columns every 10 steps
(the sink only stores them) and uses 2S-DIRK. One P core (test `ring_systems`).

Time step 10 µs (10^5 steps, 2·10^5 solutions):

| Buses | Unknowns | Sparse LU: 1 s takes | Per step | Dense LU: 1 s takes | Per step |
|---|---|---|---|---|---|
| 10 | 51 | 0.154 s | 1.54 µs | 0.292 s | 2.92 µs |
| 30 | 147 | 0.555 s | 5.55 µs | 1.89 s | 18.9 µs |
| 100 | 483 | 2.07 s | 20.7 µs | 18.7 s (run for 0.2 s, ×5) | 187 µs |
| 300 | 1443 | 6.60 s | 66.0 µs | 184 s (run for 0.02 s, ×50) | 1840 µs |
| 1000 | 4803 | 25.7 s | 257 µs | — | — |

Time step 50 µs (2·10^4 steps):

| Buses | Unknowns | Sparse LU: 1 s takes | Per step | Dense LU: 1 s takes | Per step |
|---|---|---|---|---|---|
| 10 | 51 | 0.032 s | 1.58 µs | 0.057 s | 2.85 µs |
| 30 | 147 | 0.111 s | 5.53 µs | 0.380 s | 19.0 µs |
| 100 | 483 | 0.400 s | 20.0 µs | 3.68 s (run for 0.2 s, ×5) | 184 µs |
| 300 | 1443 | 1.22 s | 61.0 µs | 45.4 s (run for 0.02 s, ×50) | 2270 µs |
| 1000 | 4803 | 5.47 s | 274 µs | — | — |

- Against the first version of this page, taken without pinning, the times are 8 to 25 %
  shorter (1.54 against 2.06 µs for 10 buses, 257 against 279 µs for 1000 buses). The time per step does not depend on the time step
  (the difference of the two tables is the scatter of single runs).
- The time per step of the sparse LU grows about in proportion to the unknowns (3.9 times
  more for 3.3 times as many unknowns from 1443 to 4803); that of the dense LU grows as
  their square. Factorizations take 7 per run (the steady state, the start, the fault and
  its clearing) and under 0.03 s; the rest of the time is the steps: right-hand sides,
  triangular solutions and state updates.
- Accuracy of the same runs: the departure from the steady state before the fault is
  3.4e-7 of the amplitude at 10 µs and 8.5e-6 at 50 µs (25 times, second order); the
  largest condition estimate is 8e7, so rounding stays below about 2e-8 of the largest
  values. Dense and sparse runs of the examples agree to rounding (`tests/solvers.rs`).
- Where `auto` switches. An ideal source and k coupled RL sections (`benchmark.rs`, test
  `dense_and_sparse_per_step_of_a_chain`, time per step without output, one P core):

  | Unknowns | 9 | 12 | 15 | 18 | 24 | 30 |
  |---|---|---|---|---|---|---|
  | Dense (µs) | 0.242 | 0.323 | 0.403 | 0.500 | 0.721 | 0.987 |
  | Sparse (µs) | 0.234 | 0.292 | 0.349 | 0.398 | 0.505 | 0.621 |

  The examples of M1 and M1.5 (4 or 5 unknowns) are faster with the dense LU (0.10 against
  0.14 µs per step for `rlc.json`); `three_phase_fault.json` (29 unknowns) with the sparse
  one (0.77 against 1.14 µs). `auto` takes the sparse LU from 16 unknowns on.
- IEEE 39-bus system (New England; built locally from MATPOWER case39 with generators as
  sources behind 0.02 pu, constant-impedance loads and pi lines; not in the repository):
  288 unknowns, 1 s at 10 µs in 0.95 s (3 runs, `run_s` of `openkeito run` with 10^4 CSV
  rows; taken without pinning, not measured again). Its steady state agrees with the power
  flow of case39 to 6e-8 pu and 6e-7°.

## Travelling-wave lines and the division of the circuit (M4)

A `bergeron_line` has no matrix entry between its ends, so a circuit divides at such lines
into parts with a matrix each (`simulation.partitioning`), which threads solve side by side
(`simulation.threads`); see
[numerical-method.md](numerical-method.md#parts-of-a-circuit-that-are-solved-on-their-own).
Test `partition_per_step` of `benchmark.rs`, 0.2 s at 10 µs (2·10^4 steps, with the fault
and its start; sparse LU from 16 unknowns on in every part), time per step in µs:

- the **ring** above with its 10 km lines as `pi_line` or as `bergeron_line` (travel times
  33.8 and 45.8 µs): every bus is a part of its own when the circuit is divided at every
  line, the smallest parts there are;
- **blocks**: chains of 30 or 100 buses joined by pi lines, with a source behind a
  transformer on the first bus, and the blocks joined into a ring by 50 km lines: parts of
  111 or 321 unknowns.

One thread (one P core), and 2, 4 and 8 threads on the 8 P cores:

| System | Unknowns | pi lines | Bergeron, `off` | `every_line`, 1 thread | `auto`: parts (largest) | 1 thread | 2 | 4 | 8 |
|---|---|---|---|---|---|---|---|---|---|
| ring of 10 buses | 51 | 1.56 | 1.58 | 2.05 | 1 (51) | 1.59 | — | — | — |
| ring of 30 buses | 147 | 5.37 | 4.98 | 5.91 | 3 (78) | 5.15 | 4.48 | 5.88 (3 threads) | — |
| ring of 100 buses | 483 | 19.6 | 17.9 | 19.8 | 7 (78) | 17.6 | 15.9 | 11.6 | 8.73 (7 threads) |
| ring of 300 buses | 1443 | 62.7 | 54.7 | 58.9 | 21 (78) | 52.6 | 36.6 | 28.8 | 21.1 |
| ring of 1000 buses | 4803 | 248 | 231 | 239 | 70 (78) | 211 | 112 | 75.3 | 51.8 |
| 8 blocks of 30 buses | 867 | 47.5 | 40.8 | 37.6 | 8 (111) | 37.9 | 29.0 | 20.7 | 15.4 |
| 16 blocks of 30 buses | 1731 | 89.5 | 81.5 | 75.2 | 16 (111) | 75.6 | 49.9 | 36.2 | 25.4 |
| 8 blocks of 100 buses | 2547 | 139 | 130 | 126 | 8 (321) | 126 | 87.3 | 59.5 | 42.7 |

(The columns of several threads come from a run on the 8 P cores, whose one-thread times
are 1 to 4 % above those of the single pinned core; a number of threads above the number
of parts is the number of parts.)

All 24 cores (P + E), where the system places the threads:

| System | 8 threads | 16 | 24 |
|---|---|---|---|
| ring of 300 buses (21 parts) | 24.5 | 19.7 | 16.6 (21 threads) |
| ring of 1000 buses (70 parts) | 63.5 | 50.6 | 46.4 |
| 16 blocks of 30 buses (16 parts) | 33.0 | 22.2 | — |
| 8 blocks of 100 buses (8 parts) | 51.4 | — | — |

- **The line model.** The ring with Bergeron lines takes as long per step as with pi lines
  at 10 buses and 7 to 13 % less from 100 buses on (no entries between the buses in the
  matrix), not divided.
- **Dividing, one thread.** With parts of 100 to 300 unknowns the divided circuit is 2 to
  8 % faster than the undivided one (smaller matrices, and a part is solved for both stages
  while its data are in the cache). With one bus per part it is 3 to 30 % slower: every
  part and every line between parts costs 10 to 50 ns per step (the currents and voltages
  of a line go through buffers of its own), against the 150 to 200 ns that a bus with its
  load takes. `auto` therefore keeps parts that a line joins together up to 48 named nodes
  (16 buses): the rings are then within −8 to +3 % of the undivided run on one thread.
- **Threads.** On the 8 P cores the 1000-bus ring runs 4.5 times faster than undivided
  (4.2 times against the divided circuit on one thread), the blocks 2.7 to 3.3 times; two
  threads give 1.1 to 2.1 times. All 24 cores give 5.0 times for the ring of 1000 buses
  (46 µs per step), a little more than the 8 P cores alone; with 8 threads the run is
  slower there than pinned to the P cores (63.5 against 51.8 µs), because some of the
  threads land on E cores. What keeps the gain below the number of threads: the links and
  the parts are two hand-overs per step, and one thread does what lies between them (the
  outputs, the controls, the comparison with the steady state over every node).
- **Where threads stop paying.** A step hands its groups of parts to the threads and waits
  for them twice (parts, then links); that costs of the order of 1 µs, more with more
  threads. The ring of 30 buses (3 parts, 5 µs per step) gains 14 % from two threads and
  loses 13 % with three; the ring of 10 buses divided at every line loses a factor of 2 with 8 threads
  (4.05 against 2.05 µs). The solver therefore uses at most one thread per 100 unknowns
  and elements of the circuit, never more than the parts or the cores, and one for a circuit that is one part (W301 when the model asks
  for more). `threads` is 1 unless the model says otherwise, and the cases of a batch that
  run at the same time use one thread each.
- **Accuracy of the same runs.** Departure from the steady state before the fault: 6.0e-6
  of the amplitude for the rings with Bergeron lines and 1.5e-6 to 1.7e-6 for the blocks
  (the travel times are not whole numbers of steps: the interpolation error of the line),
  against 3.4e-7 and 2.5e-7 to 2.7e-7 with pi lines. Divided against undivided: 2e-14 to
  3e-14 of the largest value for the rings, 1.6e-13 and 1.2e-12 for the blocks, where the
  condition estimate (8e7) allows 2e-8. Runs on 1, 2, 3 and 8 threads agree to the last
  bit (`tests/partition.rs`).
- **Factorizations.** A fault factorizes the matrix of its part only (in the ring of 12
  buses divided at every line: 4 factorizations of a matrix of 3 to 21 unknowns for the
  fault and its clearing; `tests/partition.rs`).

## Components of M4

Time per step of the examples (test `examples_per_step`, one P core, best of 9 runs,
without the output). Each has two stages per step; "solutions" above 2 are the trial
solutions for the diode states and the stage iterations.

| Example | What it shows | Unknowns | Control blocks | Per step (µs) | Solutions per step |
|---|---|---|---|---|---|
| `rlc` | linear circuit, dense LU | 4 | 0 | 0.098 | 2.00 |
| `rl_breaker` | a switch | 5 | 0 | 0.136 | 2.00 |
| `rectifier` | four diodes | 4 | 0 | 0.187 | 2.00 |
| `buck` | switch and diode at 10 kHz, switching at grid points | 4 | 3 | 0.191 | 2.04 |
| `buck_interpolated` | the same with instants within steps (10 times the step; 404 of 2000 steps split) | 4 | 3 | 0.652 | 2.81 |
| `arrester` | nonlinear resistor (power law, 0.29 stage iterations per step) | 1 | 0 | 0.149 | 2.29 |
| `saturable_reactor` | nonlinear inductor | 4 | 0 | 0.193 | 2.02 |
| `transformer_inrush` | saturation of a transformer | 7 | 0 | 0.228 | 2.01 |
| `control_blocks` | 20 control blocks on a circuit of one node | 1 | 20 | 0.150 | 2.00 |
| `pll` | phase-locked loop on a three-phase bus | 9 | 13 | 0.482 | 2.00 |
| `held_injection` | controlled current source in the dq frame | 9 | 9 | 0.471 | 2.00 |
| `converter` | grid-following converter (averaged) | 15 | 39 | 0.701 | 2.00 |
| `converter_voltage_support` | the same with voltage support and a fault | 18 | 43 | 0.651 | 2.00 |
| `synchronous_machine` | machine at an infinite bus (0.71 stage iterations per step) | 13 | 7 | 0.881 | 2.71 |
| `machine_avr_governor` | machine with AVR and governor (1.15 iterations per step) | 9 | 16 | 0.826 | 3.15 |
| `line_energization` | one Bergeron line | 12 | 0 | 0.386 | 2.00 |
| `line_areas` | three parts joined by two lines | 25 | 0 | 0.890 | 2.00 |
| `three_phase_fault` | pi line, transformer, fault (M2) | 29 | 0 | 0.762 | 2.00 |

- **Switching instants within a step.** A step that is split is solved in parts, each with
  its own factorization: the example splits 20 % of its steps and takes 3.4 times the time
  per step of the run at grid points, at a step 10 times longer (0.65 µs per 10 µs against
  0.19 µs per 1 µs: a third of the time for the same simulated time, with an error of the
  inductor current more than 1000 times smaller;
  [numerical-method.md](numerical-method.md#switching-instants-within-a-step)). At the same time step the split run
  took 21 % longer than the one at grid points (M4 stage 1).
- **Nonlinear elements.** A stage is solved again when an element moves to another segment
  (0.01 to 0.3 times per step in the examples); with a change of the matrix, the matrix of
  the element's part is factorized again.
- **Control blocks.** 20 blocks on a circuit of one node take 0.15 µs per step together,
  so a block is of the order of 5 ns. The 39 blocks of a converter with its controlled
  sources take 0.3 to 0.4 µs per converter and step (measured in systems with several
  converters, M4 stage 2).
- **Synchronous machines.** A machine iterates its stage once in most steps (0.7 to 1.2
  additional solutions per step). In the IEEE 39-bus system (built locally from MATPOWER
  case39; numbers of M4 stage 2 for 1 s of simulated time, taken without pinning): 0.23 s
  with the generators as sources, 0.29 s with 9 synchronous machines in the steady state
  and 0.35 s with a fault.

## Changes that must not slow down other models

The rule of the project: a change must not make the reference cases slower by more than 20 %
per step, and models that do not use what it adds by more than 5 %. For the division into
parts (2026-11-22), the build before and the build after were run in turn three times on
one P core; ranges of the three runs, time per step:

| Case (not divided, one thread) | Before | After |
|---|---|---|
| Ring of 10 buses, pi lines, sparse (µs) | 1.60–1.64 | 1.57–1.60 |
| Ring of 100 buses, pi lines, sparse (µs) | 19.68–19.70 | 19.56–19.72 |
| Buck converter, 1 s at 1 µs, dense (ns) | 208–209 | 213–218 |
| `rlc`, dense / sparse (µs) | 0.095–0.100 / 0.145–0.146 | 0.099–0.105 / 0.134–0.137 |
| `rl_breaker`, dense / sparse (µs) | 0.138–0.140 / 0.159–0.169 | 0.137–0.138 / 0.157–0.164 |
| `buck`, dense / sparse (µs) | 0.183–0.191 / 0.275–0.290 | 0.191–0.201 / 0.275–0.286 |
| `rectifier`, dense / sparse (µs) | 0.181–0.183 / 0.234–0.237 | 0.188–0.193 / 0.235–0.245 |
| `three_phase_fault`, dense / sparse (µs) | 1.16–1.17 / 0.772–0.784 | 1.13–1.19 / 0.764–0.802 |

The rings are unchanged. The circuits of 4 unknowns with the dense LU are 2 to 6 % slower
(5 to 10 ns per step of 100 to 200 ns): the factors are reached through the list of parts,
and the stage of a circuit that is one part goes through one more branch. A first version
was 17 % slower for the buck converter (245 ns); a path for a circuit that is one part
without elements that iterate brought it to the numbers above.

Looked at again in M5 (15 runs in turn on one P core, median): the buck converter took 207 ns
per step before the division, 215 ns after it and takes 168 ns now. Half of the loss was a
check of whether the circuit is divided, made for every current inside the loops that collect
the outputs, the measured quantities and the diode currents; it is now made once per row.
The other 1.4 % could not be given to a place: builds whose solver does the same work differ
by 2 to 3.5 ns with where the compiler puts and inlines the functions. Two vectors that were
allocated at every grid point (since before the division) are now kept, which takes 43 ns off
a step of a circuit with diodes: `buck` and `rectifier` (dense) take 0.144 and 0.143 µs,
`rlc` 0.100 µs and `rl_breaker` 0.140 µs (1 to 1.5 % above the build before the division,
within what one measurement moves). The rings and the divided networks are within 1.4 %.
The tables above this section still hold the numbers measured before this change. While
other builds run on the machine, the median of one build moves by a few ns even on a fixed
core: compare builds in turn, ten times or more, when the machine is quiet.

The thyristor (M11) changed the code that decides the states of diodes and controlled
switches and the start of a run. `openkeito run` of 0.13.0 and of the build with the
thyristor, in turn, 7 rounds on one P core, best run time without the output:

| Example | 0.13.0 | With the thyristor | Change |
|---|---|---|---|
| `buck` | 3.008 ms | 3.008 ms | 0.0 % |
| `buck_interpolated` | 1.458 ms | 1.453 ms | −0.3 % |
| `rectifier` | 14.09 ms | 14.27 ms | +1.3 % |
| `rlc` | 0.905 ms | 0.901 ms | −0.5 % |
| `rl_breaker` | 1.049 ms | 1.052 ms | +0.3 % |
| `line_areas` | 4.506 ms | 4.534 ms | +0.6 % |
| `machine_avr_governor` | 58.53 ms | 58.63 ms | +0.2 % |
| `converter` | 4.535 ms | 4.570 ms | +0.8 % |
| `saturable_reactor` | 0.233 ms | 0.233 ms | −0.1 % |
| `three_phase_fault` | 12.27 ms | 12.27 ms | 0.0 % |
| `transition_three_times` | 287.9 ms | 292.0 ms | +1.4 % |

The rows and events of the 23 examples are those of 0.13.0 to the byte.

## WebAssembly (M5)

`openkeito-wasm` (wasm32-unknown-unknown, one thread) under Node 24 against the native
build on the same core; time per step without the output, minimum of 15 runs, µs:

| Example | Unknowns | Native | wasm | Ratio |
|---|---|---|---|---|
| `rlc` | 4 | 0.107 | 0.160 | 1.49 |
| `buck_pi` | 4 | 0.183 | 0.276 | 1.51 |
| `rectifier` | 4 | 0.205 | 0.304 | 1.48 |
| `machine_avr_governor` | 9 | 1.056 | 1.318 | 1.25 |
| `line_areas` | 25 | 1.058 | 1.511 | 1.43 |
| `three_phase_fault` | 29 | 0.888 | 1.609 | 1.81 |

The module is 3.22 MB (2.38 MB after `wasm-opt -O3`, 0.73 MB with gzip). Reading the clock
is a call into JavaScript; a run reads it twice per output row, which is about a fifth of
the whole time of a light model that writes every step.

## Transitions of converted networks, 1 s (M4)

pandapower's case9, case39 and case118 converted by `openkeito.pandapower` in three ways:
with sources (generators as sources, static generators as current injections), with
machines (every generator a `synchronous_machine` with the excitation system and governor
of `openkeito.transition.add_machine_controls`), and with machines and converters (3, 4 and
12 static generators at the largest loads as `grid_following_converter`). Each is run for
1 s in three states: steady as converted; the model of a transition before its change (the
same network with the switches of the load parts and the controlled source of the grid);
and during a change that lasts the whole second (loads from 0.95 to 1.0 of their power in
four parts, generator set points with them, converter powers halved, the grid's internal
voltage moving). 2S-DIRK, bus voltages and the signals of machines and converters recorded
every millisecond, `openkeito run` (release build of 0.4.1 plus M4) pinned to one fast core
(`cmd /c start /b /wait /affinity 400 ...`); best `run_s` of 3 runs, which differ by 1 to
4 % (one run of 9 by more). Date: 2026-11-22.

| Network | Model | Machines | Converters | Unknowns | Control blocks | Unknowns of the transition model | Switches |
|---|---|---|---|---|---|---|---|
| case9 | sources | 0 | 0 | 45 | 0 | 117 | 36 |
| case9 | machines | 2 | 0 | 33 | 50 | 105 | 36 |
| case9 | machines and converters | 2 | 3 | 51 | 50 | 123 | 36 |
| case39 | sources | 0 | 0 | 276 | 0 | 780 | 252 |
| case39 | machines | 9 | 0 | 222 | 225 | 726 | 252 |
| case39 | machines and converters | 9 | 4 | 246 | 225 | 750 | 252 |
| case118 | sources | 0 | 0 | 795 | 0 | 3171 | 1188 |
| case118 | machines | 53 | 0 | 477 | 1325 | 2853 | 1188 |
| case118 | machines and converters | 53 | 12 | 549 | 1325 | 2925 | 1188 |

(The control blocks of the converters are inside the component and not counted.)

Time for 1 s at 50 µs (2·10^4 steps):

| Network | Model | Steady, as converted | Transition model, before the change | During the change | Per step, during | Further solutions per step, during |
|---|---|---|---|---|---|---|
| case9 | sources | 0.031 s | 0.055 s | 0.056 s | 2.8 µs | 0 |
| case9 | machines | 0.046 s | 0.071 s | 0.102 s | 5.1 µs | 1.67 |
| case9 | machines and converters | 0.071 s | 0.100 s | 0.135 s | 6.8 µs | 1.68 |
| case39 | sources | 0.193 s | 0.361 s | 0.361 s | 18.0 µs | 0 |
| case39 | machines | 0.280 s | 0.451 s | 0.646 s | 32.3 µs | 1.68 |
| case39 | machines and converters | 0.296 s | 0.474 s | 0.682 s | 34.1 µs | 1.69 |
| case118 | sources | 0.807 s | 1.488 s | 1.516 s | 75.8 µs | 0 |
| case118 | machines | 1.173 s | 1.923 s | 2.890 s | 144.5 µs | 1.76 |
| case118 | machines and converters | 1.272 s | 2.016 s | 2.923 s | 146.1 µs | 1.77 |

Other time steps (steady as converted / during the change):

| Network | Model | 100 µs | 50 µs | 20 µs |
|---|---|---|---|---|
| case9 | sources | 0.019 / 0.031 s | 0.031 / 0.056 s | 0.068 / 0.129 s |
| case9 | machines | 0.026 / 0.056 s | 0.046 / 0.102 s | 0.101 / 0.170 s |
| case9 | machines and converters | 0.042 / 0.074 s | 0.071 / 0.135 s | 0.163 / 0.239 s |
| case39 | sources | 0.112 / 0.199 s | 0.193 / 0.361 s | 0.438 / 0.878 s |
| case39 | machines | 0.148 / 0.357 s | 0.280 / 0.646 s | 0.589 / 1.440 s |
| case39 | machines and converters | 0.166 / 0.376 s | 0.296 / 0.682 s | 0.692 / 1.522 s |
| case118 | sources | 0.460 / 0.821 s | 0.807 / 1.516 s | 1.916 / 3.652 s |
| case118 | machines | 0.726 / 1.530 s | 1.173 / 2.890 s | 2.911 / 6.637 s |
| case118 | machines and converters | 0.787 / 1.605 s | 1.272 / 2.923 s | 2.982 / 6.866 s |

Parts of a load change (`load_steps`), during the change at 50 µs:

| Network | Model | 4 parts: unknowns, time | 1 part: unknowns, time |
|---|---|---|---|
| case39 | machines | 726, 0.646 s | 348, 0.383 s |
| case39 | machines and converters | 750, 0.682 s | 372, 0.425 s |
| case118 | machines | 2853, 2.890 s | 1071, 2.041 s |
| case118 | machines and converters | 2925, 2.923 s | 1143, 1.870 s |

Output (machines and converters, steady, 50 µs):

| Network | Outputs | Every 20th step: rows, `result.csv`, time of the output | Every step: rows, `result.csv`, time of the output | Whole run: every 20th / every step |
|---|---|---|---|---|
| case9 | 37 | 1001, 1.2 MB, 0.010 s | 20 001, 24 MB, 0.19 s | 0.071 / 0.254 s |
| case39 | 114 | 1001, 4.1 MB, 0.035 s | 20 001, 82 MB, 0.63 s | 0.296 / 0.900 s |
| case118 | 497 | 1001, 15.5 MB, 0.116 s | 20 001, 310 MB, 2.34 s | 1.272 / 3.553 s |

- The time is in the steps: factorizations take under 0.01 s of any of these runs (3 to 8
  per run; a change switches the load parts at 4 instants).
- The switches of the load parts are the larger part of the unknowns of a transition model
  (6 per part and load: three nodes and three switch currents). They are leaves of the
  matrix and cost less than their number says: 4 to 6 times the unknowns of case118 take 1.6 to
  1.9 times the time. With one part per load the change of case118 takes 0.64 to 0.71 of the
  time.
- A machine is solved again within a stage while its operating point moves: 1.7 further
  solutions per step during a change, against 0.003 to 0.08 in the steady state. This is what
  makes a change 1.35 to 1.5 times as slow as the same model before it.
- The time per step depends little on the step: 20 µs takes 1.7 to 2.5 times as long as
  50 µs for 2.5 times the steps (the machines need fewer further solutions at a smaller
  step), 100 µs 0.53 to 0.62 times.
- Per bus and second of a change at 50 µs with machines and converters: 15 ms (case9),
  17.5 ms (case39), 24.8 ms (case118, which has a machine at 53 of its 118 buses).
- Writing every step into `result.csv` takes longer than the computation (66 to 74 % of the
  run). A transition records every 20th step by default.
- This machine has 8 fast and 16 slow cores. The same run (case39, machines and converters,
  during the change) takes 0.68 to 0.71 s on each fast core and 0.97 to 0.99 s on each slow
  one (1.43 times). Runs are independent, so 24 transitions run at once: 8 at the times
  above and 16 at 1.43 times them.
- A transition through three times in one run (case39 with three converters, one
  measurement in M5): the load parts of every level make the model larger, 1248 unknowns
  against 744 for two times, and 1 s takes 1.28 s against 0.68 s. With `load_steps` = 1 it
  takes 0.61 s.
- Accuracy of the same kind of runs:
  [numerical-method.md](numerical-method.md#from-one-power-flow-to-the-next) (start 1e-10 pu
  from the power flow; end 1e-5 pu from the next one at 50 µs).

## Three-phase PWM inverters at large steps (M11)

One second of the two-level PWM inverter of
[numerical-method.md](numerical-method.md#a-three-phase-pwm-inverter-at-large-steps)
(12 two-state elements, carrier 2450 Hz, modulation fixed, 2S-DIRK; test
`table_of_the_errors_of_a_pwm_inverter_against_fine_steps` of `tests/pwm_inverter.rs`, one
P core, 0.06 s simulated with a row every µs, one run each; the time includes the rows):

| h | Within the step, no dead time | Dead time 2 µs | At grid points, no dead time |
|---|---|---|---|
| 1 µs | 0.94 s | 1.11 s | 0.64 s |
| 5 µs | 0.38 s | 0.52 s | 0.22 s |
| 10 µs | 0.31 s | 0.44 s | 0.16 s |
| 20 µs | 0.27 s | 0.42 s | 0.13 s |
| 25 µs | 0.26 s | 0.38 s | 0.12 s |

From 10 µs on the time hardly falls with a larger step: the 2741 events of 60 ms (each a placed
instant with its factorization and trial solution) cost the same at every step. A dead time
shorter than the step adds one instant per edge. Previewing the vertices of the carrier
within a step (0.15.0, which finds the pulses narrower than 25 µs) leaves these times as
they were: at 25 µs 0.253 and 0.388 s before, 0.261 and 0.387 s after (one run each).

## Eight PWM inverters on lines, before parallel switching steps (M11, issue #51)

Eight of these inverters (modulation fixed, carriers shifted by 45° from one to the next),
each with a load of 3.2 Ω and 1 mH per phase on its bus, the buses joined into a ring by
50 km `bergeron_line` (travel time 169 µs), and a 400 V source on the first bus: 86
unknowns, 96 two-state elements, about 21 800 switching events in 60 ms. Test
`pwm_inverters_per_step` of `benchmark.rs`, pinned to the 8 P cores (mask `C03C03`), best of
3 rounds, time per step without the output; "by parts" is the share of the steps whose
parts were solved each on its own (side by side on the threads), every other step is solved
stage by stage over the whole circuit:

| Switching | h | Partitioning (parts, largest) | 1 thread | 2 | 4 | 8 | Steps by parts | Split steps |
|---|---|---|---|---|---|---|---|---|
| `interpolated` | 10 µs | `off` (1, 86) | 80.3 µs | — | — | — | — | 73.6 % |
| | | `auto` (2, 56) | 84.1 µs | 84.5 µs | — | — | 0 % | 73.6 % |
| | | `every_line` (8, 16) | 58.6 µs | 59.0 µs | 58.9 µs | 63.5 µs | 0 % | 73.6 % |
| `interpolated` | 20 µs | `off` (1, 86) | 153 µs | — | — | — | — | 93.7 % |
| | | `every_line` (8, 16) | 110 µs | 110 µs | 110 µs | 110 µs | 0 % | 93.7 % |
| `grid` | 10 µs | `off` (1, 86) | 24.7 µs | — | — | — | — | 0 % |
| | | `every_line` (8, 16) | 10.3 µs | 11.3 µs | 11.5 µs | 11.5 µs | 27.0 % | 0 % |
| `grid` | 20 µs | `off` (1, 86) | 31.4 µs | — | — | — | — | 0 % |
| | | `every_line` (8, 16) | 13.5 µs | 13.9 µs | 14.0 µs | 14.1 µs | 7.1 % | 0 % |

(`auto` keeps four inverters together, up to 48 named nodes, and so has two parts; at 20 µs
it takes 155 µs with 1 or 2 threads.) The rows are the same, to the last bit, for every
number of threads.

- With `interpolated`, **no step** is solved by parts: every step looks for the zero
  crossings of the diodes, and a step that does is solved stage by stage over the whole
  circuit, even when nothing switches in it. Threads therefore bring nothing (8 threads are
  slower than 1 by the cost of waking them). Dividing at every line still pays on one
  thread (58.6 against 80.3 µs at 10 µs, 110 against 153 µs at 20 µs): an action
  factorizes only the matrix of its part, of 16 unknowns.
- With `grid`, a step with an action at its start is solved whole too; with 8 inverters
  nearly every grid point has one, and only 27 % (10 µs) or 7 % (20 µs) of the steps are
  solved by parts. Threads do not help there either.
- What issue #51 would change: each part placing its own instants and looking for its own
  zero crossings, so that a step is solved by parts unless a part itself acts. Here each
  part has about 1/8 of the events. Measured here is only the present state; the speed-up
  of #51 has not been measured.

## Control-load breakdown and thread scaling on a divided network (M10)

Test `control_breakdown_and_threads` in `crates/openkeito-core/tests/benchmark.rs` adds
machines and controls to `blocks(5, 8, "bergeron_line", 0.3)` (5 blocks of 8 buses at
66 kV joined into a ring by 50 km `bergeron_line` links, 219 unknowns, 5 parts of up to 48
unknowns and 5 links when divided, 332 expanded control blocks). Block 0 keeps its 11 kV
`three_phase_source` behind its 100 MVA YNd11 transformer; blocks 1 to 4 replace the
source with a 100 MVA `synchronous_machine` and 15 AVR and governor control blocks each
(`one` and `f_nom` shared); every block adds a 20 MVA `grid_following_converter` at
`b{g}_4` with 16 plant-level voltage/frequency droop control blocks (including a closed Q–V
droop loop). Fault `F1` on `b2_4` runs from 0.1 s to 0.2 s (0.3 s simulated, best of 3
rounds, without output).

Control evaluation time (`control_s`) is measured with `Simulator::with_detail_timing(true)`
(or `OPENKEITO_DETAIL_TIMING=1` with `openkeito run`, which writes
`OPENKEITO_DETAIL_TIMING control_s=... solve_s=... factorize_s=... output_s=... run_s=...`
to standard error without changing `summary.json`). `Per step` is measured with detail
timing disabled (`false`).

One P core (`OPENKEITO_BENCH_SINGLE=1`, mask `400`, Date: 2026-11-25):

| Step (µs) | Partitioning | Parts (largest) | Per step (µs) | `solve_s` | `control_s` | Control / solve (%) | Control loop iter. | Op. point iter. | Departure from steady state |
|---|---|---|---|---|---|---|---|---|---|
| 10 | `off` | 1 (219) | 18.24 | 0.5419 | 0.0695 | 12.8 | 384983 | 58836 | 4.60e-2 |
| 10 | `auto` | 5 (48) | 16.47 | 0.4944 | 0.0691 | 14.0 | 384983 | 233530 | 4.60e-2 |
| 10 | `every_line` | 5 (48) | 16.51 | 0.4934 | 0.0694 | 14.1 | 384983 | 233530 | 4.60e-2 |
| 20 | `off` | 1 (219) | 18.10 | 0.2702 | 0.0349 | 12.9 | 199850 | 29410 | 4.59e-2 |
| 20 | `auto` | 5 (48) | 16.56 | 0.2477 | 0.0351 | 14.2 | 199851 | 117449 | 4.59e-2 |
| 50 | `off` | 1 (219) | 18.23 | 0.1093 | 0.0144 | 13.2 | 88384 | 11839 | 4.55e-2 |
| 50 | `auto` | 5 (48) | 16.71 | 0.1001 | 0.0142 | 14.2 | 88384 | 47256 | 4.55e-2 |

Thread scaling at `time_step_s = 10 µs` on the 8 P cores (mask `C03C03`, 30 000 steps;
`set_threads` applies the automatic `work / 100` thread cap, `use_threads` forces the
requested count up to the number of parts):

| Partitioning | Mode | Asked threads | Actual threads | Parts (largest) | Per step (µs) | `solve_s` | `control_s` | Control / solve (%) |
|---|---|---|---|---|---|---|---|---|
| `off` | `set_threads` | 1 | 1 | 1 (219) | 18.44 | 0.5530 | 0.0708 | 12.8 |
| `auto` | `set_threads` | 1 | 1 | 5 (48) | 16.75 | 0.5013 | 0.0711 | 14.2 |
| `auto` | `set_threads` | 2 | 2 | 5 (48) | 14.58 | 0.4318 | 0.0706 | 16.4 |
| `auto` | `set_threads` | 3 | 3 | 5 (48) | 12.68 | 0.3766 | 0.0731 | 19.4 |
| `auto` | `set_threads` | 4 | 4 | 5 (48) | 11.04 | 0.3339 | 0.0713 | 21.4 |
| `auto` | `set_threads` | 5 | 4 | 5 (48) | 11.07 | 0.3353 | 0.0715 | 21.3 |
| `auto` | `use_threads` | 1 | 1 | 5 (48) | 16.86 | 0.5018 | 0.0704 | 14.0 |
| `auto` | `use_threads` | 2 | 2 | 5 (48) | 14.40 | 0.4338 | 0.0710 | 16.4 |
| `auto` | `use_threads` | 3 | 3 | 5 (48) | 12.43 | 0.3785 | 0.0709 | 18.7 |
| `auto` | `use_threads` | 4 | 4 | 5 (48) | 10.96 | 0.3330 | 0.0737 | 22.1 |
| `auto` | `use_threads` | 5 | 5 | 5 (48) | 10.92 | 0.3272 | 0.0723 | 22.1 |
| `every_line` | `set_threads` | 1 | 1 | 5 (48) | 16.78 | 0.5058 | 0.0714 | 14.1 |
| `every_line` | `set_threads` | 2 | 2 | 5 (48) | 14.38 | 0.4411 | 0.0730 | 16.5 |
| `every_line` | `set_threads` | 3 | 3 | 5 (48) | 12.70 | 0.3759 | 0.0716 | 19.0 |
| `every_line` | `set_threads` | 4 | 4 | 5 (48) | 11.13 | 0.3333 | 0.0718 | 21.5 |
| `every_line` | `set_threads` | 5 | 4 | 5 (48) | 11.08 | 0.3304 | 0.0708 | 21.4 |
| `every_line` | `use_threads` | 1 | 1 | 5 (48) | 16.80 | 0.5078 | 0.0712 | 14.0 |
| `every_line` | `use_threads` | 2 | 2 | 5 (48) | 14.50 | 0.4327 | 0.0713 | 16.5 |
| `every_line` | `use_threads` | 3 | 3 | 5 (48) | 12.64 | 0.3748 | 0.0731 | 19.5 |
| `every_line` | `use_threads` | 4 | 4 | 5 (48) | 11.15 | 0.3356 | 0.0728 | 21.7 |
| `every_line` | `use_threads` | 5 | 5 | 5 (48) | 10.91 | 0.3301 | 0.0709 | 21.5 |

Time-step sweep on the 8 P cores (mask `C03C03`):

| Step (µs) | Partitioning | Mode (threads) | Per step (µs) | `solve_s` | `control_s` | Control / solve (%) | Control loop iter. | Op. point iter. | Departure from steady state |
|---|---|---|---|---|---|---|---|---|---|
| 10 | `off` | `set_threads` (1) | 18.44 | 0.5530 | 0.0708 | 12.8 | 384983 | 58836 | 4.60e-2 |
| 10 | `auto` | `set_threads` (1) | 16.75 | 0.5013 | 0.0711 | 14.2 | 384983 | 233530 | 4.60e-2 |
| 10 | `auto` | `set_threads` (5 → 4) | 11.07 | 0.3353 | 0.0715 | 21.3 | 384983 | 233530 | 4.60e-2 |
| 10 | `auto` | `use_threads` (5) | 10.92 | 0.3272 | 0.0723 | 22.1 | 384983 | 233530 | 4.60e-2 |
| 20 | `off` | `set_threads` (1) | 18.47 | 0.2773 | 0.0362 | 13.0 | 199850 | 29410 | 4.59e-2 |
| 20 | `auto` | `set_threads` (1) | 16.89 | 0.2527 | 0.0354 | 14.0 | 199851 | 117449 | 4.59e-2 |
| 20 | `auto` | `set_threads` (5 → 4) | 11.06 | 0.1718 | 0.0370 | 21.6 | 199851 | 117449 | 4.59e-2 |
| 20 | `auto` | `use_threads` (5) | 10.93 | 0.1659 | 0.0372 | 22.4 | 199851 | 117449 | 4.59e-2 |
| 50 | `off` | `set_threads` (1) | 18.51 | 0.1119 | 0.0150 | 13.4 | 88384 | 11839 | 4.55e-2 |
| 50 | `auto` | `set_threads` (1) | 17.11 | 0.1024 | 0.0150 | 14.7 | 88384 | 47256 | 4.55e-2 |
| 50 | `auto` | `set_threads` (5 → 4) | 11.24 | 0.0679 | 0.0149 | 22.0 | 88384 | 47256 | 4.55e-2 |
| 50 | `auto` | `use_threads` (5) | 11.16 | 0.0657 | 0.0144 | 21.9 | 88384 | 47256 | 4.55e-2 |

- **Control evaluation share.** On one P core at 10 µs, evaluating the 332 expanded control
  blocks (384 983 control-loop iterations over 30 000 steps) takes 0.0691–0.0695 s: 12.8 %
  of `solve_s` undivided (`off`) and 14.0–14.1 % when divided on one thread (`auto` /
  `every_line`).
- **Dividing on one thread.** Splitting the 219-unknown ring into 5 parts of up to 48
  unknowns lowers one-thread `per_step` by 9.5–9.7 % at 10 µs (from 18.24 µs to
  16.47–16.51 µs on `0x400`), because smaller part matrices stay in cache across stages.
  `operating_point_iterations` is counted per part (233 530 across the 4 machine parts
  against 58 836 undivided at 10 µs).
- **Thread scaling and serial bottleneck.** On the 8 P cores at 10 µs, `auto` drops from
  16.75 µs (1 thread) to 11.04 µs (4 threads) and 10.92 µs (5 threads with `use_threads`),
  1.53 times faster than divided 1 thread and 1.69 times faster than undivided (`18.44 µs`).
  Because `controls.evaluate` runs serially on the loop thread (0.0704–0.0737 s), its
  share of `solve_s` rises from 14.0 % at 1 thread to 22.1 % at 4–5 threads. Combined with
  per-stage thread hand-overs across 5 parts, forcing 5 threads (`use_threads(5)`) over the
  automatic cap of 4 (`set_threads(5)`, `work / THREAD_WORK = 4`) gives at most a 1–2 %
  difference (within run-to-run variation).
- **`auto` against `every_line`.** Because the internal lines of each 8-bus block are
  `pi_line`s and each block has 27 named nodes (two blocks exceed the 48-named-node merge
  limit), `auto` and `every_line` make the same 5 parts and run at the same speed.
- **Time steps 10, 20 and 50 µs.** The 50 km `bergeron_line` links have a positive-sequence
  travel time of 169 µs (`>= 50 µs`), so the same 5-part division applies at all three
  steps. `per_step` changes little with the time step (18.10–18.24 µs undivided,
  16.47–16.71 µs divided 1 thread on `0x400`; 10.92–11.16 µs at 5 threads on `0xC03C03`),
  so `solve_s` and `control_s` scale inversely with `time_step_s` while the control share
  remains 13–14 % (1 thread) and 22 % (4–5 threads).

### Result-preserving hot-path cleanups (M10 stage 3)

Four strictly bit-identical changes (Date: 2026-11-25; all 23 example models match the
baseline CSV outputs byte-for-byte):

1. `PartRun::stage` (`crates/openkeito-core/src/sim/parts.rs`): tracks operating-point
   movement with a `bool` during normal iterations and moves component-name collection into
   a cold error helper when `MAX_OPERATING_POINT_ITERATIONS` is reached, removing the
   per-stage `Vec` allocation on stages with operating-point iterations.
2. `SynchronousMachine` (`crates/openkeito-core/src/element/machines.rs`): computes the
   three-phase `(sin, cos)` pairs once per `Eval` (reused across `solve`,
   `update_operating_point`, and `advance`) and shares a single `reduced(stage.alpha)` lookup
   per call.
3. `Sum` and `Product` control blocks (`crates/openkeito-core/src/control/{blocks,nonlinear}.rs`):
   store up to 4 inputs inline (`Inline`) to avoid a secondary heap indirection during
   evaluation.

`control_breakdown_and_threads` at `time_step_s = 10 µs` (best of 3 rounds, pinned):

| Core mask | Partitioning | Mode (threads) | Before: per step (µs) / `solve_s` | After: per step (µs) / `solve_s` | Change |
|---|---|---|---|---|---|
| `0x400` (1 P core) | `off` | `set_threads` (1) | 18.15 / 0.5420 | 17.53 / 0.5286 | −3.4 % / −2.5 % |
| `0x400` (1 P core) | `auto` | `set_threads` (1) | 16.45 / 0.4915 | 15.77 / 0.4753 | −4.1 % / −3.3 % |
| `0x400` (1 P core) | `every_line` | `set_threads` (1) | 16.43 / 0.4967 | 15.74 / 0.4756 | −4.2 % / −4.2 % |
| `0xC03C03` (8 P cores) | `auto` | `set_threads` (4) | 11.12 / 0.3333 | 10.81 / 0.3214 | −2.8 % / −3.6 % |
| `0xC03C03` (8 P cores) | `auto` | `set_threads` (5 → 4) | 11.07 / 0.3305 | 10.73 / 0.3220 | −3.1 % / −2.6 % |
| `0xC03C03` (8 P cores) | `auto` | `use_threads` (5) | 10.74 / 0.3231 | 10.44 / 0.3160 | −2.8 % / −2.2 % |
| `0xC03C03` (8 P cores) | `every_line` | `use_threads` (5) | 10.98 / 0.3285 | 10.53 / 0.3114 | −4.1 % / −5.2 % |

`openkeito run` CLI comparison (one P core, mask `0x400`, interleaved best of 3 runs) and
non-machine parity:

| Example | Before: `run_s` / `solve_s` | After: `run_s` / `solve_s` | Change |
|---|---|---|---|
| `examples/buck.json` | 0.01892 / 0.002740 | 0.01911 / 0.002738 | parity (`solve_s` −0.1 %) |
| `examples/machine_avr_governor.json` | 0.07230 / 0.06918 | 0.06159 / 0.05840 | −14.8 % / −15.6 % |
| `examples/transition_three_times.json` | 0.32966 / 0.31077 | 0.31644 / 0.29728 | −4.0 % / −4.3 % |

Linear-circuit parity (`pi_ring_per_step`, `0x400`): 10 buses `1.734` vs `1.734 µs/step`
after against `1.734–1.744 µs/step` before; 100 buses `21.318 µs/step` after against
`21.302–21.367 µs/step` before.
