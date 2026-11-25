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
