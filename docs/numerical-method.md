# Numerical method

## Circuit equations

The circuit is described by modified nodal analysis: the unknowns are the node voltages
(except `gnd`, including the inner nodes of three-phase components) and one branch current
for each voltage source, each time-controlled switch, each ideal transformer and each
faulted phase. Each node contributes Kirchhoff's current law, each voltage source
`V(a) - V(b) = e(t)`, and each time-controlled switch `V(a) - V(b) = 0` (closed) or
`i = 0` (open). Diodes and controlled switches are conductances (see
[Switching elements](#switching-elements)); three-phase components are described in
[Three-phase components](#three-phase-components).

Within a time step these equations are `[[G, Bᵀ], [B, 0]]` in the node voltages and the
branch currents: `G` holds the conductances between the nodes (resistors, and what
inductors, capacitors and lines are within a step), `B` one row per ideal branch, which
is its voltage equation. They have exactly one solution when the rows of `B` are
independent and no voltage pattern other than zero is in the null space of both `G` and
`B`. Validation checks the two conditions on the graph of the circuit, before any matrix
is built. Rows of `B` that depend on each other are a loop of ideal branches (E142):
branches between two nodes are joined into sets of nodes, and a branch whose ends are
already in one set closes a loop; the equations of ideal transformers are then reduced
against each other in the voltages of these sets, and one that reduces to nothing (below
1e-13 of its largest coefficient, the pivot threshold of the solver) closes a loop too.
The null space of `G` is the patterns that are constant on every set of nodes joined by
conductances and zero on the set of gnd, so a set that neither a conductance nor an ideal
branch joins to gnd is held only by the equations of ideal transformers; the sets whose
voltage these leave free (the columns without a pivot in their elimination, and the ones
that depend on them) are the nodes that nothing holds (E143). Each component type says
what it is between its nodes in a given state of its contacts; a type that does not say
counts as conductances between the nodes it connects, so it is never the cause of either
error. The state at t = 0 gives errors. The states that follow from the times of the
switches and faults, at the grid points where they act and up to the last step, give
warnings (W142, W143), because a contact that waits for a current zero opens later than
it is told to. The phasor equations of the steady-state initialization are not checked
this way (E203 reports them): there a converter is a current source and a lossless
resonance can be singular, which no graph shows.

## Integration as one stage formula

For a state `y` (inductor current, capacitor voltage) with derivative `f = dy/dt`, every
stage of the supported methods has the form

```
f = alpha * (y - y_start) - c * f_prev
```

where `y_start` is the state at the start of the step and `f_prev` the derivative
computed by the previous stage.

| Method | Stage | alpha | c | time |
|---|---|---|---|---|
| 2S-DIRK | 1 | 1/(γh) | 0 | t + γh |
| 2S-DIRK | 2 | 1/(γh) | (1-γ)/γ | t + h |
| Trapezoidal | 1 | 2/h | 1 | t + h |
| Backward Euler, half step | 1, 2 | 2/h | 0 | t + h/2, t + h |

with γ = 1 - 1/√2. The 2S-DIRK rows are the two-stage, stiffly accurate, L-stable
diagonally implicit Runge-Kutta method of order 2 (Butcher tableau
`A = [[γ, 0], [1-γ, γ]]`, `b = [1-γ, γ]`). Its stability function is
`R(z) = (1 + (1-2γ) z)/(1 - γ z)^2`, with the error constant
`R(z) - e^z ≈ (3γ² - 2γ³ - 1/6) z³ = 0.0404 z³`.

Substituting the stage formula into the element equations gives a conductance and a
history current source:

| Element | Equation | Conductance | Branch current (a to b) |
|---|---|---|---|
| Inductor | v = L di/dt | 1/(alpha L) | v/(alpha L) + i_start + (c/alpha) f_prev |
| Capacitor | i = C dv/dt | alpha C | alpha C v - C (alpha v_start + c f_prev) |

`alpha` is the same in every stage of a method, so the system matrix is constant and is
factorized once. It is factorized again only when a switch changes state.

## Start of the run

The first output row is the solution at `t = 0+`. It is computed by one backward-Euler
solution over an interval ε of 1e-6 time steps, with the sources at its end ε, which leaves
the states unchanged for the 2S-DIRK method. The sources must be taken at ε: a capacitor
whose voltage a source dictates (connected directly across it) carries C dv/dt, which a
solution with the sources at 0 would give as 0. The row therefore shows the source voltages
at ε (for a sine source, 1e-6 of a step later than 0). Where the initial conditions
contradict the sources (a capacitor at rest directly across a source that is not 0 at
t = 0), the voltage jumps within ε, and the row shows the current of that jump, C·ΔV/ε: the
discrete form of an impulse.

For the trapezoidal rule, which needs the derivatives at `t = 0+`, a first such solution,
with the sources at 0, takes up any jump caused by initial conditions that contradict the
sources and is kept as the state at t = 0; a second one, with the sources at ε and not kept,
yields the derivatives. A wrong derivative here is never damped by the trapezoidal rule: it
alternates in sign from step to step for the whole run (this happened before version 0.3,
when both solutions took the sources at 0, to the current of a capacitor across a sine
source).

Before that solution, time-controlled actions at t = 0 take effect and the diode states
are settled for the initial conditions (as at any grid point with a switching action,
below). The controls are then evaluated on the `t = 0+` solution; if they turn a
controlled switch on, the diode states are settled again and `t = 0+` is solved again.

## Switching elements

- **Time-controlled switch** (`switch`): ideal, with a branch-current unknown. Its state
  is decided at grid points. An action takes effect at the first grid point at or after
  its time, after the output of that grid point. With `open_at_current_zero`, the switch
  opens at the first grid point after the opening command at which the current has
  changed sign (or is negligible: below 1e-9 of its peak since closing, or 1e-12 A). The
  current interrupted is reported in the run summary.
  A switch still waiting for its current zero at the end of the run gives warning W221.
- **Step source** (a `step` waveform): the value of the source is a state, changed like a
  switch at the first grid point at or after `time_s`, after the output of that grid point.
  Read as a function of time, the step would already be seen by the stage that ends at
  that grid point, while the stage before it (2S-DIRK) or the derivative of the last step
  (trapezoidal rule) still belongs to the old value: the response would start 0.29 h
  (2S-DIRK) or 0.5 h (trapezoidal rule) early, an error of first order in h. As an action,
  the step also gets the two backward-Euler half steps of the trapezoidal rule.
- **Diode** (`diode`) and **controlled switch** (`controlled_switch`): a conductance
  `1/on_resistance_ohm` or `1/off_resistance_ohm` (defaults 1e-3 Ω and 1e6 Ω). They are not
  ideal on purpose:
  - With 0 Ω, the trial state at the instant a buck converter's switch turns on (switch and
    freewheeling diode both conducting) shorts the source through two ideal conductors; the
    equations are singular and give no hint which element must change. With a finite
    resistance the trial solution carries a large negative diode current, which says so.
    The same holds for a switch with an antiparallel diode.
  - With an open circuit, the DC side of a diode bridge floats whenever every diode blocks
    (at t = 0 with a sine source, for example), and the equations are singular. A finite
    blocking resistance fixes its potential, as stray capacitances do in reality.
  - The ratio 1e9 of the defaults is well within the dense LU with scaled pivoting
    (threshold 1e-13).
- A controlled switch is on while its gate signal is >= 0.5. The value computed at a grid
  point decides the state over the following step; before the first step the switch is
  off.
- **Thyristor** (`thyristor`): a diode with the same two conductances whose turn-on needs
  its gate. While the gate signal is >= 0.5 the thyristor is *fired*, and a blocking
  thyristor that is fired is contradicted by a positive voltage, as a blocking diode;
  one that is not fired never is. A conducting thyristor is contradicted by a negative
  current whatever the gate, as a diode. Its state is therefore one of the diode states
  below and settled with them. When the gate fires a thyristor whose voltage at the end of
  the last step (or part) was positive, the diode states are settled at once, as after a
  switching action, and the trial solution turns it on. A gate that fires during a
  negative voltage turns it on at the zero crossing of the voltage, if the gate still
  fires it then. No turn-off time and no holding current. At t = 0 the solution before the
  controls are first evaluated is solved again after them (as for every element that
  reads a signal), and its peaks do not count for the thresholds of the diode states.

## Diode states

The states of all switching elements are constant over a time step: both 2S-DIRK stages
use the same matrix. A diode state is judged at the end of a step only, on the
solution of the last stage (the solution at the grid point; the method is stiffly
accurate). The value of the first stage (t + γh) is not judged: when the true switching
instant lies between the two stages, no constant state agrees with both, and judging both
would make the state go back and forth. 2S-DIRK's first stage does not use the derivative
of the previous step (c = 0), so a change of state at a step boundary needs no special
treatment.

At every grid point t_k:

1. A diode contradicted by the solution at t_k (conducting with a current below
   `-ε_i`, or blocking with a voltage above `ε_v`) changes its state now, after the output
   of t_k. Like the time-controlled switch opening at a current zero, this is up to one
   step late; the row at t_k can show the diode current up to `|di/dt|·h` below zero.
2. Time-controlled switches and controlled switches take their actions; thyristors read
   their gate signals (fired, not fired).
3. If any state changed, the diode states are settled: the circuit is solved with one
   backward-Euler step of length 1/alpha (γh for 2S-DIRK, h/2 for the trapezoidal rule)
   that is not kept; the states of inductors and capacitors stay as they are. Every
   contradicted diode is changed and the solution repeated, until no diode is
   contradicted. Backward Euler is used because it damps fast modes without changing their
   sign: when a switch opens while an inductor carries current, the current that is forced
   into blocking resistances shows up as the large voltage it drives, which turns the
   freewheeling diode on. A settling solution with a much shorter interval would make the
   equations ill-conditioned (a capacitor conductance of 1e8 S or more against 1e-6 S of
   blocking resistance on a floating DC side).
4. The step is solved with the regular stages, or, after an action, as described below.

Simultaneous changes (four diodes of a bridge at a zero crossing) are made at once. If a
combination of states comes back within one grid point, the solver changes only the
contradicted diode with the least index from then on (the least-index rule for linear
complementarity problems [Murty]). If a combination comes back again, or after
`20 + 2·(number of diodes)` solutions, the run stops with E202 and names the diodes that
kept changing.

Thresholds: `ε_v` and `ε_i` are 1e-6 of the largest node voltage and the largest current
(sources, switches, diodes) of the run so far, or 1e-14 times the condition estimate of
the LU (the reciprocal of its smallest scaled pivot) if that is larger, and never less than
1e-12 V and 1e-12 A (a circuit that is still at rest has no peaks yet). Peaks of the run
rather than the present solution, because at a commutation every current of the circuit
can be near zero at once; the condition estimate covers rounding in the weakly fixed
potential of a floating part of the circuit. At the slopes of switching circuits these
thresholds move a decision by nanoseconds, far less than a step.

### The step after an action

- **Trapezoidal rule**: the step that follows any action is replaced by two backward-Euler
  half steps, which use the same matrix (critical damping adjustment [Marti, Lin]).
- **2S-DIRK** continues over an action without change: it is L-stable and does not use the
  derivative from before the action. One exception: when a diode stops conducting because
  its current crossed zero (step 1 above), it cuts a current of up to `|di/dt|·h`. If an
  inductor carried it, the current now decays through the blocking resistances, a mode far
  faster than the step (h·R_off/L of 10^4 in a 10 kHz buck converter). The stability
  function of 2S-DIRK tends to zero from below there (`R(z) ≈ (1-2γ)/(γ² z)`), so the mode
  changes sign once and, multiplied by R_off, forward-biases the diode again: the diode
  would conduct and block in turn through the whole idle interval of a converter in
  discontinuous conduction (33 changes per period in the test, and an average output 1.6 %
  too low). That step is therefore solved with two backward-Euler half steps, which damp
  the mode without a change of sign. At a current zero the circuit varies slowly, so the
  first-order error of this one step is small.

## Operating points within a stage

Some elements are linear only around an operating point that depends on the solution: the
segment of a piecewise-linear characteristic, or the speed terms of a machine. Such an
element stamps the conductance and the source of its present operating point. After every
solution of a stage the solver asks each of them whether the solution agrees with it. An
element that disagrees moves its operating point and says whether its conductance changed or
only its source; the solver then factorizes the matrix again (or keeps the factors) and
solves the same stage again, with the same states and history terms. The stage is accepted
when no element moves, so no delay of a step is left between the element and the circuit.
After 50 further solutions of one stage the run stops with E205 and names the elements that
still moved. The run summary counts the further solutions (`operating_point_iterations`,
and the largest number in one stage). A circuit without such elements is solved exactly as
before: the solver keeps a list of them, and nothing is asked when it is empty.

This iteration follows every solution: the stages of a step, the solution at t = 0+, and the
trial solution that settles the diode states, so that the diodes are judged on a solution the
other elements agree with. The last two are not kept, but the operating points they leave
are; an operating point is only the first guess of the next solution, which is iterated in
its turn. The states of diodes and switches are not part of it: they stay constant over a
step and change at grid points, as described above.

Before the run, every element receives the time step, the method and the stages of every
kind of step (their coefficients and their times as fractions of a step). An element with a
delay, such as a travelling-wave line, sizes its history and its interpolation between grid
points from them.

## Piecewise-linear elements

`nonlinear_resistor` and `nonlinear_inductor` (and the magnetizing branch of a transformer
with `saturation`) have a characteristic made of straight segments. In segment k, between two
neighbouring points of the characteristic, such an element is a conductance in parallel with
a current source, like the elements above:

| Element | Equation in segment k | Conductance | Branch current (a to b) |
|---|---|---|---|
| Nonlinear resistor | i = g_k v + j_k | g_k | g_k v + j_k |
| Nonlinear inductor | dλ/dt = v, i = Γ_k λ + j_k | Γ_k/alpha | (Γ_k/alpha) v + Γ_k (λ_start + (c/alpha) f_prev) + j_k |

The state of the inductor is its flux linkage λ, and f = v; the stage formula gives the flux
of a stage as λ_start + (v + c f_prev)/alpha. The characteristic continues beyond its
outermost points with its outermost segments (for an inductor, the air-core inductance). A
characteristic given as symmetric has no point at the origin: the segment through the origin
runs from the mirror image of the first point to the first point, and its current source is
exactly zero.

### Finding the segment

The segment is the operating point of [the iteration above](#operating-points-within-a-stage):
each stage is solved until every such element is in the segment of its own solution, so that
every stage solves the nonlinear equations of the method, not a linearization one step old.

*There is one solution.* Seen from the terminals of the piecewise-linear elements, the rest of
the stage equations is a linear network of positive conductances, ideal transformers and
sources: v = e − R i, with R symmetric and positive semidefinite. With the characteristics
i = f(v), which do not decrease (for an inductor, f of the flux, which grows with v), two
solutions v and v′ would satisfy (v − v′) + R (f(v) − f(v′)) = 0. Multiplied by
(f(v) − f(v′))ᵀ, both terms are ≥ 0, so both are zero, R (f(v) − f(v′)) = 0 and v = v′
[Fujisawa, Kuh; Chua, Lin]. This is why validation requires monotonic characteristics (E261).

*Moving to the segment of the solution does not always end.* With i = v below 1 V,
i = 4v − 3 up to 2 V and i = 0.1v + 4.8 above, behind 1 Ω and 5 V, the first segment gives
2.5 V (third segment) and the third gives 0.18 V (first segment), for ever. Moving one segment
at a time ends, but takes as many solutions as there are segments on the way, each with a new
factorization.

*The search used.* For one element, the stage is the scalar equation
F(x) = x + r f(x) − e = 0 with r ≥ 0, and F grows with x. Solving with the line of segment k
gives the zero x_k of a function F_k that also grows and equals F on segment k. If x_k lies
above the upper end b of the segment, then F(b) = F_k(b) < 0, so the solution of F lies above
b too: every trial tells on which side of its segment the solution is. The element keeps the
range of segments [lo, hi] that can still hold the solution and narrows it with every trial.
The next trial is the segment of x_k (Newton's method, which is exact when the solution is in
the neighbouring segment, the usual case from one step to the next), limited to the range;
after two trials that did not settle it, the middle of the range. A trial never returns to a
segment already excluded, so there is no going back and forth, and the number of additional
solutions is at most 2 + log₂(number of segments). The halving matters for steep
characteristics: down a curve i ∝ v^q Newton's method moves by about v/q per trial, a few
segments of a fine table.

A solution on a breakpoint may be in either segment (both give the same current): x counts
as inside a segment up to 1e-12 of the largest |x| of the table beyond its ends. When a
trial contradicts the range (the solution was above a segment and is now below a higher one),
the element accepts its segment if x is within 1e-6 of the largest |x| of its ends (rounding
of an ill-conditioned solution; the relative threshold of the diode states), and otherwise
opens the range again.

*Several elements.* Every element searches for itself, and all move in the same pass. The
range of one element is then only right while the others stay; when they move, it can
contradict itself and is opened again, which leaves Newton's method on all of them together.
The solution is still unique, but this is not a proof that the search ends: the limit of 50
solutions (E205) guards it. In the coupled circuits of the tests (two resistors on a divider,
three saturating units on an isolated star point, a reactor in series with a diode) no stage
took more than one additional solution.

Additional solutions measured in the tests (`crates/openkeito-core/tests/nonlinear.rs`;
segments counted over both polarities):

| Circuit | Segments | Steps | Additional solutions | Most in one stage |
|---|---|---|---|---|
| Resistor behind a resistor, sine source | 5 | 400 | 16 | 1 |
| Two resistors on one divider, sine source | 5 and 5 | 200 | 16 | 1 |
| Arrester, 10 kA step (tables of 1 %, 0.1 %, 0.01 %) | 35, 105, 325 | 60 | 1, 8, 9 | 1, 8, 9 |
| Arrester, 20 kA front in 8 µs | 105 | 160 | 46 | 9 (the first step) |
| Reactor and resistor, voltage step | 5 | 40 to 640 | 2 | 1 |
| Reactor on a sine voltage, two periods | 5 | 400 | 8 | 1 |
| Transformer inrush, one period | 3 | 1000 | 2 | 1 |
| Three-phase inrush, YNyn0 and Yd1 (isolated star) | 3 × 3 | 1000, 500 | 6 | 1 |
| Rectifier with a saturable reactor, three periods | 5 | 1200 | 12 | 1 |
| Switch closing onto a diode and an arrester | 105 | 20 | 15 | 8 |

A change of segment changes the matrix, which is factorized again (the whole matrix; a
change of the current source alone is not). A fine table therefore costs a factorization
for every segment passed, not more solutions per stage.

### Accuracy at a breakpoint

Each stage is solved exactly for its own segment, so the method integrates dλ/dt = v with the
piecewise-linear i(λ) as it would any nonlinear equation. Within a step that passes a
breakpoint the second derivative of the solution jumps, and a second-order method makes an
error of O(h²) in that step instead of O(h³) (trapezoidal rule: the jump times a(h − a)/2,
with a the time from the start of the step to the breakpoint). A run passes a breakpoint in
a few steps only, so the error stays of second order, but a part of it depends on where in
the step the breakpoints fall. In the test of a reactor charged through a resistor (exact
solution: one exponential per segment, breakpoints at 1.0536 ms and 1.6819 ms), the error of
the current is 1.7e-3 A (2S-DIRK) and 4.8e-3 A (trapezoidal) of 10 A at h = 100 µs; between
h = 200 µs and 12.5 µs it falls with the order 2.2 and 1.9, and from one halving to the next
the order scatters between 1.2 and 3.1. When a source dictates the voltage (a reactor
directly on a voltage source), the flux is the integral of the voltage and the breakpoints
add no error.

### Power-law characteristics

A characteristic given as segments i = p (v/V_ref)^q [IEEE WG 3.4.11] is converted into a
table when the model is read. Each segment gets points at equal ratios of the voltage,
v_k+1 = ρ v_k. The chord of a convex curve lies above it in current, so at equal current its
voltage is lower, by (q − 1)(ln ρ)²/8 of the voltage for small steps. The step is chosen so
that this stays within `relative_voltage_tolerance` (default 1e-3; the exact error of the
chord is checked, not the estimate): ln ρ = √(8·tolerance/(q − 1)), at the default 1.7 % in
voltage for q = 30. The number of points grows as 1/√tolerance; the arrester of the tests (q = 12 and
30, 22 µA to 40 kA) takes 18, 53 and 163 points per polarity for 1e-2, 1e-3 and 1e-4. In
current at equal voltage the error is q times larger. Below the first segment the table is
a straight line through the origin, and above `max_current_a` its last chord continues.
Segments that do not meet where the second one starts are joined by a chord and reported
(W261); a current that would fall there is an error (E263).

### Saturation of transformers

`saturation` replaces the magnetizing inductance of each single-phase unit by a nonlinear
inductor on the side of winding 0. Its points are in per unit of the peak values at the
ratings of that winding: flux linkage on √2·V/ω (the flux at rated voltage) and current on
√2·S/V, so that a slope in per unit is a reactance on the base V²/S; for a three-phase bank V
and S are the winding voltage and a third of the rating. The first segment is the unsaturated
branch: its slope must be 1/b_m of the table in [Transformers](#transformers) within 1 %
(E264), and the solver uses the table. The core-loss resistance stays linear. Hysteresis,
remanence and the coupling of the limbs of a three-limb core are not modelled.

### Steady state and frequency scans

A phasor solution takes each piecewise-linear element as the straight line of its segment
through the origin: the conductance g of a nonlinear resistor (none when the characteristic
has a dead band there: the element is open), the unsaturated inductance 1/Γ of a nonlinear
inductor. A steady state inside that segment is exact, and the run stays on it. If the peak
of the steady-state voltage (resistor) or flux linkage (inductor, |V|/ω) lies outside the
segment, the run starts from a state that is not its steady state, and warning W260 names
the element, the peak and the end of the segment. The flux is then still set to its phasor
value and the segment to the one of that flux. A frequency scan uses the same segment
without a warning: it has no operating point.

A nonlinear resistor with a segment of constant current is an open circuit there (g = 0,
as an open switch). It does not count as a connection to gnd in the check of floating nodes
(E141), because in that segment nothing else would hold the node.

### With diodes

Diode states stay constant over a step and change at grid points; segments are found in
every stage. Where both act, the segments are settled first on every solution that judges
the diodes (the stages, and each trial solution after a change of a diode state), so a diode
is never judged on a solution whose segments are wrong. In the tests, a reactor in series
with a diode (the diode blocks when the flux is back at zero, in the segment of the origin)
agrees with the exact solution within the one step by which a diode switches late, and a
switch closing onto a diode in series with an arrester finds the diode state and the
segment, far up the table, at the same grid point.

## Controls

Control blocks are evaluated once per grid point, in the order of their inputs, after the
circuit has been solved up to that grid point. Measurements read that solution. The outputs
act on the circuit over the next step: the circuit at t_k+1 uses the controls computed at
t_k, one step of delay from the controls to the circuit, and no equation couples them
within a step. An element reads the signals at the start of a step: a controlled switch
takes its state from them and holds it over the step, a controlled source its value (see
[Controlled sources](#controlled-sources)). At t = 0 the signals are read after a first
solution at t = 0+, which is solved again when they change anything. In the other
direction, an element can offer internal quantities (a speed, an angle) as signals named
`<element>.<signal>`, with their values at the grid point; a measurement block reads them.

A block with several outputs gives the signals `<name>.<output>`; the solver keeps one
value per signal.

### Blocks with a state

A block with a state x and x′ = f(x, u) is integrated with the trapezoidal rule from the
last grid point to the present one, x_new = x + h/2·(f(x, u_prev) + f(x_new, u)), whatever
the method of the circuit. The blocks are linear in x, so each solves this in closed form:

| Block | State equation | Step (a = h/2T) |
|---|---|---|
| `integrator` | x′ = K u | x_new = x + K h/2 (u_prev + u) |
| `lag` | T y′ = K u − y | y_new = ((1 − a) y + a K (u_prev + u)) / (1 + a) |
| `lead_lag` | T2 x′ = u − x, y = K (T1/T2 · u + (1 − T1/T2) x) | as the lag, with K = 1 |
| `transfer_function` | x′ = A x + B u, y = C x + D u | x_new = P x + g (u_prev + u) |
| `pi` | I′ = Ki e, y = Kp e + I | I_new = I + Ki h/2 (e_prev + e) |

The transfer function N(s)/D(s) is written in controllable canonical form (A the companion
matrix of D, of the order n of D); P = (I − h/2 A)⁻¹(I + h/2 A) and g = (I − h/2 A)⁻¹ B h/2
are computed once. For a decaying mode the rule multiplies the deviation by
(1 − a)/(1 + a) = e^{−h/T}(1 − (h/T)³/12 …) per step, so a step response is off by at most
(h/T)²/(12 e) of the step; an oscillation of frequency ω comes out lower by (ωh)²/12 of ω.

Limits follow two distinct anti-windup formulations:

1. **IEEE Std 421.5 non-windup integrator and state-limited PI** (`integrator` and `lag`
   with `[min, max]`, and `pi` with `"anti_windup": "state"` in format `"0.7"`): the
   *state itself* is clamped within its limits (`x_new = clamp(x + delta, min, max)` via
   `apply_limits`), so that it leaves a limit as soon as the input drives it back, without
   first unwinding what it would have accumulated beyond the limit. The integrator leaves
   when its input changes sign, the lag when `K u` falls back inside the limit; in the step
   in which that happens the clamped sum is off by at most `|x″| h²/2`. For `pi` with
   `"anti_windup": "state"`, the integral state `I` itself is clamped into `[min, max]`
   (`I_new = clamp(I + delta, min, max)`) regardless of the proportional term `Kp e`, and
   the total output is then clamped as `y = clamp(Kp e + I_new, min, max)`. During a
   sustained error `I` can reach up to the full limit `min` or `max` and stays clamped there
   until the error reverses sign, matching `gain + integrator(minimum, maximum) + sum +
   limiter` cleanly.
2. **Conditional integration** (`pi` default `"anti_windup": "conditional"`, [Åström,
   Hägglund]): freezes the integral state `I` as soon as the *total output*
   `Kp e + I + delta` would move further past `[min, max]`, while clamping the output
   `y = clamp(Kp e + I, min, max)`. Because saturation is judged on the total output, `I`
   stops around `limit − Kp e` rather than reaching `limit`. When the error magnitude later
   diminishes (even before reversing sign), `Kp e + I` drops inside `(min, max)` and
   leaves saturation early, which can yield less integral hold-back and a higher overshoot
   peak on large recovery swings than integrator-state limiting.

In format `"0.7"`, `minimum`/`maximum` on `integrator` (unless `"wrap": true`), `lag` and
`limiter`, and `output_min`/`output_max` on `pi` also accept a control signal `[min(t),
max(t)]` evaluated at each grid point (direct feedthrough for loop checking), preserving
non-windup state clamping (`apply_limits`) and conditional integration; `min` overrides `max`
if signal limits transiently invert `min > max`, and non-finite limits stop the run with
E251.

At t = 0 a lag, a lead-lag and a transfer function start in the steady state of a constant
input: the input at t = 0, or the value given as `initial_input` (0: at rest). For the
transfer function that is A x + B u = 0, which only has a solution when D(0) ≠ 0; with a
pole at s = 0 the states start at 0. An integrator and the integral of a PI block start at a
given number or at the value that another signal has at t = 0. If `pi`'s `initial_integral`
starts outside `[output_min, output_max]`, the initial integral state itself is clamped
into `[output_min, output_max]` at t = 0 (matching `integrator` and `lag`, which clamp
their t = 0 state into `[minimum, maximum]`), preventing wind-up before moving off the
limit. A `pll` starts locked. An `initial_value` block keeps the value its input had at
t = 0, which writes a set point as "what the quantity is now".

The `piecewise_linear` block (`"0.7"`) has no state (direct feedthrough) and interpolates
linearly between adjacent breakpoints `(x[k], y[k])`, holding `y[0]` for `u <= x[0]` and
`y[last]` for `u >= x[last]` (a single-point table holds `y[0]` everywhere). The
`expression` block (`"0.8"`) also has no state (direct feedthrough): its expression string is
parsed and compiled once before the run and evaluated at each grid point on its inputs,
stopping with E251 if the result is not finite. The `delay` keeps its input at the last ⌊d/h⌋ + 1 grid points and joins the two around t − d
by a straight line: for d = (k + r) h the error is at most r (1 − r)/2 · h² |u″| (the chord
of the input), and none for a whole number of steps. A delay shorter than the step
interpolates between the previous and the present input, so its output depends on the
present input. The `rate_limiter` moves its output towards the input by at most rate × h per
step.

### Loops of blocks

A loop of blocks is allowed when it passes through an input on which the output does not
depend at the same instant: the input of an integrator, of a lag, of a transfer function
whose numerator has a lower order than its denominator, of a delay of at least one step. A
loop of blocks that all depend on their present input—including a loop passing through
`piecewise_linear`, `expression` or through a signal-name limit on `integrator`, `lag`, `pi` or `limiter`—has no order
of evaluation and is rejected (E171).

The blocks are grouped into strongly connected components [Tarjan], which are evaluated in
the order of their inputs. A component with a loop is cut at inputs that are not direct, as
few as its order needs, and its blocks are evaluated in turn, each with the latest outputs of
the others, until no output changes by more than 1e-12 of the largest output of the loop.
In the first pass the block after the cut reads the input of the last grid point, so that
pass is a forward-Euler prediction; the passes that follow are fixed-point iterations of
the trapezoidal equations of all the blocks of the loop together. What they converge to is
the trapezoidal rule for the loop as a whole: second order, and stable for every stable
loop. Each pass multiplies the error by the gain around the loop within one step, about
h/2 times the gain of the loop in 1/s (for K/(1 + sT) followed by 1/s: (h/2T)·K·h/2), so
they converge when that is below 1 and in three or four passes at the steps that resolve
the loop. After 100 passes the run stops with E250; the run summary counts the passes
beyond the first of each grid point (`control_loop_iterations`). A delay of at least one
step in the loop makes its output known before the pass, and the loop settles in two
passes.

A block computes its new state from the state of the last grid point and keeps it aside
while the loop settles; the states are replaced once, after the grid point is settled.

A signal that is not a finite number (a division by 0 without `minimum_denominator`, the
square root of a negative number, a non-finite result from `expression`, or a non-finite
signal limit) stops the run with E251.

### Rotating frame and phase-locked loop

With θ the angle of the d axis ahead of the axis of phase a and the q axis 90° ahead of the
d axis [Kundur; Park]:

```
d    =  2/3 (a cos θ + b cos(θ − 2π/3) + c cos(θ + 2π/3))
q    = −2/3 (a sin θ + b sin(θ − 2π/3) + c sin(θ + 2π/3))
zero =  1/3 (a + b + c)

a = d cos θ − q sin θ + zero,    b, c: with θ − 2π/3 and θ + 2π/3
```

or d + jq = 2/3 (a + b e^{j2π/3} + c e^{−j2π/3}) e^{−jθ}. The factor 2/3 keeps amplitudes.
For balanced phases with the phasor X of phase a (rms, cosine reference, sequence a, b, c:
x_a = √2 |X| cos(ωt + ∠X)) and θ = ωt + θ0,

```
d + jq = √2 · X · e^{−jθ0},
```

constant in time: with θ0 = 0 the d axis is the cosine reference of the phasors, d is the
peak of the in-phase part and q that of the part that leads by 90°. A negative-sequence
component appears at 2ω, a zero-sequence component only in `zero`.

The `pll` is the phase-locked loop in the synchronous reference frame [Kaura, Blasko;
Chung]: the inputs are transformed with the loop's own angle θ, and

```
e = q(θ) / rated_amplitude,   I′ = Ki e,   ω = ω0 + Kp e + I,   θ′ = ω.
```

For inputs of rated amplitude at the angle φ, e = sin(φ − θ); for a small error ε = φ − θ
the loop is ε″ + Kp ε′ + Ki ε = φ″, so ε follows s²/(s² + Kp s + Ki): natural frequency
√Ki, damping Kp/(2√Ki). Both states are integrated with the trapezoidal rule. The present e
depends on the present θ, which leaves one equation for θ,
θ = θ_prev + h/2 (ω_prev + ω0 + I_prev + Ki h/2 · e_prev + (Kp + Ki h/2) e(θ)), solved by
Newton's method with de/dθ = −d/rated_amplitude (two or three steps; it has one solution
while h/2 (Kp + Ki h/2) times the amplitude in per unit is below 1). The same loop built
from `abc_to_dq0`, `pi` and an `integrator` folded into [0, 2π) is a loop of blocks, iterated
as above, and gives the same values (to 1e-11). At t = 0 the loop starts at the angle of
the space vector of its inputs, atan2(β, α), unless an angle is given.

### Controlled sources

A controlled source takes its value from a signal. The signal computed at the grid point
t_k is the first that the step from t_k to t_k+1 can use, so the source is one step late.
Over that step it moves in a straight line from u_k−1 to u_k:

```
source(t) = û(t − h),   û: the signal joined by straight lines between grid points.
```

The source is then continuous, each stage of a step sees the value of its own time, and
what remains against u(t − h) is the chord of the signal, at most h²|u″|/8 between grid
points and nothing at them. Both methods keep their second order: for R = 1 Ω and
L = 10 mH driven by 10 V at 50 Hz with h = 100 µs the current is within 3.4e-4 A of the
closed-form response to the delayed sine, and within 8.5e-5 A at half the step. Holding the
value over the step instead (source = u_k from t_k on) is the same for the trapezoidal
rule, which looks at grid points only, but the first stage of 2S-DIRK at t_k + 0.29 h then
sees a source that is 0.71 h ahead of the straight line: 5.1e-2 A in the same case, and
half of that at half the step (first order).

The step after a change of the value is solved with the regular stages. A value that
follows a signal changes at every step; treating each change as a switching action, with
the two backward-Euler half steps that follow one under the trapezoidal rule, would turn
the whole run into backward Euler (2.4e-2 A in the case above, first order). A jump of the
signal reaches the circuit as a ramp over one step, which both methods integrate to second
order (the current of the circuit above after a step of 10 V: 8.3e-5 A off for the
trapezoidal rule, 4.0e-5 A for 2S-DIRK). As with any source, a quantity that the source
dictates directly (the current of a capacitor across a controlled voltage source, the
voltage of an inductor in series with a controlled current source) rings after such a ramp
under the trapezoidal rule; 2S-DIRK damps it, and an impedance between the source and the
element avoids it.

The source takes its value from the time within the step, not from the stage, and reads its
signal at grid points only: in a step that is solved in parts
([Switching instants within a step](#switching-instants-within-a-step)) it stays on the
same straight line, and a previewed signal does not move it. A run with a PWM-driven switch
beside the circuit above, which splits a quarter of the steps, differs from the same run
with a plain source of the delayed sine by less than the chord, (ωh)²/8 of the amplitude.

Before t = 0 the signal is taken to have had its value at t = 0: the source has that value
at t = 0 and over the first step. With the delay, a loop from the circuit through a
measurement, the blocks and a controlled source is the delay equation of the continuous
loop with the dead time h. A PI controller of the current of R and L in series whose zero
cancels the pole of the circuit (crossover ωc = 500 rad/s, h = 100 µs) is within 8e-5 of the
solution of L i′ + R i = v(t − h) (1.8e-5 at half the step), and 1.3e-2 = 0.27 ωc h away from
the response 1 − e^{−ωc t} of the loop without a delay.

In the steady-state initialization a controlled source is the phasor given as
`steady_state` (0 without it, warning W240). The run then starts with the source at
√2·Re of that phasor, and over the first step the source moves from there to the signal of
t = 0. For the run to stay in the steady state the signal must describe the source one step
ahead, u_k = source(t_k+1): a set point in the rotating frame is turned back into phase
quantities with the angle θ + ωh. With that, a current injection held by a PLL and
`dq0_to_abc` departs from the phasor solution by 1.9e-5 of the amplitudes at h = 50 µs
(4.8e-6 at half the step, and no more after 1 s than after 0.1 s): the chords of the
source, (ωh)²/8 at most, which the first stage of 2S-DIRK sees. Without the angle of one
step the same run departs by ωh = 1.6e-2 of the injected current.

The control blocks are not part of the phasor solution (W183): they start from their own
initial values, evaluated on the circuit's steady state at t = 0.

### Averaged converter

`grid_following_converter` is a three-phase voltage source behind its reactor (one
controlled source per phase and a coupled RL branch, with inner nodes between them) and a
set of control blocks. The model crate writes the blocks (`expanded`), named after the
component, and the solver runs them with the blocks of the model: nothing in the time loop
knows of converters. The switching of the bridge is averaged away: the source has the
voltage that the modulation would give over a carrier period [Yazdani, Iravani; Mohan,
Undeland, Robbins], and the dc voltage enters only as the largest voltage the source can
have.

The control works in per unit of the ratings, in the frame of a phase-locked loop on the
terminal voltage [Kaura, Blasko; Chung], whose d axis is the voltage:

```
measured:    v_dq (the loop's own d, q), i_dq = abc_to_dq0(i, θ),   p = v_d i_d + v_q i_q
references:  i_d,cmd = P_ref / max(v_f, v_min),  i_q,cmd = −Q_ref / max(v_f, v_min),
             v_f = v_d / (1 + s T_v)
limit:       |(i_d, i_q)| ≤ I_max; the current with priority is clamped to ±I_max and the
             other one to ±√(I_max² − it²)                                  [WECC, REEC_A]
control:     e_d = v_d − x i_q + Kp ε_d + Ki ∫ε_d dt,   ε = i_ref − i
             e_q = v_q + x i_d + Kp ε_q + Ki ∫ε_q dt,   |(e_d, e_q)| ≤ m V_dc / 2
source:      e_abc = dq0_to_abc(e, θ + ω h)
```

With the reactor (x/ω) di/dt + r i = e − v in per unit, the feedforward of v and the terms
±x i leave (x/ω) di/dt + r i = Kp ε + Ki ∫ε on each axis, and Kp = x/(ω τ), Ki = r/τ make
the open loop 1/(τ s): the closed loop is 1/(τ s + 1) [Yazdani, Iravani, chapter 8]. Two
blocks were added for it. `dq_limiter` is the limit of a vector with a priority. `pi_dq` is
the pair of PI controllers with the feedforward and a common limit of the magnitude of
their output: the output vector is scaled back to the limit, and both integrals are held
while the vector is beyond the limit and their step has a component along it (the vector
form of the conditional integration of the `pi` block [Åström, Hägglund]). The current
limit acts on the references before the controllers and has no state, so nothing winds up
behind it.

**Defaults.** A model that gives only the ratings and the reference runs with typical
values: a reactor of 0.15 per unit with X/R = 30 (0.1 to 0.25 per unit for the interface
reactor of a two-level converter) and τ = 1 ms (0.5 to 5 ms) [Yazdani, Iravani, chapter 8];
a phase-locked loop with a natural frequency of 20 Hz and a damping of 0.707 [Chung]; a
current limit of 1.1 per unit (1.1 to 1.3) with priority of the reactive current and a lag
of 20 ms on the voltage that the powers are divided by (I_max, PQ flag and T_rv of REEC_A)
[WECC]; a voltage limit of 1.2 per unit, which is a dc voltage of 1.04 times the peak
line-to-line voltage with third-harmonic injection [Mohan, Undeland, Robbins]. They are
meant as a starting point, not as the data of a product.

**Dead time.** The blocks are evaluated at grid points and reach the circuit one step
later ([Controlled sources](#controlled-sources)). Two things follow.

A steady state must survive it: the voltage reference of the grid point t_k is what the
source has at t_k+1, so it is turned back into phase quantities with the angle θ + ω h. The
source then lies on the sine at every grid point and on its chords between them.

The current loop becomes τ s + e^{−s h} = 0, or with λ = s τ and ρ = h/τ: λ + e^{−λ ρ} = 0.
Its roots are real for ρ ≤ 1/e = 0.37 (no overshoot), a damped pair up to ρ = π/2, and
unstable beyond; the phase margin is 90° − 57.3° ρ. For ρ ≤ 0.2 the response differs from
1/(τ s + 1) by the delay alone: late by between h and 2h (the step of the reference reaches
the source one step later, and the source takes one more step to get there in a straight
line), which is at most (2ρ/e) of a step of the reference where the lag is steepest.
Validation warns above ρ = 0.36 (W290). The decoupling terms ±x i use the current of the
grid point before: while one current moves at the rate Δ/τ the other axis sees the voltage
x Δ h/τ for about τ, and moves by Δ ω h/e. Both effects are of first order in h; the
integration of the circuit itself stays of second order. Measured for a step of 0.6 per
unit with τ = 1 ms (`a_current_step_follows_the_sampled_loop_and_the_first_order_lag`):

| h | against the loop with its dead time, written by hand | against 1/(τ s + 1) | largest i_q |
|---|---|---|---|
| 100 µs | 2.0e-5 (trapezoidal 1.7e-4) | 2.7e-2 | 7.7e-3 |
| 50 µs | 5.0e-6 (4.1e-5) | 1.4e-2 | 3.6e-3 |
| 25 µs | 1.3e-6 (1.0e-5) | 7.3e-3 | 1.8e-3 |

**Slow mode.** The cancelled pole stays in the response to anything but the reference. A
change of the operating point by Δi needs r Δi more in the integral; until it is there the
current is off by the deficit over Kp, r Δi/Kp = (r ω τ/x) Δi (1 % for the defaults), which
decays with L/R = x/(ω r) (95 ms). The same holds for what the integrals did not take while
the voltage limit held them.

**Steady state.** In the phasor solution each phase of the source is a current source of
the operating point, I = conj(S/(3 V)) with the powers and the bus voltage that the model
gives (those of a power flow): the row of the branch current of the source reads I_k = −I
in place of V_a − V_b = E. The controls of a converter hold its current, so this is the
steady state they would reach; a voltage phasor computed beforehand from an assumed terminal
voltage would turn every error of the assumption into a current through the small
reactance (1 % of voltage is 7 % of current at x = 0.15). The voltage behind the reactor
follows from the solution and is where the source starts; with all sources at zero (a
frequency scan) the converter is an open circuit. At t = 0 the blocks start on that
solution: the phase-locked loop locked on the terminal voltage, the lags in the steady
state of their inputs, and the integrals of the current control at r i_d and r i_q
(`initial_integral` as signals), the voltage across the resistance, which is all the
controller adds to the feedforward in a steady state. The run then stays where the power
flow put it. What remains is the chord of the source, (ω h)²/8 of its voltage, which the
integrals take up within L/R: for short-circuit ratios of 20 and 3 the powers stay within
2.5e-5 per unit of the operating point at 50 µs (a quarter of that at 25 µs) and within
2e-7 after 0.5 s, and the voltage keeps an offset of 5e-6 to 1.2e-5
(`a_run_from_a_power_flow_stays_at_its_operating_point`).

The reactor has 1000 times its impedance for the zero sequence: a converter with three
wires carries none.

**Against a switching bridge.** The same blocks drive a two-level bridge of six
`controlled_switch` elements with a carrier of 5 kHz, with the switching instants within
the steps (`the_averaged_model_gives_the_fundamental_of_a_switching_bridge`). Row by row
its currents differ from those of the averaged model by the ripple of the carrier, 0.065
per unit against the estimate (V_dc/2) ω/(8 x f_c) = 0.063; averaged over a carrier period
they differ by 8e-3 through a step of the current reference from 0.3 to 0.8 per unit.

**Faults.** With a three-phase fault at the terminals (0.35 per unit of voltage left), the
current reference is at its limit and the current within 2.4e-4 of it. A jump of the
voltage reaches the current for the one step before the feedforward follows, Δv ω h/x at
most (0.07 for a jump of 0.65 per unit at 50 µs): the current stays below 1.114 for a limit
of 1.1. A lag in the feedforward (`voltage_feedforward_time_constant_s`) lets the jump
through for its time constant instead (1.58 per unit when the fault starts, with 1 ms), so
the default is none. Runs at 100, 50 and 25 µs converge to the one at 12.5 µs with a factor
of 2.4 to 3.6 per halving (`the_fault_response_converges_with_the_time_step`).

## Three-phase components

Three-phase components and transformers are expanded into elements when the circuit is
built. Their inner nodes are numbered but not named in the model (error messages call them
`V(LINE1:1.a)`, with a `:` that node names cannot contain). Every recorded current is a
linear combination of element currents, such as the current into a pi line at its sending
end (series branch plus shunt capacitance) or the line current of a delta winding (the
difference of two winding currents).

Conventions: phasors are rms with a cosine reference, x(t) = √2·|X|·cos(ωt + ∠X); the phase
sequence is a, b, c; symmetrical components use a = e^{j120°} and
[X0, X1, X2] = (1/3)[[1, 1, 1], [1, a, a²], [1, a², a]]·[Xa, Xb, Xc] [Fortescue].

### Coupled RL branches

n branches (n ≤ 3) with v = R i + L di/dt, R and L symmetric n×n matrices, L positive
definite. With the stage formula di/dt = α(i − i_start) − c·f_prev,

```
(R + α L) i = v + L (α i_start + c f_prev)
i = G v + h,   G = (R + α L)^-1,   h = G L (α i_start + c f_prev)
```

G is a conductance matrix between the two node sets and h a history current per branch;
G and H = G·L are computed once for each α of the run (regular stages, backward-Euler half
steps, the negligible step at t = 0+). The derivative for the next stage is
f = α(i − i_start) − c·f_prev, which equals L⁻¹(v − R i). A branch with n = 1 is a series RL.
Positive- and zero-sequence values give the matrix with self value (Z0 + 2Z1)/3 and mutual
value (Z0 − Z1)/3, whose eigenvalues are Z0 (once) and Z1 (twice).

A zero-sequence impedance k times the positive-sequence one (k of 1e6 for a branch without
a zero-sequence path) makes the entries of G times those of L k times the entries of
G·L, so the product loses log10 k digits, and h with it. In a regular stage that is a
relative error of k·1e-16 of the current. In the step at t = 0+, whose α is 1e6 times
larger, G is 1e6 times smaller and the same error of h is a voltage error 1e6 times larger:
with G·L, k = 1e6 moved the voltages at t = 0+ by 1e-4 and k = 1e8 by 1e-2. Where the
product loses more than two digits and αL is above R, G·L is therefore computed as
(I − G R)/α, which is the same matrix: there G R is the small part, and what it loses does
not reach h. The voltages at t = 0+ then do not depend on k
(`a_large_zero_sequence_ratio_leaves_the_start_steady`). Ordinary ratios keep the product.

R + αL must be regular for every α of the run, and these span many orders of magnitude:
2/h for a backward-Euler half step, 3.41/h for a regular stage of 2S-DIRK, 1e6/h at
t = 0+ and, with switching instants within the step, up to 3.41e6/h for the shortest part
of a split step. A branch whose resistances and inductances have different ratios between
the sequences is governed by R at a small α and by L at a large one, so a matrix that is
regular at one α can be singular to rounding at another. Validation therefore looks at
the pivots of R + αL for all of these coefficients (`Simulation::stage_coefficients`, the
same numbers as the solver's stages) and reports the smallest as E139, with the α at which
it fails; its threshold is 100 times stricter than that of the run. What remains for the
run (E204) is a branch given by matrices that is singular only at an α between the two
ends of a split step: for sequence values the ratio is monotonic in α, so the ends
suffice. Over 396 models (zero- to positive-sequence inductance ratios of 1e9 to 1e17, both
methods, both kinds of switching, three time steps) validation refuses the model or the
run completes (`tests/diagnostics.rs`).

A nodal capacitance matrix C (i = C dv/dt) is treated in the same way: conductance αC and
history current C(α v_start + c f_prev).

The source `three_phase_source` is three ideal voltage sources between inner nodes and the
star point, in series with a coupled RL branch to the bus.

### Pi lines

A line of length ℓ with `sections` = N is N cascaded pi sections: each has the coupled series
R and L of ℓ/N and half of its shunt capacitance matrix at each end; the halves of adjacent
sections add up. For the positive sequence of an open line, the receiving-end voltage is
V_s/A with A the first entry of the product of the section ABCD matrices
[[1 + ZY/2, Z], [Y(1 + ZY/4), 1 + ZY/2]]; for the distributed line A = cosh(γℓ),
γ = √(z y) [Grainger, Stevenson]. The lumped line approaches the distributed one with an
error that falls as 1/N² (measured in the tests).

### Travelling-wave lines

`bergeron_line` is the line with distributed constant parameters of [Dommel 1969]. It is
one element with no inner nodes and no state that is integrated.

**Modes.** The phase voltages and currents are written in three modes that do not couple,
v_mode = W·v_phase and i_phase = Wᵀ·i_mode (rows of W of length 1; power is the same in
both). For sequence values W is the Clarke transformation,
rows [1, 1, 1]/√3, [2, −1, −1]/√6 and [0, 1, −1]/√2: the zero-sequence mode with
(R0, L0, C0) and two line modes with (R1, L1, C1) [Clarke]. For matrices, with the
Cholesky factor L = S·Sᵀ and the eigenvalues of the symmetric matrix Sᵀ·C·S = Q·Λ·Qᵀ
(Jacobi rotations [Golub, Van Loan]), the rows of W = Qᵀ·S⁻¹ are left eigenvectors of L·C:
W·L·Wᵀ = 1 and W⁻ᵀ·C·W⁻¹ = Λ. L and C are symmetric and positive definite (validated), so
the modes are real for every line. Modes of equal velocity are turned within their plane
so that the resistance W·R·Wᵀ is diagonal there too, which makes a balanced matrix give
the values of the Clarke transformation (tested). Between modes of different velocity the
resistance keeps mutual terms, which the model drops: validation measures the largest
|R_jk|/√(R_jj·R_kk), warns above 0.01 (W231) and refuses the line above 0.1 (E231).

**One mode.** A mode with L′, C′, R′ per km and length ℓ is a lossless line with
Z_c = √(L′/C′) and travel time τ = ℓ·√(L′·C′), cut in the middle, with R = R′·ℓ lumped in
series: R/4 at each end and R/2 in the middle [Dommel, EMTP Theory Book, chapter 4].
Eliminating the middle gives, for an end e with the other end o (currents into the line),

```
i_e(t) = (v_e(t) − E_e(t)) / z,          z = Z_c + R/4,   ρ = (Z_c − R/4)/z
E_e(t) = (1 + ρ)/2 · a_o(t − τ) + (1 − ρ)/2 · a_e(t − τ)
a(t)   = v(t) + ρ z i(t) = (1 + ρ) v(t) − ρ E(t)
```

To the circuit an end is the conductance matrix G = Wᵀ·diag(1/z)·W from its phase nodes
to gnd and the known current Wᵀ·(E/z). G does not depend on the stage coefficient, so the
line adds no factorization, and it has no entry between the two ends: parts of a circuit
that only such lines join do not couple within a time step (the element reports the
unknowns of its two ends for the solver that will use this). The check that every node
reaches gnd (E141) therefore counts every node of the line as connected to gnd and the two
ends as not connected to each other.

Against the distributed line, whose open-end ratio is A = cosh(γℓ), the lumped resistance
changes A by ρ₀²/8·(cos θ − sin θ/θ + sin²(θ/2)) with ρ₀ = R/Z_c and θ = ωτ, which is
ρ₀²·θ²/96 for a short line (1.2e-6 for 300 km at 50 Hz in the tests, 1.3e-4 for 1000 km).
Both are functions of Z·Y that agree up to the first order in R; the term is the
difference of their second-order terms. Validation warns when R exceeds Z_c (W230).

**History.** The stages of a step are solved at t + γh and t + h (2S-DIRK), t + h
(trapezoidal rule), t + h/2 and t + h (the half steps after an action), and the solutions
at t = 0+ and the trial solution of the diode states at their own times; each needs a_o
and a_e one travel time earlier. The element keeps a of both ends from the solution of
every stage of every step: the grid points and the stages between them. A value between
two samples is their linear interpolation.

- A travel time of a whole number of steps (the quotient τ/h within 1e-9 of it) reads the
  sample of the same stage of an earlier step: no interpolation. A line between sources
  and resistors then follows the lattice diagram to rounding with either method.
- Otherwise, between samples a distance d·h apart (d = 1 with the trapezoidal rule; γ and
  1 − γ with 2S-DIRK) at the place w of that distance, the error is
  w(1 − w)/2·(d·h)²·a″: second order. A sine of angular frequency ω is multiplied by
  1 − w(1 − w)(ω·d·h)²/2 each time it is read: a loss of at most (ωh)²/8 per travel
  time. The weights are positive, so no frequency is amplified whatever the terminations
  (a lossless line between an ideal source and an open end stays bounded); interpolation
  of higher order does not have this property, which is why it is linear. The price is
  that fast components die out and that a front is spread over one more step each time it
  is read. A time step that divides the travel times avoids both.
- Before t = 0 the history is not stored: it is zero, or the sine of the steady state
  (below), evaluated at the time asked for. The solution at t = 0+ is the sample of t = 0.
  The two are kept apart, so a wave that starts at t = 0 arrives whole in the row of the
  first grid point at or after τ, also when τ is between grid points.
- Every reading must lie at or before the start of the step being solved: τ ≥ h for every
  mode. Then the stage at the end of a step reads at most the grid point at its start, and
  earlier stages read before it. Validation refuses a shorter line (E230, which names the
  travel times, `pi_line` and the largest time step).
- Solutions that are not kept do not move the history: the trial solution of the diode
  states only reads it, and t = 0+ solved again replaces the sample of t = 0. `save` and
  `restore` give back the history of the start of a step.
- A switching action (at a grid point after its row, or at an instant within a split
  step) leaves two samples at its time: the solution before it, and the value just after
  it. The solver tells every element that an action took effect; the value just after it
  is on the straight line through the first two solutions that follow (at t + γh and
  t + h with 2S-DIRK), which is exact for a wave that jumps and is then level, and of
  second order for one that starts to rise from the old value (a source behind an
  inductance). Readings up to the time of the action get the first sample, later ones
  the second: a front is not spread over the stage after the action, and with a whole
  number of steps the far end has the wave in the row of t + τ, as for a wave that starts
  at t = 0.
- In a step that is split at switching instants the stages lie at the times of the parts;
  the element is told where each part lies in the step, keeps their solutions as samples
  like any others and reads each stage on its own (the history terms of a regular step
  are computed together, when the step before it ends).

**Phasors.** With a(t − τ) → e^{−jωτ}·A the same equations give, per mode with
p = e^{−jωτ}, the admittance (1 − p)/((1 + pρ)·z) to equal voltages at both ends and
(1 + pρ)/((1 − pρ²)·z) to opposite ones; half their sum and half their difference are the
entries of the two-port, which is that of the cascade R/4, half line, R/2, half line, R/4
(to rounding in the tests) and j·tan and −j·cot of θ/2 without losses. The steady-state
initialization and the frequency scan use it. A lossless line is singular where
1 + p = 0 or 1 − p = 0. For a run that starts from the steady state the element keeps the
phasors of a, and the history before t = 0 is √2·Re(A·e^{jωt}); the solution at t = 0+,
whose sources are 1e-6 of a step late, is taken back to t = 0 along that sine. With a
whole number of steps and no other states, the run then stays on the steady state to
rounding (2e-14 in the test); otherwise it departs by the interpolation error above and
the error of the method in the rest of the circuit.

### Transformers

A single-phase unit is the T equivalent circuit: an ideal transformer with turns ratio
n = V_0/V_1 (rated winding voltages), half of the leakage impedance in series with each
winding, and the magnetizing branch (R_m ∥ L_m) across the ideal transformer on the side of
winding 0. On the base Z_b = V²/S of each winding:

| Quantity | Value (per unit) |
|---|---|
| Leakage, each half | (u_r + j·√(u_k² − u_r²))/2 |
| Magnetizing admittance | y_m = i_0, g_m = P_fe/S, b_m = √(y_m² − g_m²) |

u_k, u_r and i_0 are `impedance_percent`, `resistance_percent` and
`no_load_current_percent` divided by 100. The ideal transformer is written with its winding
current i as an extra unknown:

```
V(p1) − V(q1) − n (V(p2) − V(q2)) = 0
current i into p1 and out of q1, −n i into p2 and out of q2
```

A coupled-inductor matrix of the windings is not used: with a small magnetizing current it
is nearly singular, and without one it does not exist.

A three-phase transformer is a bank of three units (rating S/3, winding voltage V_LL/√3 for
star and V_LL for delta); the zero-sequence coupling of a three-limb core is not modelled, and
saturation is optional (see [Saturation of transformers](#saturation-of-transformers)). The unit of phase p has its windings between the ends below (next(a) = b,
prev(a) = c, N the star point; the first end carries the polarity dot):

| Group | High-voltage winding | Low-voltage winding |
|---|---|---|
| Yy0, Yy6 | (P, N) | (p, n), (n, p) |
| Yd1, Yd11 | (P, N) | (p, next p), (p, prev p) |
| Yd5, Yd7 | (P, N) | (prev p, p), (next p, p) |
| Dy11, Dy5 | (P, next P) | (p, n), (n, p) |
| Dy1, Dy7 | (P, prev P) | (p, n), (n, p) |
| Dd0, Dd6 | (P, next P) | (p, next p), (next p, p) |

For positive-sequence voltages, V_p − V_next p = √3·V_p·e^{j30°} and V_p − V_prev p =
√3·V_p·e^{−j30°}, and reversing a winding adds 180°; the table makes the low-voltage side
lag by n·30° for clock number n (IEC 60076-1). A star point without N is an inner node.
The windings of a transformer are not connected to each other, so the check that every node
reaches gnd (E141) treats each winding separately: a delta side with no grounded component
floats. With both star points isolated and no magnetizing branch, the zero-sequence voltage
of the windings is not determined (E137).

### Faults

A fault is one element: for each faulted phase a contact in series with
`fault_resistance_ohm` from the phase node to a fault point, with the branch current as an
unknown (closed: V(p) − V(F) − R_f i = 0; open: i = 0). The fault point is gnd for a ground
fault without `ground_resistance_ohm`, an inner node connected to gnd through that
resistance otherwise, or a floating star point for a fault without ground. While every phase
of a fault without ground is open, nothing determines the voltage of the fault point; the
element then holds it at 0 V (a unit diagonal in its row).

All phases close at the first grid point at or after `start_time_s`. After `clear_time_s`
each phase opens at the first grid point at which its own current has changed sign or is
negligible, by the same rule as the time-controlled switch, so the first pole to clear
interrupts alone and the remaining phases carry the current among themselves until their
own zero (a quarter period later for a symmetrical fault without ground).

### Loads and current injections

`three_phase_load` expands into, per phase, a resistor G = P/V², an inductor
L = V²/(ωQ) for Q > 0 or a capacitor C = −Q/(ωV²) for Q < 0, from the phase node to the
neutral (V the rated line-to-line voltage, P and Q three-phase, ω from
`rated_frequency_hz`). At a phase-to-neutral voltage U it takes
3·|U|²·(G + j/(ωL)) = (P + jQ)·(√3|U|/V)²: the powers at the rated voltage and a constant
impedance otherwise. `three_phase_current_source` expands into three current sources
√2·I·sin(ωt + φ + 90° − k·120°), k = 0, 1, 2, from the neutral into the phases, whose
phasors are I·e^{j(φ − k·120°)}.

### Ideal transformers and phase shifters

`three_phase_ideal_transformer` is one element with three current unknowns j (the currents
into it at side 1) and the constraint V1 = N·V0 on the phase voltages to gnd. The currents
into it at side 0 are −Nᵀ·j, so that V0ᵀ·i0 + V1ᵀ·j = 0: no power is stored or lost. With
ρ = v1/v0 and α the phase shift,

N = ρ·T,  T_ik = (1 + 2 cos(α − 120°·(i − k)))/3,

the real circulant matrix with the eigenvectors of the symmetrical components: T·[1, 1, 1] =
[1, 1, 1], T·p = e^{jα}·p and T·p̄ = e^{−jα}·p̄ for the positive-sequence vector
p = [1, e^{−j120°}, e^{j120°}]. It is the symmetric phase shifter [IEEE C57.135]: the positive
sequence turns by +α, the negative by −α, the zero sequence passes with the plain ratio.
The stamps are those of the single-phase ideal transformer with the scalar ratio replaced
by N, the same in the time domain and with phasors (no state). With α = 0 it is an ideal
YNyn0 bank.

## Synchronous machines

`synchronous_machine` is Park's model in the dq axes of the rotor [Park; Kundur, chapter 3;
Krause et al.]: on the d axis the field winding and one damper winding, on the q axis two
damper windings (model 2.2 of IEEE Std 1110) or one (model 2.1). All machine quantities are in
per unit of the ratings, in the reciprocal system of Kundur (base voltage: the peak of the
rated phase voltage; base current: the peak of the rated current), with time in seconds and
ω0 = 2π·`frequency_hz`. The stator currents of the dq axes are positive out of the machine
(generator convention), θ is the angle of the d axis from the axis of phase a and the q axis
leads the d axis, as in the `abc_to_dq0` block:

```
v_d = (1/ω0) dψ_d/dt − ω ψ_q − Ra i_d          ψ_d = −(L_ad + L_l) i_d + L_ad (i_fd + i_1d)
v_q = (1/ω0) dψ_q/dt + ω ψ_d − Ra i_q          ψ_q = −(L_aq + L_l) i_q + L_aq (i_1q + i_2q)
(1/ω0) dψ_fd/dt = e_fd − R_fd i_fd              ψ_fd = (L_ad + L_fd) i_fd + L_ad i_1d − L_ad i_d
(1/ω0) dψ_1d/dt = −R_1d i_1d                    (and so on for 1d, 1q, 2q)
T_e = ψ_d i_q − ψ_q i_d
2H dω/dt = T_m − T_e − D (ω − 1),   dδ/dt = ω0 (ω − 1),   θ = ω0 t + δ − π/2
```

δ is the angle of the q axis against the reference cos(ω0 t) of the phasors, so in a steady
state it is the power-flow angle of the voltage behind the q-axis reactance. The zero sequence
is the branch Ra + 3 R_n, X0 + 3 X_n to gnd (none for an ungrounded star point). Magnetic
saturation is not modelled.

### From standard to fundamental parameters

The model takes the standard parameters (Xd, Xd′, Xd″, Xq, Xq′, Xq″, Xl, Ra and the
open-circuit time constants) and computes the windings with the classical expressions
[Kundur, section 4.2], with all mutual inductances of an axis equal (L_ad or L_aq):

```
L_ad = Xd − Xl
L_fd = L_ad (Xd′ − Xl) / (Xd − Xd′)                 from Xd′ = Xl + L_ad L_fd / (L_ad + L_fd)
L_1d = (Xd′ − Xl)(Xd″ − Xl) / (Xd′ − Xd″)           from Xd″ = Xl + 1 / (1/L_ad + 1/L_fd + 1/L_1d)
R_fd = (L_ad + L_fd) / (ω0 Td0′)
R_1d = (L_1d + L_ad L_fd / (L_ad + L_fd)) / (ω0 Td0″)
```

and the same on the q axis with the windings 1q (Xq′, Tq0′) and 2q (Xq″, Tq0″). With one
damper winding on the q axis, L_1q = L_aq (Xq″ − Xl)/(Xq − Xq″) and R_1q = (L_aq + L_1q)/(ω0 Tq0″).
The expressions are approximations: they take the transient values as those of the first
winding alone and the subtransient ones as those of the second winding with the first one
short-circuited without resistance (R_1d ≫ R_fd). The reactances of the model are exactly the
given ones; the time constants of its exact transfer functions differ slightly from the
given ones. For the 555 MVA machine of Kundur's examples the expressions return the
windings that example 4.1 starts from, to three digits (L_fd = 0.165, L_1d = 0.171,
R_fd = 0.0006, R_1d = 0.0284). Short-circuit time constants are not accepted; convert them with
Td0′ = Td′ Xd/Xd′ and Td0″ = Td″ Xd′/Xd″. The order Xd > Xd′ > Xd″ > Xl > 0 (E280) and
Td0′ > Td0″ > 0 (E281) keeps every inductance and resistance positive.

The field voltage of the input, `Efd`, and the field current of the signals are in the
non-reciprocal system in which 1 pu gives rated voltage at no load and rated speed:
Efd = (L_ad/R_fd) e_fd and I_fd = L_ad i_fd, so that Efd = I_fd in a steady state. The
mechanical input is the torque (on the rated power at rated speed); a governor that gives a
power divides it by the speed.

### A voltage behind the subtransient reactance

With the rotor currents eliminated, the stator flux of an axis is ψ_s = k′ψ_r − L″ i_s, where
ψ_r are the rotor fluxes of the axis, k = L_rr⁻¹ m (L_rr: inductances of the rotor windings,
m: their mutual inductances with the stator) and L″ the subtransient inductance. Let X be
the mean of L_d″ and L_q″ and ψ̃ = k′ψ_r − (L″ − X) i_s per axis. The two stator equations
then are, in the phases and for the currents into the machine,

```
v_abc = Ra i_abc + (X/ω0) d i_abc/dt + e_abc,      e_dq = (1/ω0) dψ̃_dq/dt + ω J ψ̃_dq
```

(J turns a dq pair by 90°): a voltage e behind the branch Ra, X, which is the same in every
phase and does not depend on the rotor angle [Dommel, chapter 8; Wang and Jatskevich]. This
branch is integrated in the phases with the stage formula, exactly as a coupled RL branch (the
zero sequence with X0 and the neutral impedance): a constant conductance and a history
current. The matrix does not change while the machine runs, so nothing is factorized again.
The rotor fluxes, the speed and the angle are integrated with the same stage formula in the
rotor frame. dψ̃/dt is k′ dψ_r/dt − (L″ − X) di_s/dt with the derivatives that the stage formula
gives for the rotor fluxes and for the phase currents (the derivatives of the dq currents
follow from those of the phase currents and the speed). All are derivatives of states of
differential equations, so the trapezoidal rule does not make them oscillate.

Integrating the stator in the phases matters for the accuracy of the voltages. The reactance
of the machine is in series with the inductances of transformers and lines. When all of them
are integrated with the same formula, the voltage of the node between them is the same
combination of the voltages on both sides as in the differential equations. Stator fluxes
integrated in the rotor frame are as accurate as states, but the terminal voltage then has
the error of a derivative of 2S-DIRK, about 0.35 ω0 h of the voltage across the series
inductance; this was tried first and measured (6e-4 of the terminal voltage at h = 50 µs for a
line of 0.15 Ω, falling only by half when the step was halved).

### The iteration of a stage

The voltage e of a stage depends on the dq currents of that stage: through the rotor windings
(the rotor fluxes of the stage are a linear function of the stator current of their axis,
found once per stage coefficient from the two rotor equations), through the difference of
the two axes (L″ − X) and through the torque, which moves the speed and the angle. The
machine therefore evaluates e at a guess of the dq currents, the circuit is solved, and
`update_operating_point` compares the dq currents of the solution with the guess. When they
differ by more than 1e-9 pu (or, where that is less, by what moves the terminal voltage by
1e-7 pu), the guess is moved and the stage is solved again with the same factors
(`RhsChanged`). Nothing of the machine is one step late: the speed voltages, the rotor angle
and the difference of the axes belong to the solution of the stage.

- Plain substitution (the next guess is the solution) contracts by about
  |L_d″ − L_q″| / (L_d″ + L_q″ + 2 L_e) per solution, where L_e is the inductance of the
  circuit seen from the terminals at the stage coefficient: 0.04 for the machine of Kundur
  (0.23, 0.25) at a terminal short circuit, 0.2 for a salient-pole machine with 0.2 and 0.3,
  and about 2e-4 when the two are equal (what is left is the rotor resistances over one
  stage). It converges for every machine with Xd″, Xq″ > 0.
- The machine learns the sensitivity of the solution to its guess (a 2 × 2 matrix B, with
  Broyden's update from each pair of successive guesses) and moves the guess by
  (1 − B)⁻¹ (solution − guess). B belongs to the circuit and the stage coefficient, so it is
  kept from stage to stage; when a move makes the difference larger, B is forgotten and the
  iteration continues with plain substitution.
- The first guess of a stage continues the dq currents of the same stage of the three steps
  before by a parabola (they are constant in a steady state).
- Measured (2S-DIRK, h = 100 µs): a terminal short circuit from no load needs 0.95 further
  solutions per stage on average and at most 4 for both machines above (plain substitution:
  4.8 for the round rotor); a fault and the swing after it against an infinite bus
  (`examples/synchronous_machine.json`) 0.36; a steady state 0 once the small transient of
  the start has decayed. For the IEEE 39-bus system with nine machines, one second (20 000
  steps of 50 µs) takes 0.29 s in the steady state (0.005 further solutions per stage) and
  0.35 s with a fault of 100 ms (0.9 per stage), against 0.23 s and 0.17 s with sources
  behind impedances (one core; the machines have no inner nodes, so the system has 222
  unknowns instead of 276). One machine with its small circuit takes about 0.6 µs per step.

The inputs follow their signals as a controlled source does: in a straight line over the step,
one step late. The signals of the machine are those of the last grid point; the dq voltages
are the Park transform of the terminal voltages there.

### Start

In the phasor solution the machine is a source behind Ra + jX with the internal voltage
E = V + (Ra + jX) I, where V and I = conj(S/3V) are the terminal values of `operating_point`
(the negative and zero sequence see Ra + jX and the zero-sequence branch). When the circuit
is that of the power flow, the solution is the power flow. The rotor is then set from the
positive-sequence terminal voltage and current of the solution, whatever they are [Kundur,
section 3.6.5]: δ = arg(V + (Ra + jXq) I), the dq components of V and I, ψ_d = v_q + Ra i_q,
ψ_q = −v_d − Ra i_d, i_fd = (ψ_d + L_d i_d)/L_ad, no damper currents, ω = 1, and the inputs
Efd = L_ad i_fd and T_m = T_e, which the machine keeps when it has no signals and offers as
`initial_field_voltage_pu` and `initial_mechanical_torque_pu`. The phase currents of the
branch Ra, X start on their phasors. The run therefore starts in a steady state of the
solved operating point, also when that is not the one written in the model; it departs from
the phasors only by the integration error of the branch and the circuit, of the order
(ω0 h)². A phasor solution with a negative or zero sequence at the terminals (above 1e-4 pu)
is reported with W280: the rotor's response to it is not part of the phasor solution.
Without steady-state initialization the machine starts with the fluxes and currents of
`operating_point` at rated speed, while the circuit starts from its own initial values.

## Steady-state initialization

With `"initialization": "steady_state"` the run starts from the sinusoidal steady state of
its sources. The same modified nodal equations are written with phasors at the source
frequency ω: an inductor has the admittance 1/(jωL), a capacitor jωC, a coupled RL branch
(R + jωL)⁻¹, a capacitance matrix jωC; resistors, ideal transformers, voltage-source rows,
switches (in their state before t = 0) and faults (not started) keep their real stamps. A
sine source A·sin(ωt + φ) is the phasor (A/√2)·e^{j(φ − 90°)}. The complex system is solved
once with the LU of the run (dense or sparse, with complex numbers) and every state is set to
its value at t = 0: an inductor current i(0) = √2·Re(I), a capacitor voltage
v(0) = √2·Re(V), and the current and its peak in a closed switch (for an opening at a current
zero).

No other history is needed, except by travelling-wave lines, which read the sine before
t = 0 (see [Travelling-wave lines](#travelling-wave-lines)). The first stage of 2S-DIRK does not use the derivative of the
previous step (c = 0), and the trapezoidal rule takes its derivatives at t = 0+ from the
existing start (two backward-Euler steps of 1e-6 h, see [Start of the run](#start-of-the-run)),
which on consistent states moves them by about 1e-6 h times their derivative. The run then
departs from the exact steady state only by the error of the method in the forced response,
about k·(ωh)² of the amplitude (k = 1/12 for the trapezoidal rule, 0.01 to 0.03 for 2S-DIRK
in the tests). The run measures this departure over every node voltage and every recorded
current until the first switching action and reports it in the summary.

### Currents that a source dictates

The current of a capacitor connected directly across a voltage source is C dv/dt of a
prescribed voltage (the circuit equations are then of index 2, as for an inductor in
series with a current source). The trapezoidal rule gives it to second order. 2S-DIRK gives
it to first order only: with exact stage values y(t + γh) and y(t + h), its last stage
derivative (Y2 − y0 − h(1 − γ)f1)/(γh) differs from y′(t + h) by
h·y″·(1 − 3γ + γ²)/(2γ) = h·y″/(2√2), a relative error of ωh/(2√2) for a sine
(1.1e-3, or 0.064°, at 50 Hz and h = 10 µs). The voltages, and the currents of branches with
any impedance in series, stay of second order. Other programs that use 2S-DIRK show
the same error. A source impedance or the trapezoidal rule avoids it.

The dual index-2 case behaves in the same way: at a node connected only to inductive
branches and an ideal current source, the node voltage is determined by the derivative of
the injected current, so 2S-DIRK has a first-order voltage phase error of ωh/(2√2) while
the trapezoidal rule stays second order. Where a shunt with negative conductance and
capacitive susceptance is converted (such as in a PowSyBl network), keeping the capacitive
reactive power as a constant impedance (`three_phase_load` with `active_power_mw: 0`) and
representing only the negative active part as a current source avoids creating such a node
and restores second-order accuracy.

Only circuits with a sinusoidal steady state qualify: sine sources of one frequency without
offset (a dc source of 0 is allowed), no diodes and no controlled switches (E180 to E182).
The steady state of a circuit with diodes is periodic but not a sine; it has no phasor. Such
circuits start from rest and run into their periodic steady state.

In format `"0.7"`, individual circuit components can be marked `"initialization": "exclude"`
to omit them from the phasor solution while keeping the rest of the circuit in steady state
(E180–E182 and W240 do not apply to excluded components; E183 rejects excluding all
components or disconnecting or ungrounding the remaining included subcircuit, and W185 warns
when a series or multi-bus component is excluded so only shunt branches are omitted
unintentionally). In the phasor
solver, excluded elements skip `phasor_singular`, `stamp_phasor`, `rhs_phasor`,
`init_from_phasor` and `steady_state_warning`, and report zero phasor current. Any untouched
unknowns in the complex system—private inner or auxiliary unknowns of excluded elements and
nodes connected only to excluded elements—receive unit diagonals (`X = 0`), while the
remaining included subcircuit is solved as usual. At t = 0, excluded elements start with zero
state (or their own `initial_*` parameters) and enter the time-domain circuit from the first
time step (t = 0+).

### From a power flow

The Python converter (`openkeito.pandapower`) does not pass a power-flow solution to the
solver. It chooses the component values so that the phasor solution above is the power
flow, which needs no new method and keeps the model self-contained. A power flow fixes, at
every bus, the voltage V (phase to neutral, from `vm_pu`, `va_degree` and the rated
voltage) and, for every injection, the complex power S. Then:

- a source with internal impedance Z delivers I = conj(S/(3V)) at V when its internal
  voltage is E = V + Z·I;
- a load takes S at V when it is the constant impedance with that S at |V|
  (`rated_voltage_kv` = √3|V|);
- a current injection I = conj(S/(3V)) delivers S at V.

With these, the bus voltages of the power flow satisfy the nodal equations of the network,
so they are the phasor solution as long as the branches are modelled as in the power flow.
pandapower's are: nominal pi lines (one section, shunt capacitance halved at each end),
transformers as a T circuit with the series impedance u_k referred to the tapped rated
voltage of the low-voltage side and the magnetizing admittance in the middle (conductance
from P_fe; susceptance −√(Y_m² − G²) with Y_m from |i_0|), the tap changing the rated
voltage of its side and the phase shift added to the ratio, and bus-bus switches with
`z_ohm` > 0 as impedances of R/X = 2 [Thurner]. The converter writes the same values into
`pi_line` (one section), `three_phase_transformer` (whose leakage halves and magnetizing
branch form the same T) and `coupled_rl`. What remains is the convergence tolerance of the
power flow (pandapower: 1e-8 MVA mismatch), which is what the tests see: about 1e-10 pu in
voltage and 1e-8 relative in branch power for case9, case39 and case118 [Zimmerman].

Zero-sequence and generator data that a power flow does not need are assumed and listed as
warnings (see [python.md](python.md#from-a-power-flow-pandapower)). They do not change the balanced steady state, only the response
to unbalanced disturbances and the short-circuit level.

The PowSyBl converter (`openkeito.powsybl`) writes IIDM's equivalent circuits, checked
against OpenLoadFlow's terminal flows to rounding (1e-15): a line is the pi circuit R + jX
with the shunts g1 + jb1 and g2 + jb2; a two-winding transformer is an ideal ratio
ρ·e^{jα} at side 1 followed by the magnetizing g + jb and the series R + jX on the side-2
voltage (at the current tap: ρ, α, R, X, g and b of the tap step); a three-winding
transformer is three such legs, each with the ratio rated_u0/rated_ui (times its taps) at
the network bus, meeting at a star node; a dangling line has its whole shunt at the
network bus and the boundary's constant p0 + jq0 at its far end [PowSyBl]. OpenLoadFlow
stops when the mismatch is below 1e-4 per unit per equation and leaves up to 1 MW at the
slack bus by default; both are far above the rounding of the phasor solve. The converter
therefore runs the load flow with 1e-10 and 1e-6 MW, without distributed slack and with
the largest generator as slack, and gives what the reported terminal flows still leave
unbalanced at a bus to a generator there (or to a current injection): the load-flow
voltages then satisfy the nodal equations of the model exactly (differences of 1e-12 pu
for the CGMES MicroGrid BE and NL configurations and IEEE 14 to 118).

IIDM puts the magnetizing admittance of a transformer directly behind the ideal ratio,
before the series impedance; `three_phase_transformer` has it in the middle of the leakage
(a T circuit). The converter therefore keeps IIDM's circuit with a
`three_phase_ideal_transformer`: with the T circuit in its place the bus voltages move by
an amount in proportion to the magnetizing admittance (6e-5 pu for the Eurostag example).

IIDM's load flow does not turn the voltages by the phase-angle clocks of the transformers,
and by default neither does the model. With `phase_angle_clocks="rotate"` the converter adds
−30°·n to the angle of the ideal transformer of a two-winding transformer with clock n
(side 2 lags side 1; for a three-winding transformer, at the ideal transformer of leg 2 or
3, whose bus lags the star node). The ideal transformer turns the positive sequence by its
angle and the negative sequence by minus that, which is what a transformer of that vector
group does to them [IEC 60076-1]; the zero sequence is handled as below. The buses and
star nodes form a graph whose edges are the branches connected at both ends; in every
connected part the bus with the highest nominal voltage keeps its load-flow angle, and
every other bus gets the sum of the clocks on the way to it. Turning all voltages and
currents of a zone by one angle leaves its nodal equations as they are, so the turned load
flow is the phasor solution (differences of 1e-12 pu for the Eurostag example, IEEE 14 and
MicroGrid BE; `python/tests/test_powsybl_clocks.py`). A loop whose clocks do not add up to
a multiple of 12 has no such solution and is refused. A phase tap changer's angle is in
IIDM's load flow already and stays in the ideal transformer beside the clock. For a
line-to-ground fault behind a Dyn1 and a YNd11 transformer, the phase voltages and currents
agree with symmetrical components (positive sequence turned by +θ, negative by −θ of the
zone) within the 5e-4 of the blocking factor below; the generator currents are in the
proportion (1, 0, −1), agreeing to 2e-6, in place of the (2, −1, −1) without clocks. The
ground-fault response. Where both sides are known grounded stars (`Y_GROUNDED`) and the
clock is 6 (YNyn6), the ideal transformer sets `zero_sequence_inverted: true` so the zero
sequence is inverted ($-v_1/v_0$) as well, without a warning; unknown connections or clocks
other than 0 and 6 warn and treat the zero sequence as YNyn0.

Short-circuit data (IIDM's Fortescue extensions, the short-circuit attributes of a CGMES
equipment file, or values of the caller) change the zero sequence only, by parts that the
positive and negative sequence do not see, so the agreement with the load flow stays
[Anderson, Grainger]:

- A series branch (line, transformer impedance) is a coupled RL branch with its own Z0.
  Where a winding lets no zero-sequence current through (a delta, a star without a grounded
  star point), Z0 = 10⁶·Z1 stands for the open circuit: the error of a zero-sequence
  impedance Z at a bus is |Z|/(10⁶·|Z1|) of itself for each such branch there (up to
  2.2e-4 in the test: 1.6e-4 at a generator grounded through 6 Ω beside a blocked
  transformer of 0.04 Ω), and the factor does not cost accuracy at the start (see
  [Coupled RL branches](#coupled-rl-branches)).
- A shunt y1 with the zero-sequence admittance y0 is a star y0 to gnd and a star y1 − y0 to
  a star point of its own: in the positive sequence the star point stays at 0 V and the two
  add up to y1; the zero sequence sees y0. y0 = 0 (a magnetizing branch with free fluxes,
  or behind a winding that is not a grounded star) is the second star alone. A magnetizing
  branch that carries zero-sequence current has y0 = 1/(j·x_m) when the extension gives the
  zero-sequence magnetizing reactance `xm`, and y0 = y1 otherwise.
- The delta winding of a YNd transformer short-circuits the zero sequence of the star side
  through the transformer's impedance. The model has that as a bank of three ideal
  single-phase units in YNd (`three_phase_transformer` without magnetizing branch, nothing
  connected to its delta, one corner of it at gnd), whose leakage is Z0 + 3·Z_g: balanced
  voltages induce delta voltages that add up to zero, so no current flows; a zero-sequence
  voltage drives a current around the delta that only the leakage limits. It sits behind
  the ideal ratio of IIDM's circuit for a delta at side 2, at the bus of side 2 for a delta
  at side 1, and at the star node for a delta leg of a three-winding transformer.
- Grounding impedances Z_g carry three times the zero-sequence current and are added as
  3·Z_g to the zero-sequence impedance of their winding or generator; a generator that is
  not grounded has its star point on a node of its own.

The test builds the positive- and zero-sequence nodal admittance matrices of the Eurostag
example by hand for three pairs of connections (Dyn + YNd, YNyn + YNyn, Yyn + YNy), and
compares the impedances at every bus with the frequency scan and I = 3V/(Z1 + Z2 + Z0 + 3R_f)
with the steady state of a line-to-ground fault through R_f: the positive sequence agrees
to 1e-9, the zero sequence and the fault current within the 5e-4 that the blocking factor
allows.

The units of the short-circuit attributes of a CGMES file depend on its version: r0, x0,
r2 and x2 of a synchronous machine are per unit in CGMES 2.4.15 and in ohm in CGMES 3.0
(the resistance r is in ohm and the saturated reactances are per unit in both), by the RDF
schemas of the two versions. The converter tells the versions apart by the CIM namespace
of the file.

Schedule times are RFC 3339 date-times with an offset, converted to seconds since
1970-01-01 UTC with the days_from_civil algorithm [Hinnant] to check their order.

### From one power flow to the next

A transition (`openkeito.transition`) starts from the power flow of one time of a schedule
and ends at that of another. The converter is run on the network at both times, and the two
models are compared component by component; no power-flow solution is passed to the solver
here either. Every device gets, as a function of time, the input that the second power flow
asks of it, so that the end is a steady state of the model that has the bus voltages of the
second power flow, as the start has those of the first:

- **Machines.** The excitation system and the governor are control blocks
  (`add_machine_controls`): E_fd = K_A (V_ref − V_t)/(1 + sT_A) within its ceilings
  [IEEE 421.5, type ST1A without stabilizer; Kundur, chapters 12 and 13], and
  valve = (P_ref − (ω − 1)/R)/(1 + sT_G) within its limits, P_m = valve/(1 + sT_CH),
  T_m = P_m/ω [Kundur, section 11.1]. Their references are the values that hold t = 0
  (`initial_value` blocks of V_t + E_fd/K_A and of T_m, which the machine gives from its phasor
  solution) plus a change. The steady state of the machine for a power-flow point (P, Q, V)
  gives the inputs it needs there [Kundur, section 3.6]: with I = conj(S/V) and
  E_Q = V + (R_a + jX_q)·I on the q axis, E_fd = v_q + R_a·i_q + X_d·i_d and
  T_m = P + R_a·|I|². The load reference moves by T_m(b) − T_m(a), and the voltage reference by
  (V_b − V_a) + (E_fd(b) − E_fd(a))/K_A: the second term is what a proportional regulator needs
  to end at V_b and not at V_b minus its error. Without saturation these are exact, so the
  machine ends at the second power flow when the rest of the network does.
  A power system stabilizer of the form of [IEEE 421.5] type PSS1A, with the speed as its
  input and without the transducer lag and the torsional filter, is added to the voltage
  reference unless `pss=False`:
  V_s = K · sT_W/(1 + sT_W) · (1 + sT_1)/(1 + sT_2) · (1 + sT_3)/(1 + sT_4) · (ω − 1) within
  its limits, with values of a stabilizer for a thyristor exciter [Kundur, chapter 12]
  (K = 9.5, T_W = 1.4 s, T_1 = 0.154 s, T_2 = 0.033 s, no second stage, limits ±0.1 pu;
  these values have not been checked against the book). The washout makes its output 0 in
  any steady state, so the start is the power flow as before. It is made of a
  `transfer_function`, `lead_lag` blocks and a `limiter`. A machine with less than 10 % of
  its rating as active power at every time gets none: the stabilizer reaches the torque in
  proportion to the load, changes the damping of an unloaded machine by 0.03 1/s only, and
  made the small synchronous condensers of case118 swing more than without it.
- **Converters.** The power references are signals that go from the powers of the first
  power flow to those of the second.
- **Loads.** A load is the impedance that takes its power-flow power at its power-flow
  voltage, at each time: G + jB = (P + jQ)/V². What both times have in common stays connected
  (the smaller conductance, and the smaller susceptance when the signs agree); the rest is
  switched out or in through ideal switches, in `load_steps` equal parts at the midpoints of as
  many equal parts of the ramp. All loads change a little whenever the voltages do, because the
  schedule fixes their powers. The switches open at the current zeros of their phases.
  A part behind an open switch is all that ties its node to ground. An inductance L alone does
  not do so in the first part of the first step, where it counts as h/(10⁶·L): below the
  threshold of the solver, 10⁻¹³ S, from L = 10⁷·h on (500 H at 50 µs, a part of less than
  0.63 Mvar at 345 kV). Such parts appear when only the active power of a load is scheduled:
  its susceptance Q/V² then moves a little with the voltage, against its conductance. Every
  part with an inductance therefore has at least 1 W of active power (8·10⁻¹² S at 345 kV),
  and a change of less than 1 W per part is left out; both are 10⁻⁶ MW against the 10⁻³ MW at
  which the end of a transition agrees with the power flow.
- **Sources.** An external grid behind its impedance Z has E = V + Z·I at each time. It becomes
  a `three_phase_controlled_source` behind the same Z whose voltage is written by a `dq0_to_abc`
  block on the angle ω₀(t + h), with d + jq = √2·E going from the one value to the other in a
  straight line of the complex plane; the angle is one step ahead because a controlled source
  follows its signals one step late. The grid stays at the rated frequency: it is the angle
  reference of both power flows.
- **Breakers.** A branch that is in service at one time only gets ideal switches at both ends
  and a leak of 1 W to ground behind them, so that an isolated branch keeps defined voltages.
  A machine that stops is tripped by a breaker at its terminals, with its load reference going
  to zero at the same instant; one that starts is in the model from t = 0 at no load and at the
  voltage that its bus has in the first power flow. When a bus-bus switch joins or splits
  buses, one breaker joins or splits the two nodes.
- **What does not move.** The ratio of a transformer is a constant of the model, so a tap
  change keeps the first value; current injections keep theirs. Both are warnings, and the end
  then differs from the second power flow.

Without a source the network is an island: the load references still move by T_m(b) − T_m(a)
of each machine, the slack machine of the power flow included, so the powers balance at the
rated speed in the second state and the droop leaves no frequency error beyond what the
voltages at the loads leave.

The time axis of the schedule is not that of the run. The set points move over a ramp time
given in seconds (or the change of power over a rate), after `start_time_s` of steady state
and before `settle_time_s`. A schedule with `"interpolation": "step"` says nothing about how
its values get from one time to the next, so any ramp is a choice of the caller; with
`"linear"`, `ramp_time_s="schedule"` takes the real time between the two times, which may be
any two times within the schedule, and the values that only step (`in_service`) then change
at the end of the ramp.

**More than two times.** `build_sequence` (the converters' `transition(times=[...])` and
`run_transitions(chain=True)`) takes the conversions of n times and makes one model. The
time axis is `start_time_s` of steady state and then, for each interval, its changes and
its settling time; the interval k + 1 begins at the grid point nearest to the end of the
last change of interval k plus `settle_time_s[k]`. The time between two times of the
schedule is not run.

- A set point is one function of time through all its values. A machine's references move
  by T_m(k) − T_m(1) and (V_k − V_1) + (E_fd(k) − E_fd(1))/K_A, the values of each time
  against the first, so every time is reached as the second time of a transition between
  two is.
- A load is G_k + jB_k at time k. The conductance, the inductive and the capacitive
  susceptance are each a sequence of levels. What lies below the lowest level stays
  connected. The slice between two neighbouring levels is connected at the times whose
  value reaches the upper one; slices that are connected at the same times form one part,
  which is switched in `load_steps` equal parts. For two times these are the common part
  and the parts that go out and come in.
- A switch closes once and opens once. A part or a branch that is in service at several
  separate spans of times has one set of switches per span, in parallel (one set serves
  "open, then closed again").
- A machine that starts at time k is in the model from t = 0 at no load; until then its
  voltage reference follows the voltage that its bus has in each power flow. A machine
  that stops and returns to service re-synchronizes while its breaker is open using its
  governor and excitation system (`abc_to_dq0` against the live bus voltage magnitude and angle
  error $\operatorname{atan2}(-d, q)$ with lead-lag synchronizing torque, releasing back to the
  normal governor and AVR when the breaker closes); re-synchronizing without both controls is
  refused (`E700`).
- Transformer tap ratios (`tap_changes`) and coupled series impedances (`impedance_changes`) step
  at the interval switching instants (`t_switch`), scaling linear magnetizing currents and
  saturation curves to keep the core flux per turn continuous across HV tap changes.

`measure(..., time=k)` compares the run at the end of the settling time of time k with the
power flow of that time. A run through all times differs from the runs of the intervals in
what the interval before has left: each interval on its own starts from the exact steady
state of its first time.

**Ratings for a transition.** A generator without a rating is rated by `machine_rating_for`
for all times of the transition: its largest apparent power is 85 % of the rating, and the
reactive power it absorbs at most 85 % of V²/X_d of the rating. The second rule keeps the
field voltage of the steady state positive: without active power E_fd = V − X_d·(−Q)/V,
which is zero at −Q = V²/X_d. Rated by the first time alone, a machine whose output grows
hits the upper limit of the governor valve (1.1 pu) and does not reach the second power
flow. Time slices rate a machine by their one time.

`measure` reads the bus voltage magnitude from the three phase voltages of one recorded
instant, |V| = √(v_a² + v_b² + v_c²) (line to line, rms), and the angle from their space
vector against ω₀t. Both are exact for balanced sinusoidal voltages, need no window and work
with any thinning of the output; a dc component after a switching action shows as a ripple at
the fundamental frequency. Angles at the end are compared between buses, so that an island at
another frequency compares as well.

Tests (`python/tests/test_transition.py`, `test_pss.py`, `test_transition_chain.py`,
`test_transition_ratings.py`), at 50 µs unless said:

| Case | Compared with | Difference |
|---|---|---|
| Step of the load reference of a machine alone on a load (555 MVA, H = 4 s, typical governor) | Δω = ΔP_ref/(s·D(s)), D = 2H·T_G·T_CH·s³ + 2H(T_G + T_CH)s² + 2Hs + 1/R, by partial fractions | 1.8e-5 of a peak of 3.3e-3 (0.6 %) at 100 µs, 2.0e-5 at 50 µs: the transfer function takes the electrical power as constant, and it moves by 0.5 % of the step with the voltage. Final value R·ΔP_ref within 1e-3 of it |
| Step of the active power reference of a converter in a strong grid (τ = 1 ms) | first-order lag 1 − e^{−t/τ} | 2.2 % of the step at 50 µs, 1.1 % at 25 µs (the dead time of one step; bound 0.74·h/τ) |
| Ramp of the same reference over 20 ms | response of the lag to the ramp | 5.9e-4 MW of 0.6 MW at 50 µs, 2.9e-4 at 25 µs (below the ramp of one step) |
| Start of a transition (case9, case39, a ring with all kinds of elements, IEEE 14 from PowSyBl) | power flow of the first time | 1e-15 to 3e-10 pu, as for time slices; before the change the magnitudes stay within 9e-6 to 7e-5 of their first value (bound (ωh)²) |
| End (same cases, governor time constants 0.02 and 0.03 s, 4 to 10 s after the change) | power flow of the second time | 9e-6 pu, 4e-4° to 1.3e-3°, 2e-3 to 0.016 MW: the error of the time step, as before the change |
| End with the sources held (case9) | the same | 5e-4 pu: the internal voltage of the grid differs by 0.11 % between the two power flows |
| An island of two machines | 50 Hz at the end | 2e-9 Hz of swing; voltages within 2e-7 pu |
| Parallel and sequential runs | each other | equal bit for bit |
| Swing of a machine against an infinite bus (555 MVA, 0.9 + j0.3 pu through 0.02 + j0.65 pu, regulator gain 200), 100 µs | eigenvalue of the system linearized in the test (Park's equations with the stator and line transients, 12 states) | without stabilizer +0.601 ± j7.358 1/s (unstable): 1.0e-4 of the frequency, 2.3e-4 1/s; with it −1.206 ± j6.912 1/s (damping ratio 0.172): 4.7e-4, 2.1e-3 1/s. The step that starts the swing moves the operating point by this much |
| Step of the speed into the stabilizer | partial fractions of its transfer function | 2e-6 of the peak (bound 0.25·(h/T_2)²) |
| End with the typical governor, 10 s after the change | power flow of the second time | case9: 8.0e-6 pu, 1.1e-4°, 3.8e-4 MW with the stabilizer (7.8e-5 pu, 0.12°, 0.52 MW without); case39: 3.3e-6 pu, 2.9e-3°, 0.017 MW (4e-3 pu, 3°, 20 MW without) |
| case9, four times of an OPF plan in one run (typical governor and stabilizer, 10 s of settling each) | power flow of each time at the end of its settling time | 8.0e-6 to 9.7e-6 pu, 7e-5 to 1.2e-3°, 4e-4 to 4.7e-3 MW |
| The same run | the intervals as runs of their own, at the same times | 1.1e-9 pu and 1.7e-5 MW at most: what the interval before has left after 10 s |
| A load that rises and falls, a line that goes out and comes back, a machine that stops or starts at the third time (ring) | power flow of each time | 7e-6 to 9e-6 pu, 1.4e-3 MW |
| Two times given as a list | the transition between two times | the same model text; results equal bit for bit |

Without the stabilizer the swings of the machines against the grid decay slowly with the
typical governor (T_G = 0.2 s, T_CH = 0.3 s): at their frequency (0.8 Hz in case9) the
governor and turbine lag the speed by about 90°, and their torque cancels most of the
damping of the rotor windings. After a load step in case9 the swing shrinks by 0.70 every
2 s without governor, by 0.90 with the typical one, and to 0.07 over 4 s with the
stabilizer. Without it, case39 is still 3 MW from the second power flow 60 s after the
change. The typical values of the stabilizer are for a local mode near 1 Hz; a slow mode
between areas needs a longer washout. A reheat turbine and damping
(`machine_parameters={"damping_pu": ...}`) are not part of the typical controls.

## Frequency scan

`openkeito scan` assembles the same complex nodal matrix Y(ω) with every source set to zero
(voltage sources shorted through their branch rows, current sources open, three-phase sources
reduced to their internal impedance) and solves Y(ω) x = e_k for a unit current injected into
each requested node k. The voltages at the requested nodes form the columns of the bus
impedance matrix Z_bus = Y⁻¹ restricted to those nodes: self impedances Z_kk and mutual
impedances Z_jk. For a bus, the 3×3 block of its phase nodes is transformed to symmetrical
components, Z012 = A⁻¹·Z_abc·A, and the diagonal gives Z0, Z1 and Z2. The matrix is
factorized once per frequency. A frequency at which it is singular gives an empty row; a
lossless resonance at exactly a scanned frequency usually is not singular in floating point
(the admittances cancel within one entry to rounding) and shows a very large impedance.

Diodes and controlled switches have no state that a scan could know; they are taken as off
(their blocking resistance). Validation says so (W192) for every model that has a
`frequency_scan` and such components, so `validate`, `run` and `scan` report the same
warning once.

## Error of switching at grid points

With `"switching": "grid"` (the default) switching instants are grid points. The error of
such a run has two parts, which the tests measure separately
(`crates/openkeito-core/tests/power_electronics.rs`); the second part is what
[switching instants within a step](#switching-instants-within-a-step) remove:

1. **Integration error**: comparing with the exact solution of the same circuit (switches as
   resistors with two values, solved piecewise with matrix exponentials) for the switching
   instants the solver used. It is second order for both methods, right after the actions
   too: in a 10 kHz buck converter at h = 1 µs, 1.5e-6 A for 2S-DIRK and 1.2e-4 A for the
   trapezoidal rule, whose backward-Euler half steps after the 40 actions dominate
   (40 · h²/4 · |i_L''|).
2. **Switching-time error**: an instant moves to the next grid point.
   - PWM: the comparator is evaluated at grid points, so each edge moves by less than h and
     the duty differs from the reference by less than `h·f_sw` (1 % at 10 kHz and 1 µs); the
     average output moves by up to `E·h·f_sw`. With a symmetric triangular carrier whose
     half period is a whole number N/2 of steps, the realized duty is
     `(2·floor(D·N/2) + 1)/N`. When a crossing falls exactly on a grid point (D·N/2 a
     whole number, such as D = 0.4 with 100 steps per period), rounding of the carrier
     decides it, and the duty varies irregularly from period to period. Choose the duty or
     the carrier so that crossings fall between grid points.
   - Diodes: a diode changes state up to one step late. The row after a turn-off shows the
     current up to `|di/dt|·h` past zero (1.6 A at h = 1 µs in the discontinuous buck test,
     first order). The effect on averages is small: the late turn-off draws at most
     `(1/2)(v/L)·h²` of charge per period (0.9 mV of 80 V at h = 1 µs in the test).

## Switching instants within a step

With `"switching": "interpolated"` every switching instant t* lies where it is, and the
time step that contains it is split there. 2S-DIRK and the trapezoidal rule are one-step
methods: a step needs nothing of the steps before it but the states (and, for the
trapezoidal rule, their derivatives). The step from t0 to t1 is therefore solved as two
steps of the same method, [t0, t*] and [t*, t1], with the switching action between them. A
part that covers the fraction f of the step has the stage coefficients α/f of the table
above (the same c and the same stage times as fractions of the part). The solution comes
back to the grid point t1 with no interpolation of states, and the output stays on the
grid. The error is that of the method, second order; nothing of first order is added by
the switching.

Where the instants come from:

- **Given times** (a `switch`, a `fault`, a `step` waveform): the time itself. A part ends
  there.
- **Gate signals**: the controls are evaluated at grid points, on the circuit solution
  there. At a grid point t_k the solver previews every signal over the step that follows,
  with the measured quantities and the states of the blocks held at their values of t_k:
  a block that is a known function of time (the carrier, a step, a ramp, a sine wave, a
  time series) or of its inputs alone (gain, sum, comparator, product, quotient, function,
  limiter, dead band, selector, piecewise linear, expression, the transformations to and from
  the rotating frame) is evaluated at the previewed time, every other block (those with a
  state, the measurements) holds its output. The blocks of a loop are previewed once, in
  their order.
  The instant from which a `controlled_switch` would switch, or from which the gate of a
  `thyristor` (conducting or not) starts or stops firing it, is found by bisection on the
  previewed time, to 2⁻⁴⁰ of a step. With a constant reference the PWM edges are the
  crossings of reference and carrier, exact to rounding; a reference that follows a
  measurement is sampled at grid points and held for one step (a delay of at most one
  step, half a step on average, and no quantization of the duty). The signals are
  previewed at the end of the part and, when no element would switch there, at the
  vertices of every `triangle_wave` within the part (its minima and maxima): a pulse that
  begins and ends within one step is widest at a vertex of the carrier, so it is found
  there and its edges by bisection between the start of the part and the vertex. A pulse
  that begins and ends within one step while the end of the part shows a switch (three
  edges in one part), or that no vertex of a carrier holds, is not seen.
- **Zero crossings** (a diode or thyristor; a contact that opens at a current zero): each
  such element has a zero-crossing function, positive while its state holds (the current
  of a conducting diode or thyristor, the reverse voltage of a blocking diode or fired
  thyristor, the current of a waiting contact in its direction at the command). The solver evaluates it at the start of a part, after
  the first stage and at the end. When the value at the end contradicts the state, the
  crossing is placed by linear interpolation between the two neighbouring values, the part
  is discarded and solved again up to the crossing. After an action the value at the start
  is not known (voltages and currents jump there); the line through the stage value and
  the end value is then followed back. A gate that starts or stops firing a thyristor
  changes its function without an action (its reverse voltage is armed or not); the
  value at the start of the part that follows is evaluated again, so that a crossing in
  the same step is placed from it, with one stage as with two. The place is off by
  (1/8)·h²·|g″/g′| of time at most for a function g (h²ω/8 for a sine: 1.6e-8 s at
  h = 20 µs and 50 Hz).

At every instant the logic of a grid point applies: the actions are taken, the diode
states are settled with the trial solution (backward Euler over 1/α of the rest of the
step), simultaneous changes are made at once, and E202 stops a run whose states do not
settle. The part that follows uses the stages of [the step after an action](#the-step-after-an-action).
The grid point itself keeps its checks too: what the placing left there (a crossing within
the last part of 1e-6 of a step, a gate signal that changed with the new measurement) acts
at the grid point as with `"switching": "grid"`.

Limits:

- No part is shorter than 1e-6 of a step; an instant closer than that to a grid point or
  to the instant before it is moved there. A short part has a large stage coefficient
  (conductances αC grow and 1/(αL) shrink against the blocking and conducting
  resistances), so the condition estimate of its matrix grows as 1/f at most. The solver
  keeps the estimate for a part below 1e11 (the smallest scaled pivot 100 times above the
  threshold of a singular matrix, the diode thresholds at 1e-3): with the largest estimate
  c of the regular matrices so far, the shortest part is c·1e-11 of a step, and never more
  than 5 %. A buck converter (c below 1e6) keeps 1e-6; a diode bridge with a capacitor on a
  floating DC side (c = 3e9 at h = 1 µs) has 3 %.
- At most 20 + 2·(two-state elements + zero-crossing functions) instants are placed within
  one step. Beyond that the states stay until the grid point and the run continues there
  as with `"switching": "grid"`; warning W270 counts such steps.
- A current that a source dictates through a capacitor is of first order under 2S-DIRK
  ([Currents that a source dictates](#currents-that-a-source-dictates)), and so is its
  zero: a diode that feeds a capacitor directly from an ideal source turns off
  ωh/(2√2) of angle early (0.35 h). A source impedance, or the trapezoidal rule, removes
  it. Where the early zero opens a diode whose forward voltage then rises again within the
  same step, the diode is closed again at the same instant and opens at the zero that
  follows; the two actions at one instant cancel and leave no event (a capacitor-input
  bridge at h = 50 µs: one closing and one opening per diode and period, the opening about
  ωh/(2√2) from the ideal angle; `a_diode_that_is_opened_too_early_leaves_one_event`).
- Under the trapezoidal rule the part after an instant is two backward-Euler half steps.
  Their error, (h²/4)·|x″| per instant, is of second order but can exceed the error of the
  rule itself by far where a derivative jumps (1.2e-2 A against 1.4e-5 A for 2S-DIRK in the
  discontinuous buck converter at h = 1 µs).
- A travelling-wave line reads its history by its own interpolation between the solutions
  of stages; the stages of the parts are such solutions. What it sends at an instant is
  sampled before the action there, so a front leaves with the first stage after it (less
  than a step late; a step with switching at grid points).

What an element has to offer for this is written at `Element` (`element/mod.rs`): any stage
coefficient (`prepare_alpha`), the part being solved (`begin_part`), its given times
(`action_times`), whether it would switch on previewed signals (`would_switch`) and its
zero-crossing functions (`event_functions`, `take_event`).

Measured (2S-DIRK, `crates/openkeito-core/tests/switching.rs`; errors against the exact
solution for ideal instants):

| Circuit | At grid points | Within the step |
|---|---|---|
| Buck converter, continuous conduction, 10 kHz, D = 0.405: i_L error at h = 1 µs | 0.21 A (duty 0.41) | 1.5e-6 A (duty exact) |
| The same at h = 10 µs (10 steps per period) | 3.7 A (duty 0.50) | 1.5e-4 A |
| Its average output at h = 10 µs (E·h·f = 10 V) | 9.5 V off | 3.8e-6 V off |
| Buck converter, discontinuous conduction: i_L error, lowest i_L at h = 1 µs | 1.9 A, −1.6 A | 1.4e-5 A, −6e-5 A (the leak) |
| Diode bridge, resistive load, h = 20 µs | 0.54 V in the row after a crossing | rounding |
| PI-controlled buck converter: averages over single periods at h = 2 µs | a limit cycle of the quantized duty | equal to 1e-4 V |
| Six-pulse rectifier with source inductance: overlap time at h = 20 µs | 14.6 µs off | 0.15 µs off |
| Contact opening at a current zero (30 A, 50 Hz), h = 20 µs: current interrupted | 0.19 A | 2.8e-6 A |
| Six-pulse thyristor converter (400 V, 1 mH, 50 A, α = 15°, 30°, 60°, 150°), h = 20 µs: average DC voltage against the textbook | 0.83 V low at α = 30° (the firing waits for a grid point, half a step on average) | 1.3e-4 relative at most (0.034 V at α = 60°) |
| The same: overlap time; firing instant of the gate | — | 0.48 µs off at most; 7e-18 s |

The halving of the step divides these errors by 4. With switching at grid points the
firing of a thyristor waits for the first grid point at which the gate is >= 0.5, and the
average DC voltage of the six-pulse converter at α = 30° is 1.69, 0.83, 0.40 and 0.19 V low
at h = 40, 20, 10 and 5 µs, of first order (half a step of delay on average; within the
step 0.02 V high at every h, the drop on a third thyristor during the overlap;
`crates/openkeito-core/tests/thyristor.rs`). A thyristor whose gate always fires gives
the rows and events of a diode to the last bit. One second of the 10 kHz buck converter
takes 0.21 s at grid points with h = 1 µs and 0.26 s within the step (one more
factorization per instant, and the preview and the zero-crossing functions in every step);
at h = 10 µs it takes 0.058 s, with an error more than 1000 times below that of the grid
points at 1 µs (1.5e-4 A against 0.21 A over the 2 ms of the test).

### A three-phase PWM inverter at large steps

A two-level voltage-source inverter built from its elements (per phase a
`controlled_switch` to each rail and an antiparallel `diode`, ±375 V with the midpoint at
gnd), an L filter of 0.2 Ω and 2 mH and a 400 V, 50 Hz grid behind 0.02 Ω and 0.2 mH, with
sine-triangle PWM at 2450 Hz (a carrier period of 408.16 µs, a whole number of none of the
steps). The current is about 100 A peak (the rating, 50 kVA, is 102 A peak). Each run is
compared with the same model solved within the step at h = 0.25 µs, on the phase currents
of 40 to 60 ms (one period of the fundamental) at the grid points of the coarser run: the
largest difference of the averages over carrier periods, and the amplitude and phase of the
fundamental (a DFT over the period). The reference is checked by its own convergence: the
differences of 1 µs and 0.5 µs from it fall by 3.5 to 4.4, so it is about 1/4 of the 0.5 µs
difference off (below 1e-5 A with fixed modulation). 2S-DIRK,
`crates/openkeito-core/tests/pwm_inverter.rs` (the table is its ignored test, one P core):

| Modulation fixed (M = 0.89) | h | Carrier average (of the rated peak) | Amplitude | Phase | Events | Time for 1 s |
|---|---|---|---|---|---|---|
| Within the step, no dead time | 5 µs | 7.6e-4 A (7.4e-6) | 6.2e-6 | 1.2e-4° | 2741 | 0.38 s |
| | 10 µs | 3.0e-3 A (2.9e-5) | 2.3e-5 | 1.8e-4° | 2739 | 0.31 s |
| | 20 µs | 1.0e-2 A (9.8e-5) | 6.5e-5 | 9.0e-4° | 2739 | 0.27 s |
| | 25 µs | 1.3e-2 A (1.3e-4) | 6.2e-5 | 9.4e-4° | 2739 | 0.26 s |
| Within the step, dead time 2 µs | 10 µs | 3.1e-3 A (3.1e-5) | 2.6e-5 | 4.3e-4° | 2743 | 0.44 s |
| | 20 µs | 1.1e-2 A (1.0e-4) | 8.7e-5 | 1.0e-3° | 2743 | 0.42 s |
| | 25 µs | 1.6e-2 A (1.6e-4) | 1.3e-4 | 1.7e-3° | 2743 | 0.38 s |
| At grid points, no dead time | 1 µs | 1.1 A (1.1e-2) | 7.6e-3 | 0.21° | 2741 | 0.64 s |
| | 20 µs | 15 A (0.14) | 0.11 | 5.0° | 2737 | 0.13 s |

The reference has 2741 events (2745 with the dead time). Halving the step from 20 to
10 µs divides the error of the carrier averages by 3.4 (both dead times), from 10 to 5 µs by
3.9 (no dead time) and 2.6 (dead time 2 µs). A dead time shorter than the step puts the
turn-off of one switch, the turn-on of the other and the commutation to the diode into one
step: at most 6 switching events in one step of 20 µs, far below the limit of placed
instants (56 here); no step reached it (no W270) and no run stopped (no E202) from 0.5 to
25 µs.

Two effects show:

- **Pulses narrower than a step.** At M = 0.89 the narrowest pulse lasts (1 − M)·T_c/2 =
  22.4 µs, so at 25 µs some pulses begin and end within one step. They are found at the
  vertex of the carrier (above): every pulse is seen (2739 events, as at 10 and 20 µs) and
  the error of the carrier averages grows from 20 to 25 µs by 1.35 (no dead time) and 1.52
  (dead time 2 µs), against (25/20)² = 1.56. When only the end of the part was previewed
  (to 0.14.0), 36 events were missing at 25 µs and the error jumped to 5.5 A (6.0 A with the
  dead time); the runs from 0.5 to 20 µs with fixed modulation are unchanged by the vertices.
- **Controls that read the circuit.** With PI current control in the rotating frame (gains
  4 Ω and 400 Ω/s, the grid voltage fed forward, the angle a ramp of ωt), the controls are
  evaluated at grid points on the measured current, ripple included, and held for a step.
  That sampling is of first order: from 1 to 0.5 µs the difference falls by 1.6 to 2.1, not
  4, and from 20 to 10 µs by 1.7 to 1.8. At 20 µs the amplitude is off by 0.83 %, the phase
  by 0.39°, the carrier averages by 1.2 A (1.2 % of the rated peak; at 10 µs 0.27 %, 0.19°
  and 0.72 A; at 25 µs 0.91 %, 0.43° and 1.7 A). The ripple that the controls sample makes
  narrow pulses, which the vertices of the carrier find (2673 events against 2679 at
  20 µs; with the end of the part previewed only, 2553 events and 3.3 A). At grid points
  the same case is off by 19 A and 4.7° at 20 µs.

`grid` is twice as fast per step and about 1500 times less accurate here; `interpolated`
at 20 µs is 100 times more accurate than `grid` at 1 µs in less than half the time.

### Methods that were compared

Before the method was chosen, three ways of handling the instants were run on the same
circuits (2S-DIRK; i_L error against the exact solution for ideal instants; time for one
second of the converter without output, best of three):

- **Split**: the method above.
- **Linear**: linear interpolation of the solution back to the instant and on to the grid
  [Kuffel, Kent, Irwin], with every step of the full length. It keeps the matrix of the
  regular stages (no factorization for a part) but interpolates states, an error of
  (1/2)·θ(1−θ)·h²·|x″| per instant, and needs every element with a history to interpolate
  it.
- **Stage diodes**: the diodes as resistors with two segments whose segment is decided
  within every stage, by solving the stage again until no diode is contradicted (their
  states can differ from stage to stage; no settling at grid points), with the gate
  instants at grid points or split.

Buck converter in continuous conduction (10 kHz, D = 0.405; no diode current zero, so the
stage diodes give the figures of grid and split):

| h (µs) | Grid: error (A), time (s) | Split: error (A), time (s) | Linear: error (A), time (s) |
|---|---|---|---|
| 1 | 0.207, 0.211 | 1.55e-6, 0.256 | 8.5e-5, 0.355 |
| 5 | 1.76, 0.055 | 3.88e-5, 0.080 | 5.4e-4, 0.107 |
| 20 | 7.44, 0.026 | 6.22e-4, 0.059 | 2.3e-3, 0.059 |

Buck converter in discontinuous conduction (L = 50 µH, D = 0.4013):

| h (µs) | Grid: error (A), time (s) | Split: error (A), time (s) | Linear: error (A), time (s) | Split gates, stage diodes: error (A), time (s) |
|---|---|---|---|---|
| 1 | 1.87, 0.280 | 1.45e-5, 0.360 | 2.4e-3, 0.364 | 2.8e-4, 0.290 |
| 5 | 9.87, 0.079 | 3.59e-4, 0.121 | 1.4e-2, 0.115 | 5.3e-4, 0.090 |
| 20 | 23.8, 0.038 | 1.08e-2, 0.073 | 1.95, 0.059 | 0.32, 0.045 |

Split is the only one whose error is that of the integration alone and falls with h² at
every step size. Linear interpolation was 50 to 170 times less accurate at 1 µs and no
faster, and the trial failed at 20 µs (two instants in one step). The stage diodes needed
a third fewer factorizations, but their turn-off error did not fall with the step
(2.8e-4 A at 1 µs, 2.3e-4 A at 2 µs), and at 20 µs the average output was 0.39 V off
(split: 7e-3 V). Without the backward-Euler half steps after a current zero, split gave
1.6e-5 A at 1 µs and no chattering in these circuits; they are kept, since the current left
at a placed zero is of order h² and depends on the circuit. The code of the linear
interpolation and of the stage diodes was removed after the comparison, so these figures
cannot be reproduced from the repository any more. Changing the step without changing the
matrix [Strunz] is a further alternative that was not tried.

## Linear solver

Two LU factorizations with the same contract, for real (time steps) and complex (phasor
solutions, scans) matrices:

- **Dense**: implicit scaled partial pivoting. A pivot below 1e-13 of the largest entry of
  its row (in the original matrix) means the equations are singular; the error names the
  unknown that cannot be determined. The smallest scaled pivot gives the condition estimate.
- **Sparse**: the left-looking LU with partial pivoting of Gilbert and Peierls, as
  implemented by the faer crate (MIT), in its simplicial form. The fill-reducing column
  order (COLAMD) is computed once, from the union of the entries of every state of the
  circuit (each element writes the entries of all its states, such as both states of a
  switch): this is the symbolic step. When values change (a switching action, another
  stage coefficient), only the numeric factorization is repeated; it chooses its pivots
  again each time. Before the factorization every row is divided by its largest entry (row
  equilibration), so that pivoting by magnitude is the scaled pivoting of the dense LU, and
  the smallest |u_kk| of the factors is the smallest scaled pivot: the singularity
  threshold (1e-13) and the condition estimate for the diode thresholds keep their meaning.
  The singular unknown is found through the column order.

In the singularity test (threshold 1e-13), column scaling (`col[k]`, dividing by the
largest entry of column `k` in the original matrix alongside row scaling) is applied so that
a small conductance (< 1e-13 S, such as an inductor with L > 10⁷·h at t = 0+ where
ε = 10⁻⁶·h) in a row that also carries ±1 from an ideal branch (voltage source, switch or
ideal transformer) is not falsely flagged as singular.

`simulation.linear_solver` chooses: `auto` (default) takes the dense LU below 16 unknowns
and the sparse one from there on (measured: the sparse LU is faster per step from about 12
unknowns, 30 % at 27 and 2.4 times at 99; see `docs/performance.md`), `dense` or `sparse`
force one. The two order their eliminations differently and agree to rounding, which the
condition estimate bounds: rounding errors in a solution are about the condition estimate
times the machine epsilon (2.2e-16). The run summary reports the solver used and the largest
condition estimate.

## Parts of a circuit that are solved on their own

A travelling-wave line has no matrix entry between its two ends: to its nodes an end is a
conductance matrix and a current that is known, because it left the other end at least one
travel time, and so at least one time step, earlier ([Travelling-wave lines](#travelling-wave-lines)).
Where only such lines join two sets of nodes, the system matrix is block diagonal and the
blocks can be factorized and solved one by one, in any order or at the same time
[Dommel 1986; Falcão et al. 1993; Marti and Linares 1994]. `simulation.partitioning` uses
this.

**Division.** The nodes of the model are joined by every component except, for a
`bergeron_line`, across its two ends (the three phases of an end stay together: its
conductance matrix couples them). The connected sets are the parts. A line whose two ends
lie in one part, because a pi line, a transformer or any other component also joins them,
is an ordinary element of that part; the others are the links between parts. Islands
that nothing joins are parts as well. With `auto` (the default), two parts that a line
joins are put together again while they have no more than 48 named nodes between them (16
three-phase buses): every part and every link costs 10 to 50 ns in each step (the port
buffers below), which is 3 to 30 % of the step when a part is a single bus with a load.
`every_line` divides at every line, `off` not at all.

**What a part owns.** Each part has its unknowns, numbered from 0 (node voltages, then
the inner nodes and branch currents of its components in the order of the model), its
matrix with its own choice of dense or sparse LU (`linear_solver` `auto` by the size of
the part), its factors, its right-hand side and its solution. The elements of a part are
built with the numbers of its unknowns and are handed the slices of the right-hand side
and of the solution that belong to the part; nothing an element writes lies in another
part. A link numbers the nodes of its two ends from 0 on its own: before a stage the
solver lets it write its known currents into a buffer of its own (its *ports*), from which
each part adds those of its nodes to its right-hand side, and after the stage it copies
the voltages of those nodes into a second buffer, from which the link takes its new
samples. The conductance matrices of the links are added to the matrices of the parts
when these are factorized. The phasor solution of the steady state is the exception:
there the two ends of a line are coupled, and one matrix over all parts is solved once.

**A step without a switching action** (almost every step). The line computes the history
terms of both stages of a step when the step before it has ended: every stage reads one
travel time back, at or before the start of its step. So before the step, each link writes
its port currents for every stage; then each part solves all stages of the step and
advances its elements, with the stage iteration of its nonlinear elements and machines
([Operating points within a stage](#operating-points-within-a-stage)) kept within the
part, and computes the outputs and measured quantities that its elements carry; then each
link is advanced over the stages and writes the port currents of the next step. Within
the step a part reads its own elements and port currents that were written before it
started, and writes only its own memory; a link reads the solutions of the parts when all
of them are done. There is no order among the parts, and none among the links, that the
result could depend on.

**A step with a switching action**, a part of a split step
([Switching instants within a step](#switching-instants-within-a-step)), the solution at
t = 0+ and the trial solution for the diode states are solved stage by stage over the
whole circuit: links write their port currents, every part solves the stage, links and
elements are advanced. After an action the line gives the grid point a second sample (the
value just after the action), which the second stage of the same step may read when the
travel time is one step, so the stages of such a step cannot be taken together. The
arithmetic of a part is the same in both forms (a test solves every step the second way
and compares the rows bit by bit). All parts use the same stages, and a step that is
split is split at the same instants in every part: the samples of a line are those of
both of its ends.

**Factorization.** A part keeps the factors for the stage coefficient of the regular
stages, and those for one other coefficient (the backward-Euler half steps after a diode
cut a current, the part of a split step). Each part counts the changes of its matrix (a
switch or a fault that acts, a diode that changes its state, an operating point that
moves a stamp); factors are those of the count they were computed for, and are computed
again only when it has moved on. A fault therefore factorizes the matrix of the part it
lies in, at each of its actions, and no other (in a ring of 12 parts: 4 factorizations of
one part for a fault and its clearing at three current zeros, where the undivided circuit
factorizes its one matrix 4 times). A change of the stage coefficient, as in a split
step, still factorizes every part once for that coefficient.

**Threads.** `simulation.threads` (default 1) is the largest number of threads. The parts
of a step without an action are dealt out in groups of neighbouring parts of about equal
size (unknowns plus elements), four groups per thread so that a thread on a slower core
holds up less; the links likewise. The threads are started once per run and wait for the
next step in a loop (they go to sleep when none comes for some milliseconds, as in a
long run of steps with switching actions, which one thread solves); a step hands them its groups by
storing one number, and the thread of the time loop takes groups as well. The solver uses
fewer threads than asked when the parts are fewer, when the machine has fewer cores, or
when the work of a step is below 100 unknowns and elements per thread (measured: below
that, handing over and waiting, of the order of 1 µs per step, cost more than the
threads save; see
`docs/performance.md`). Sequential and parallel runs give the same rows, events and counts
to the last bit, on any number of threads (tests: a ring with a fault under both methods;
diodes with switching at grid points and within steps; a synchronous machine with surge
arresters; a machine with a grid-following converter). WebAssembly has no threads; there,
and with `threads` 1, the same code runs the groups one after the other.

**Divided against undivided.** The equations are the same, the order of the eliminations
and of the sums is not, so the results agree to rounding and not to the last bit. The
bound is that of the [linear solver](#linear-solver): the condition estimate times 2.2e-16
of the largest values per solution. Found: 3e-13 of the largest value for a ring of 12
buses with a fault (bound 2e-8), 1e-10 and 1e-9 for a diode bridge between two lines with
switching at grid points and within steps (bound 2e-4; the same events at instants within
1e-12 s), 3e-13 over 3000 steps with a synchronous machine and arresters. Equality to the
last bit would need the same pivots and the same order of the sums in the right-hand
sides, which the numbering within the parts and the port buffers give up; it holds only
where no line joins parts (then the circuit is one part and the run is the one of `off`,
bit by bit). Outputs keep the order of the model, and events that happen at one instant
are recorded in the order of the model's components, as in the undivided run.

**What an element with decoupled ends promises** (`Element::decoupled_ends`): no matrix
entry between its ends; no unknowns, operating point or switching state of its own; a
stamp that depends on the stage coefficient only; and, in a step that no action precedes
and that is not split, right-hand sides of all stages that follow from the solutions up to
the start of the step.

## Verification

`crates/openkeito-core/tests/analytic.rs` compares both methods with closed-form solutions of
linear circuits and checks that halving the time step divides the error by about 4.
`crates/openkeito-core/tests/power_electronics.rs` checks the buck converter (continuous and
discontinuous conduction, PI control) and the single-phase diode bridge (resistive and
capacitive load) against exact and textbook solutions; every tolerance is derived in a
comment.
`crates/openkeito-core/tests/switching.rs` repeats these circuits with switching instants
within the step, against the exact solutions for ideal instants (second order), and adds a
six-pulse rectifier with commutation overlap against V_d = (3√2/π)·V_LL − (3/π)·ωL_s·I_d
[Mohan, Undeland, Robbins], a switch and a step source at times between grid points, and
contacts and fault phases that open at their current zeros.
`crates/openkeito-core/tests/thyristor.rs` checks the six-pulse phase-controlled converter
against U_d = (3√2/π)·V_LL·cos α − (3/π)·ωL_s·I_d and the overlap angle from
cos α − cos(α + u) = 2ωL_s·I_d/(√2·V_LL) [Mohan, Undeland, Robbins, ch. 6] as a rectifier
(α = 15°, 30°, 60°) and an inverter (α = 150°), the firing instants, the diode rectifier
for a gate that always fires, and the first-order delay of the firing at grid points.
`crates/openkeito-core/tests/three_phase.rs` compares the fault currents of all fault types,
bolted and through resistances, with symmetrical components; the voltage ratio and phase
shift of every vector group, the short-circuit current from u_k and the magnetizing current
from i_0 with the equivalent circuits; the zero-sequence path of an YNd transformer; and the
open-end voltage of pi lines with the ABCD parameters of the sections and of the
distributed line. Phasors are taken by a Fourier transform over the last period after the
transients have decayed. At h = 20 µs and 50 Hz (ωh = 6.3e-3) the relative errors are
3.2e-6 for the trapezoidal rule, which is (ωh)²/12 as its frequency warping predicts, and
4e-7 to 1.2e-6 for 2S-DIRK; the time-domain error of a ground fault switched on at t = 0
falls by 4.00 when the step is halved.
`crates/openkeito-core/tests/bergeron.rs` compares the travelling-wave line, for both
methods, with: the open line that doubles a dc step with a period of four travel times, the
matched line, and the lattice diagram with resistances at both ends, all to rounding (1e-15
of the source voltage) when the travel time is a whole number of steps, down to one step;
the same lossy line built from two lossless halves and three resistors (1e-14); the phasor
solution with the ABCD parameters of that circuit (1e-15) and with cosh(γℓ) of the
distributed line (the estimate of the lumped resistance to 5 %); pi lines of 2 to 32
sections, whose open-end voltage approaches it with ratios of 4.01 to 4.00 per doubling;
the separate arrival of the zero-sequence and line modes when one phase is energized, for
sequence values and for the same line as matrices, and of the three modes of an
untransposed line; the frequency scan with B/D of the circuit. A run from the steady state
with a whole number of steps stays on it to 2e-14; with a travel time of 100⅓ steps it
departs by (ωh)²/9·(1 + sin θ)/cos θ = 1.5e-6 at h = 10 µs as predicted (to 3 %), and by
4.00 times less at half the step. A front that arrives between grid points is whole in the
next row at its first arrival and spread over one more step at each later reading; the dc
value is reached to rounding. A switch and a diode at the two ends exercise the steps
after actions; a line switched on through an inductance follows the exponential rise at
both ends (0.015 V and 0.08 V of 100 V at ten steps per time constant), and a line switched
on and off at instants within steps agrees with a run of 100 times smaller steps in every
row when the travel time is a whole number of steps. In the time domain, pi lines approach the line only slowly where fronts are
steep (they ring at the fronts): for `examples/line_energization.json`, 1600 sections at
h = 0.625 µs are still 1.3 % (rms over 20 ms) from it, while the line itself converges
with the second order when the step divides the travel times (1.3 % at 10 µs, 0.33 % at
5 µs, 0.08 % at 2.5 µs against 0.625 µs; the lossless reflections keep every phase error
of the integration for the whole run).
`crates/openkeito-core/tests/steady_state.rs` compares the phasor solution with the hand
calculation of a three-phase network with mutual coupling and shunt capacitors (to rounding),
checks that a run started from it stays on it (departure 1.7e-6 for 2S-DIRK and 1.2e-6 for
the trapezoidal rule at h = 20 µs, falling by 4.00 when the step is halved, against 0.86 of
the amplitude from rest), that a ground fault applied to the initialized network reaches the
current of symmetrical components, and the magnetizing current of a transformer with core
loss. The frequency scan is compared with the bus impedances of the same network (self,
mutual and sequence values, to rounding), with series and parallel resonances, and with an
EMT run that injects a 250 Hz current (2.8e-6 at h = 2 µs).
`crates/openkeito-core/tests/controls.rs` compares the control blocks with their closed
forms: the step responses of a lag, a lead-lag and a transfer function of second order
(errors of (h/T)²/(12e) and (ωn h)²/12 as derived above, falling by 4.00 when the step is
halved), the integral of a ramp (exact), a delay (the chord of the input), a limited
integrator and a limited lag that leave their limits at the time a wound-up state would
not, the slopes of a rate limiter, the rotating frame of a bus voltage against its phasor
and the way back (to rounding), a PLL after a step of the phase and of the frequency
against its linear model (within the cube of the step, which is what the sine in the loop
leaves), the PLL block against the same loop built from blocks, and a loop of a lag and an
integrator against the closed loop of second order. For the controlled sources it compares
the current of R and L in series with the response to the delayed step and sine for both
methods, a current loop through the circuit with the solution of its delay equation
(integrated separately with a fine step), the three-phase sources with their per-phase
circuits, measurements of the currents of lines and transformers with the recorded columns
(identical), the flux that a nonlinear inductor offers as a signal with the integral of its
voltage, a controlled source in steps that are split at switching instants, and a run with
controls that starts in the steady state and stays there.

`crates/openkeito-core/tests/nonlinear.rs` compares the piecewise-linear elements with closed
forms evaluated from the points of the model: a nonlinear resistor behind a resistor with the
algebraic solution of its segment (to rounding, 1e-12 of the source amplitude; symmetric
tables, a dead band and a table that is not symmetric; dc and sine sources), two such
resistors in one circuit with the only pair of segments that holds its own solution, an
arrester given by a power law with the voltage at which its characteristic and a line share
a surge current (to rounding against the table; 2.4e-3, 1.7e-4 and 5.1e-5 against the power
law for tolerances of 1e-2, 1e-3 and 1e-4), a reactor charged through a resistor with the
exponential of each segment, a reactor on a sine voltage with the current of the flux
(V/ω)(1 − cos ωt) (peak 20 A; error 2.6e-3 A for the trapezoidal rule at h = 100 µs, which is
(ωh)²/12 of the flux, and 3.2e-4 A for 2S-DIRK; both fall by 4.00 when the step is halved),
and a transformer switched in at a voltage zero: the core flux reaches 1.944 pu (2 pu less the
leakage drop) and the current 1.8660 pu, as the characteristic gives, and a source resistance
lowers the peak by about two thirds of the bound R·i·t of its drop. The three units of a
YNyn0 bank reach 1.866, −0.703 and −0.703 pu, each from its own flux. A transformer whose
flux stays below the first point gives the waveforms of the linear one to rounding.

`crates/openkeito-core/tests/synchronous_machine.rs` checks the synchronous machine with the
555 MVA generator of Kundur's examples: the steady state at rated output against the
equations of section 3.6.5 (to rounding) and against the values printed in example 3.2
(δ = 39.1°, e_d = 0.631, e_q = 0.776, i_d = 0.905, i_q = 0.425, i_fd = 1.565, e_fd = 0.000939,
T_e = 0.9027, with the saturated inductances of the example as constants; within 0.4 %,
the printed values being rounded and remembered); a run against an infinite bus that stays
at its start (departure 5.6e-5 of the voltages and 1.2e-4 of the currents at h = 100 µs, a
quarter at half the step, and no farther in the third second than in the first); a
three-phase short circuit at the terminals from no load against Park's equations integrated
separately by the classical Runge-Kutta method (phase current within 2.0e-4 pu at h = 100 µs
and 5.6e-5 pu at 50 µs of a current of several per unit; 9.3e-4 and 2.3e-4 for the trapezoidal
rule; 1.4e-3 and 3.4e-4 for a machine with Xd″ = 0.2, Xq″ = 0.3 and one q-axis damper
winding) and against the classical formula with Xd″, Xd′, Xd, Td″, Td′ and Ta (within 6 % of
E/Xd″, the accuracy of the formula); the swing against an infinite bus with constant rotor
fluxes against √(K_S ω0/2H) and D/4H (9.514 rad/s for 9.542, 0.2143 1/s for 0.2143); the
open-circuit voltage after a step of the field voltage against the closed form of the two
d-axis windings (5e-10 pu) and with a proportional regulator around it (5e-6 pu, the delay
of one step); a load switched in next to the machine at an instant within a step (3.3e-4 pu
of current with `switching` "interpolated", 1.0e-2 pu at the grid point); the zero-sequence
path of a grounded, an impedance-grounded and an ungrounded star point; and the diagnostics.
`python/tests/test_machines.py` converts case9 and case39 (pandapower) and the IEEE 9- and
14-bus systems and the Eurostag example (PowSyBl) with the generators as machines: the bus
voltages are those of the power flow to 1e-9 pu, every machine delivers its power-flow
output, and the run stays in the steady state.

`crates/openkeito-wasm/tests/compare.mjs` runs every example from the WebAssembly module in
Node and with the command. The arithmetic of the solver gives the same bits in both: the
examples that call no elementary function (`buck`, `buck_interpolated`, `buck_pi`,
`current_control`, `rlc`) give the same files byte for byte. The others differ from the
last digit on, by about 2e-9 of a column's largest value at most, because `sin`, `exp`, `atan2`
and the like are Rust's own in the module and those of the system's C library in the
command; the test allows 1e-6 there. For the same reason the last digits of these models
can differ between operating systems.

## References

The data of the journal papers (authors, title, journal, volume, issue, pages, year, DOI)
were checked against the records of the publishers in November 2026, those of the books
against library catalogues. The chapters, sections and tables named in the text were not
checked against the books and standards themselves.

- H. W. Dommel, "Digital computer solution of electromagnetic transients in single- and
  multiphase networks," IEEE Trans. Power Apparatus and Systems, vol. PAS-88, no. 4,
  pp. 388–399, 1969, doi:10.1109/TPAS.1969.292459.
- R. Alexander, "Diagonally implicit Runge-Kutta methods for stiff O.D.E.'s," SIAM
  J. Numerical Analysis, vol. 14, no. 6, pp. 1006–1021, 1977, doi:10.1137/0714068.
- T. Noda, K. Takenaka, T. Inoue, "Numerical integration by the 2-stage diagonally
  implicit Runge-Kutta method for electromagnetic transient simulations," IEEE Trans.
  Power Delivery, vol. 24, no. 1, pp. 390–399, 2009, doi:10.1109/TPWRD.2008.923397.
- J. R. Marti, J. Lin, "Suppression of numerical oscillations in the EMTP," IEEE Trans.
  Power Systems, vol. 4, no. 2, pp. 739–747, 1989, doi:10.1109/59.193849.
- P. Kuffel, K. Kent, G. Irwin, "The implementation and effectiveness of linear
  interpolation within digital simulation," Int. J. Electrical Power & Energy Systems,
  vol. 19, no. 4, pp. 221–227, 1997, doi:10.1016/S0142-0615(96)00045-2.
- K. Strunz, "Flexible numerical integration for efficient representation of switching in
  real time electromagnetic transients simulation," IEEE Trans. Power Delivery, vol. 19,
  no. 3, pp. 1276–1283, 2004, doi:10.1109/TPWRD.2004.824387.
- N. Mohan, T. M. Undeland, W. P. Robbins, *Power Electronics: Converters, Applications,
  and Design*, 3rd ed., Wiley, 2003 (line-frequency diode rectifiers and commutation,
  chapter 5).
- C.-W. Ho, A. Ruehli, P. Brennan, "The modified nodal approach to network analysis,"
  IEEE Trans. Circuits and Systems, vol. CAS-22, no. 6, pp. 504–509, 1975,
  doi:10.1109/TCS.1975.1084079.
- K. G. Murty, "Note on a Bard-type scheme for solving the complementarity problem,"
  Opsearch, vol. 11, pp. 123–130, 1974.
- K. J. Åström, T. Hägglund, *PID Controllers: Theory, Design, and Tuning*, 2nd ed.,
  Instrument Society of America, 1995.
- IEEE Std 421.5, *IEEE Recommended Practice for Excitation System Models for Power System
  Stability Studies* (annex on windup and non-windup limits of integrators and lags).
- P. Kundur, *Power System Stability and Control*, McGraw-Hill, 1994 (chapter 3: the dq0
  transformation with the q axis leading the d axis).
- R. H. Park, "Two-reaction theory of synchronous machines: generalized method of analysis,
  part I," Trans. AIEE, vol. 48, no. 3, pp. 716–727, 1929,
  doi:10.1109/T-AIEE.1929.5055275.
- V. Kaura, V. Blasko, "Operation of a phase locked loop system under distorted utility
  conditions," IEEE Trans. Industry Applications, vol. 33, no. 1, pp. 58–63, 1997,
  doi:10.1109/28.567077.
- S.-K. Chung, "A phase tracking system for three phase utility interface inverters," IEEE
  Trans. Power Electronics, vol. 15, no. 3, pp. 431–438, 2000, doi:10.1109/63.844502.
- A. Yazdani, R. Iravani, *Voltage-Sourced Converters in Power Systems: Modeling, Control,
  and Applications*, Wiley-IEEE Press, 2010 (averaged model, current control in the dq
  frame with feedforward and decoupling, chapters 5 and 8; phase-locked loop).
- WECC Modeling and Validation Work Group, *Solar Photovoltaic Power Plant Modeling and
  Validation Guideline*, Western Electricity Coordinating Council, 2019, and the
  specifications of the generic models REGC_A and REEC_A of the WECC Renewable Energy
  Modeling Task Force (current limit logic with priority of the active or reactive
  current, filtered terminal voltage).
- R. Tarjan, "Depth-first search and linear graph algorithms," SIAM J. Computing, vol. 1,
  no. 2, pp. 146–160, 1972, doi:10.1137/0201010 (strongly connected components, for the
  loops of control blocks).
- R. W. Erickson, D. Maksimović, *Fundamentals of Power Electronics*, 2nd ed., Kluwer,
  2001 (chapter 5, discontinuous conduction mode).
- C. L. Fortescue, "Method of symmetrical co-ordinates applied to the solution of polyphase
  networks," Trans. AIEE, vol. 37, part II, pp. 1027–1140, 1918,
  doi:10.1109/T-AIEE.1918.4765570.
- P. M. Anderson, *Analysis of Faulted Power Systems*, Iowa State University Press, 1973
  (fault connections with fault and ground impedances).
- J. J. Grainger, W. D. Stevenson, *Power System Analysis*, McGraw-Hill, 1994 (long lines,
  transformers, unsymmetrical faults).
- H. W. Dommel, *Electromagnetic Transients Program Reference Manual (EMTP Theory Book)*,
  Bonneville Power Administration, Portland, OR, 1986 (coupled branches, transformers;
  chapter 4: lines with distributed parameters, lumped resistance, modes).
- D. M. Falcão, E. Kaszkurewicz, H. L. S. Almeida, "Application of parallel processing
  techniques to the simulation of power system electromagnetic transients," IEEE Trans.
  Power Systems, vol. 8, no. 1, pp. 90–96, 1993, doi:10.1109/59.221253 (subsystems that
  transmission lines decouple, solved in parallel).
- J. R. Marti, L. R. Linares, "Real-time EMTP-based transients simulation," IEEE Trans.
  Power Systems, vol. 9, no. 3, pp. 1309–1317, 1994, doi:10.1109/59.336135 (the network as
  blocks joined by transmission lines, each with its own matrix).
- H. W. Dommel, *EMTP Theory Book* (as above), chapters 6 and 12: piecewise-linear
  representation of nonlinear resistances and inductances, saturation of transformers.
- IEEE Working Group 3.4.11, "Modeling of metal oxide surge arresters," IEEE Trans. Power
  Delivery, vol. 7, no. 1, pp. 302–309, 1992, doi:10.1109/61.108922 (characteristics
  i = p (v/V_ref)^q).
- T. Fujisawa, E. S. Kuh, "Piecewise-linear theory of nonlinear networks," SIAM J. Applied
  Mathematics, vol. 22, no. 2, pp. 307–328, 1972, doi:10.1137/0122030 (existence and
  uniqueness of the solution).
- L. O. Chua, P.-M. Lin, *Computer-Aided Analysis of Electronic Circuits: Algorithms and
  Computational Techniques*, Prentice-Hall, 1975 (piecewise-linear analysis; networks of
  monotonic resistors).
- J. Katzenelson, "An algorithm for solving nonlinear resistor networks," Bell System
  Technical J., vol. 44, no. 8, pp. 1605–1620, 1965,
  doi:10.1002/j.1538-7305.1965.tb04195.x (moving from segment to segment along the
  solution).
- E. Clarke, *Circuit Analysis of A-C Power Systems*, vol. 1: *Symmetrical and Related
  Components*, Wiley, 1943 (the α, β, 0 components).
- G. H. Golub, C. F. Van Loan, *Matrix Computations*, 3rd ed., Johns Hopkins University
  Press, 1996 (Cholesky factorization, Jacobi method for symmetric eigenvalue problems).
- IEC 60076-1, *Power transformers — Part 1: General* (vector groups and clock numbers).
- J. R. Gilbert, T. Peierls, "Sparse partial pivoting in time proportional to arithmetic
  operations," SIAM J. Scientific and Statistical Computing, vol. 9, no. 5, pp. 862–874,
  1988, doi:10.1137/0909058.
- T. A. Davis, J. R. Gilbert, S. I. Larimore, E. G. Ng, "A column approximate minimum degree
  ordering algorithm," ACM Trans. Mathematical Software, vol. 30, no. 3, pp. 353–376,
  2004, doi:10.1145/1024074.1024079.
- T. A. Davis, E. Palamadai Natarajan, "Algorithm 907: KLU, a direct sparse solver for
  circuit simulation problems," ACM Trans. Mathematical Software, vol. 37, no. 3, 2010,
  doi:10.1145/1824801.1824814 (background: why circuit matrices suit left-looking LU without supernodes).
- S. El Kazdadi, faer: a linear algebra library for Rust, https://codeberg.org/sarah-quinones/faer
  (version 0.24, MIT).
- L. Thurner et al., "pandapower — an open-source Python tool for convenient modeling,
  analysis, and optimization of electric power systems," IEEE Trans. Power Systems,
  vol. 33, no. 6, pp. 6510–6521, 2018, doi:10.1109/TPWRS.2018.2829021; pandapower
  documentation, version 3.5 (line, transformer and switch models of the power flow).
- R. D. Zimmerman, C. E. Murillo-Sánchez, R. J. Thomas, "MATPOWER: steady-state operations,
  planning, and analysis tools for power systems research and education," IEEE Trans.
  Power Systems, vol. 26, no. 1, pp. 12–19, 2011, doi:10.1109/TPWRS.2010.2051168 (case9,
  case39 and case118).
- H. Hinnant, "chrono-Compatible Low-Level Date Algorithms,"
  https://howardhinnant.github.io/date_algorithms.html (days_from_civil).
- G. Klyne, C. Newman, "Date and Time on the Internet: Timestamps," RFC 3339, 2002.
- IEEE Std C57.135-2011 (IEC 62032:2012), *IEEE Guide for the Application, Specification,
  and Testing of Phase-Shifting Transformers* (symmetric phase shifters).
- PowSyBl, *Grid model (IIDM)* documentation: line, two-winding and three-winding
  transformer, dangling line; *OpenLoadFlow* documentation: parameters
  (newtonRaphsonConvEpsPerEq, slackBusPMaxMismatch, slackBusSelectionMode),
  https://powsybl.readthedocs.io (pypowsybl 1.16).
- ENTSO-E, *CGMES Conformity Assessment: MicroGrid test configuration* (BE and NL models,
  as shipped with pypowsybl).
- ENTSO-E, *Common Grid Model Exchange Specification (CGMES)*, version 2.4.15, equipment
  profile with the short-circuit attributes (IEC 61970-452, IEC 61970-301 CIM16); IEC
  60909-0, *Short-circuit currents in three-phase a.c. systems — Part 0: Calculation of
  currents* (the data these attributes carry).
- P. M. Anderson, *Analysis of Faulted Power Systems* (as above), and J. J. Grainger,
  W. D. Stevenson, *Power System Analysis* (as above): zero-sequence circuits of
  transformers by winding connection, grounding impedances as 3·Z_g, sequence networks of
  the line-to-ground fault.
- Model Context Protocol specification, https://modelcontextprotocol.io (the MCP server).
- P. Kundur, *Power System Stability and Control* (as above), chapters 3 to 5 (Park's
  equations in per unit, steady state and its initial values, standard and fundamental
  parameters, short circuit), 12 and 13 (small-signal swing, initial values of the controls).
- P. C. Krause, O. Wasynczuk, S. D. Sudhoff, *Analysis of Electric Machinery and Drive
  Systems*, 2nd ed., Wiley-IEEE Press, 2002 (synchronous machines in the rotor reference
  frame).
- IEEE Std 1110, *IEEE Guide for Synchronous Generator Modeling Practices and Parameter
  Verification with Applications in Power System Stability Analyses* (model structures 2.1
  and 2.2).
- H. W. Dommel, *EMTP Theory Book* (as above), chapter 8: the synchronous machine as a
  voltage behind a constant reactance, with prediction and compensation.
- L. Wang, J. Jatskevich, "A voltage-behind-reactance synchronous machine model for the
  EMTP-type solution," IEEE Trans. Power Systems, vol. 21, no. 4, pp. 1539–1549, 2006,
  doi:10.1109/TPWRS.2006.883670.
- C. G. Broyden, "A class of methods for solving nonlinear simultaneous equations,"
  Mathematics of Computation, vol. 19, no. 92, pp. 577–593, 1965,
  doi:10.1090/S0025-5718-1965-0198670-6.
- P. Kundur, *Power System Stability and Control* (as above), section 3.6 (steady-state
  field voltage and torque of an operating point), section 11.1 (governor with droop,
  non-reheat turbine and their typical time constants) and the examples of chapters 12 and 13
  (thyristor exciter with a high gain and its ceilings): the controls of a transition.
- IEEE Std 421.5, *IEEE Recommended Practice for Excitation System Models for Power System
  Stability Studies* (type ST1A; non-windup limits).
