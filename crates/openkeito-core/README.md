# openkeito-core

The solver of OpenKeito, an open electromagnetic transient (EMT) simulator for power
systems.

- Modified nodal analysis with a fixed time step; two integration methods of second order,
  the two-stage diagonally implicit Runge-Kutta method (2S-DIRK, L-stable) and the
  trapezoidal rule.
- Linear and piecewise-linear R, L and C, sources, switches, diodes, coupled three-phase
  branches, pi lines and travelling-wave lines, transformers with saturation, faults,
  synchronous machines, an averaged grid-following converter and control blocks.
- Steady-state initialization from a phasor solution, frequency scans, a dense and a sparse
  LU, switching instants within a time step, and the solution of the parts of a circuit that
  travelling-wave lines separate on several threads, with results that do not depend on the
  number of threads.

`Simulator` runs a model of `openkeito-model` and hands its rows to a sink;
`FrequencyScanner` computes the impedances of a frequency scan. The crate writes no files:
`openkeito-cli` is the command `openkeito` and the library that writes the result files and
the summary. The methods, with their references and the tests against closed-form
solutions, are described in the documentation of the repository
(`docs/numerical-method.md`).

The crate also compiles for `wasm32-unknown-unknown` (one thread; the host sets the clock
that the timing reads, see the module `clock`).

Licensed under either of Apache License 2.0 or MIT at your option.
