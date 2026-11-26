//! Circuit elements and their stamps.
//!
//! Sign convention of every two-terminal element: the branch current is
//! positive from `a` to `b` through the element, and the branch voltage is
//! V(a) - V(b). Elements with several phases (`CoupledRl`,
//! `CapacitanceMatrix`) apply it phase by phase.
//!
//! The elements live in one module per family. A new element implements
//! [`Element`] in the module of its family (or a new module) and is created
//! by the expansion of its component in `netlist`.

mod converters;
mod lines;
mod machines;
mod nonlinear;
mod passive;
mod sources;
mod switches;
mod transformers;

pub(crate) use converters::*;
pub(crate) use lines::*;
pub(crate) use machines::*;
pub(crate) use nonlinear::*;
pub(crate) use passive::*;
pub(crate) use sources::*;
pub(crate) use switches::*;
pub(crate) use transformers::*;

use openkeito_model::Diagnostic;

use crate::linalg::{Complex64, RealPart, Scalar, Stamp};
use crate::sim::Stages;

/// Index of a node voltage among the unknowns; `None` is the reference node.
pub type Node = Option<usize>;

/// One implicit stage of a time step.
///
/// For a state `y` with derivative `f`, every supported method can be written
/// as `f = alpha * (y - y_start) - c * f_prev`, where `y_start` is the state at
/// the start of the step and `f_prev` the derivative of the previous stage.
/// See `docs/numerical-method.md`.
#[derive(Debug, Clone, Copy)]
pub struct Stage {
    pub alpha: f64,
    pub c: f64,
    /// Time of the stage as a fraction of the step, 0..=1.
    pub time_fraction: f64,
    /// True when the stage result becomes the new `y_start`.
    pub commit: bool,
}

impl Stage {
    /// True for the negligible step at t = 0+ (`Stages::initial`), which
    /// either starts at `time_fraction == 0.0` (first trapezoidal stage) or
    /// ends at `time_fraction == 1.0` without committing (`!commit`).
    #[inline]
    pub fn is_initial(&self) -> bool {
        self.time_fraction == 0.0 || (self.time_fraction == 1.0 && !self.commit)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SwitchAction {
    Close,
    /// Carries the current interrupted, in ampere.
    Open(f64),
    /// A source with a step waveform went to its value after the step.
    Step,
}

/// Thresholds below which a voltage or current counts as zero when the
/// states of diodes are checked, so that rounding cannot flip a state.
#[derive(Debug, Clone, Copy)]
pub struct Tolerance {
    pub voltage: f64,
    pub current: f64,
}

/// What an element found when it compared its operating point with the
/// solution of a stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatingPoint {
    /// The solution agrees with the operating point.
    Consistent,
    /// The element moved its operating point, and only its `rhs` changed:
    /// the stage is solved again with the same factors.
    RhsChanged,
    /// The element moved its operating point and its `stamp` changed: the
    /// matrix is factorized again before the stage is solved again.
    MatrixChanged,
}

/// What an element did with the control signals it read at the start of a
/// step.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SignalResponse {
    /// Nothing the solver has to know of.
    Unchanged,
    /// A value that the element takes for the step changed (the value of a
    /// controlled source): its `rhs` differs, its `stamp` does not. It is
    /// not a switching action: the step is solved with the regular stages
    /// (a value that follows a signal changes at every step, and
    /// backward-Euler half steps after each change would make the run of
    /// first order).
    ValueChanged,
    /// The element switched: its `stamp` changed. Recorded as an event and
    /// handled like any switching action.
    Switched(SwitchAction),
    /// A thyristor that blocks a forward voltage is fired: the diode
    /// states are settled now (as after a switching action), and the diode
    /// logic turns it on and records the event.
    Fired,
}

/// An element is `Send`: the parts of a divided circuit are solved on
/// several threads, each element by one thread at a time.
pub trait Element: Send {
    fn name(&self) -> &str;
    /// Number of extra unknowns (branch currents) the element needs.
    fn aux_count(&self) -> usize {
        0
    }
    /// Receives the index of the first extra unknown.
    fn set_aux(&mut self, _index: usize) {}
    /// Name of extra unknown `k`, for error messages.
    fn aux_label(&self, _k: usize) -> String {
        format!("I({})", self.name())
    }
    /// Receives the conditions of the run, once before it starts: the time
    /// step, the method and every stage the solver will use. The element
    /// computes what depends only on them: what belongs to a stage
    /// coefficient (`stages.alphas()`), or the length of a history and the
    /// interpolation weights of an element with a delay (the time step and
    /// the `time_fraction` of the stages). An error says why the element
    /// cannot be solved under these conditions.
    fn prepare(&mut self, _stages: &Stages) -> Result<(), String> {
        Ok(())
    }
    /// Adds the element to the system matrix for a stage coefficient `alpha`.
    fn stamp(&self, alpha: f64, m: &mut dyn Stamp);
    /// Writes every entry the element can have in any of its states (values
    /// do not matter), for the order of a sparse factorization.
    fn structure(&self, m: &mut dyn Stamp) {
        self.stamp(1.0, m);
    }
    /// Adds the element's sources and history terms to the right-hand side.
    fn rhs(&self, _t: f64, _stage: &Stage, _b: &mut [f64]) {}
    /// Current `k` (phase k, or winding k) for the stage solution `x`. Call
    /// before `advance`.
    fn current(&self, k: usize, t: f64, stage: &Stage, x: &[f64]) -> f64;
    /// Current of an element with two states (a diode or a controlled
    /// switch) in a solution `x`. It does not depend on the stage.
    fn conduction_current(&self, _x: &[f64]) -> f64 {
        0.0
    }
    /// True for an element with an operating point that follows from the
    /// solution of each stage (the segment of a piecewise-linear
    /// characteristic, the speed terms of a machine). Only these elements
    /// are asked by `update_operating_point`. The answer is fixed for the
    /// run.
    fn has_operating_point(&self) -> bool {
        false
    }
    /// Compares the operating point with the solution `x` of a stage at time
    /// `t`. When they disagree, the element moves its operating point and
    /// says what changed; the solver then solves the same stage again and
    /// asks again, until every element answers `Consistent` (or stops with
    /// E205 after `MAX_OPERATING_POINT_ITERATIONS` solutions). Called before
    /// `advance`, and also on solutions that are not kept (t = 0+, the trial
    /// solution that settles the diode states): the operating point is a
    /// guess for the next solution, not a state, so `save` and `restore`
    /// leave it alone. `structure` must cover the stamps of every operating
    /// point.
    fn update_operating_point(&mut self, _t: f64, _stage: &Stage, _x: &[f64]) -> OperatingPoint {
        OperatingPoint::Consistent
    }
    /// Updates the element's state from the stage solution `x`.
    fn advance(&mut self, _stage: &Stage, _x: &[f64]) {}
    /// Called at the start of a step at time `t`; pushes the switching
    /// actions taken now, each with the name of the switching element.
    /// `tol` absorbs rounding when comparing times.
    fn pre_step(&mut self, _t: f64, _tol: f64, _actions: &mut Vec<(String, SwitchAction)>) {}
    /// Like `pre_step`, and returns `true` when the element changed its
    /// stamp or parameters (so its part must be re-factorized) even if no
    /// separate event row was pushed to `actions`.
    fn pre_step_changed(
        &mut self,
        t: f64,
        tol: f64,
        actions: &mut Vec<(String, SwitchAction)>,
    ) -> bool {
        let before = actions.len();
        self.pre_step(t, tol, actions);
        actions.len() > before
    }
    /// True while a contact of the element has been told to open and still
    /// waits for a current zero.
    fn waiting_to_open(&self) -> bool {
        false
    }
    /// An unknown of the element when its own equations cannot be solved at
    /// angular frequency `omega` (its phasor stamp would not be finite).
    fn phasor_singular(&self, _omega: f64) -> Option<usize> {
        None
    }
    /// True for an element that reads control signals. Only these elements
    /// are given the signals by `read_signals`. The answer is fixed for the
    /// run.
    fn reads_signals(&self) -> bool {
        false
    }
    /// Reads the control signals (indexed like `model::signal_names` of the
    /// model's `controls`) at the start of a step, after the output of its
    /// grid point. The signals are those computed at that grid point, and
    /// what the element takes from them is what it uses over the step: the
    /// controls reach the circuit one step late. A switch holds its state
    /// over the step; a controlled source moves in a straight line from the
    /// value it took at the step before to this one (`Followed`). At t = 0 the signals are read after a first solution at
    /// t = 0+, which is solved again when an element answers anything but
    /// `Unchanged`. What is read is not a state: `save` and `restore` leave
    /// it alone.
    fn read_signals(&mut self, _signals: &[f64]) -> SignalResponse {
        SignalResponse::Unchanged
    }
    /// Names of the internal quantities the element offers as signals (a
    /// speed, an angle, a flux). Their full names are
    /// `<element name>.<signal name>`. Fixed for the run.
    fn signal_names(&self) -> Vec<String> {
        Vec::new()
    }
    /// Value of signal `k` of `signal_names` at the grid point the element
    /// was last advanced to (at t = 0, its initial value).
    fn signal(&self, _k: usize) -> f64 {
        0.0
    }
    /// Stores the state at the start of a step, so that the step can be
    /// solved again after `restore`.
    fn save(&mut self) {}
    /// Returns to the state stored by `save`.
    fn restore(&mut self) {}

    // Switching instants within a step (`simulation.switching` =
    // "interpolated"). The solver then splits a step at every switching
    // instant t* into parts [t0, t*] and [t*, t1], each solved with the
    // stages of the method scaled to its length. What this asks of every
    // element:
    //
    // - Any stage coefficient: a part of length f h has the coefficients
    //   `alpha / f` of the stages in `Stages` (same `c`, `time_fraction` and
    //   `commit`). `stamp`, `rhs`, `current` and `advance` must work for
    //   them; an element that prepares values per coefficient gets
    //   `prepare_alpha` first.
    // - The time of a stage is the `t` it is given. `time_fraction` is the
    //   fraction of the part, not of the time step.
    // - A stage with `commit` can end within a time step. An element that
    //   keeps values of several grid points (a history) is told by
    //   `begin_part` whether the part ends at a grid point.
    // - A part can be solved and then discarded: the solver calls `save`
    //   before a part whose stages commit before the last one, and `restore`
    //   when it discards it. `advance` of a stage without `commit` must
    //   leave nothing that a later solution from the same start depends on,
    //   except the derivative of the previous stage (which a first stage
    //   with c = 0 does not read). When an element keeps what every stage
    //   leaves (the history of a travelling-wave line, which answers
    //   `decoupled_ends`), `save` is called before every part that can be
    //   discarded.

    /// Makes the element ready for the stage coefficient `alpha` of a part
    /// of a split step, which is not among `stages.alphas()`. Called before
    /// every use of such a coefficient; only the last one prepared is in use
    /// (besides those of `prepare`). An error says why the element cannot be
    /// solved with it.
    fn prepare_alpha(&mut self, _alpha: f64) -> Result<(), String> {
        Ok(())
    }
    /// Called before the stages of every part [t_start, t_end] of a split
    /// step, and before the trial solution that settles the diode states at
    /// its start. `ends_at_grid_point` is false for a part that ends at a
    /// switching instant within the step: what its last stage commits does
    /// not belong to a grid point. The last part of a step ends at the grid
    /// point. Not called for a step that is solved whole: an element that
    /// keeps what it was told forgets it when a part has ended at the grid
    /// point.
    fn begin_part(&mut self, _t_start: f64, _t_end: f64, _ends_at_grid_point: bool) {}
    /// Called for every element when a switching action of any element has
    /// taken effect at the present time (a grid point after its output, or
    /// an instant within a step at which a part has ended), before the
    /// solutions that follow it. An element with a history keeps the values
    /// before and after the action apart. Not called at t = 0, where the
    /// solution at t = 0+ is the one after the actions.
    fn after_action(&mut self) {}
    /// The times at which the element acts on its own (`pre_step` pushes an
    /// action): a switch closing at a given time, the step of a source.
    /// Fixed for the run. The solver ends a part at each of them and calls
    /// `pre_step` there.
    fn action_times(&self) -> Vec<f64> {
        Vec::new()
    }
    /// True when `read_signals` with these signals would switch the element.
    /// The solver looks for the instant within a step from which this holds,
    /// with the signals previewed at times within the step.
    fn would_switch(&self, _signals: &[f64]) -> bool {
        false
    }
    /// Number of zero-crossing functions of the element: quantities whose
    /// change of sign changes its state (the current of a conducting diode,
    /// the voltage of a blocking one, the current of a contact that waits
    /// for a current zero). Fixed for the run; `event_functions` and
    /// `take_event` are only called when it is not 0.
    fn event_count(&self) -> usize {
        0
    }
    /// Writes for every zero-crossing function its value in the solution `x`
    /// and its threshold: `g[k] = [value, threshold]`. The value is positive
    /// while the present state holds, in any unit; the state is contradicted
    /// when the value is below -threshold. A function that cannot change
    /// the state now (a contact that is not waiting) has the value infinity.
    /// The solver asks at the start of a part, after its first stage and at
    /// its end, and places the instant of the change where the value crosses
    /// zero, by linear interpolation between these.
    fn event_functions(&self, _x: &[f64], _tol: &Tolerance, _g: &mut [[f64; 2]]) {}
    /// Makes the change of state of zero-crossing function `k`, after the
    /// solver has brought the circuit to the instant of the crossing. A
    /// change that is a switching action with its own event is pushed to
    /// `actions`; a diode only changes its state.
    fn take_event(&mut self, _k: usize, _actions: &mut Vec<(String, SwitchAction)>) {}
    /// For a diode or controlled switch whose model leaves a resistance to
    /// its default: what it blocked and carried in the run so far (for
    /// warning W271).
    fn default_stress(&self) -> Option<DefaultStress> {
        None
    }
    /// For an element whose state follows from the solution (a diode): true
    /// when the solution `x` at the end of a step contradicts the state.
    fn inconsistent(&self, _x: &[f64], _tol: &Tolerance) -> bool {
        false
    }
    /// Changes the state of such an element.
    fn flip(&mut self) {}
    /// Conducting or not, for elements with two states.
    fn conducting(&self) -> Option<bool> {
        None
    }
    /// Adds the admittance of the element at angular frequency `omega` to a
    /// phasor solution. The default passes the real stamp on, which is right
    /// for elements without states (their stamp does not use `alpha`).
    fn stamp_phasor(&self, _omega: f64, m: &mut dyn Stamp<Complex64>) {
        self.stamp(f64::NAN, &mut RealPart(m));
    }
    /// Adds the phasors of the sources of the element at `omega` (rms,
    /// cosine reference).
    fn rhs_phasor(&self, _omega: f64, _b: &mut [Complex64]) {}
    /// Phasor of current `k` in the phasor solution `x`.
    fn current_phasor(&self, k: usize, omega: f64, x: &[Complex64]) -> Complex64;
    /// Sets the state at t = 0 to the steady state of the phasor solution
    /// `x`: x(0) = sqrt(2) Re(X).
    fn init_from_phasor(&mut self, _omega: f64, _x: &[Complex64]) {}
    /// A warning when the steady state that `init_from_phasor` received is
    /// not one the element's phasor stamp describes: a nonlinear element
    /// whose steady state leaves the segment through the origin. Asked once,
    /// after `init_from_phasor`.
    fn steady_state_warning(&self) -> Option<Diagnostic> {
        None
    }
    /// For an element with a delay between two ends (a travelling-wave
    /// line): the unknowns of the nodes of each end. Such an element has no
    /// matrix entry between the two sets, and what one end reads of the
    /// other is at least one time step old, so parts of a circuit that only
    /// such elements join can be solved independently of each other within
    /// a time step (`sim/parts.rs`). When its ends lie in different parts,
    /// the element is a link: it is built with its nodes numbered from 0,
    /// `rhs` is given a buffer of that many entries, from which the parts
    /// take their currents, and `current` and `advance` (and their phasor
    /// counterparts) are given the node voltages in the same order. What
    /// the solver relies on:
    ///
    /// - no unknowns (`aux_count`), no operating point and no switching
    ///   state of its own, and a `stamp` that depends on `alpha` only;
    /// - in a step that no switching action precedes and that is not split,
    ///   `rhs` of every stage follows from the solutions up to the start of
    ///   the step: the solver asks for all of them before it advances any
    ///   stage, and again for the next step right after `advance` of the
    ///   last stage. After an action (`after_action`), and in the parts of a
    ///   split step, it asks stage by stage, each after `advance` of the
    ///   stage before.
    fn decoupled_ends(&self) -> Option<[Vec<usize>; 2]> {
        None
    }
}

/// Value at t = 0 of the waveform with rms phasor `x` (cosine reference).
#[inline]
pub(crate) fn instant(x: Complex64) -> f64 {
    std::f64::consts::SQRT_2 * x.re
}

#[inline]
pub(crate) fn phasor(x: &[Complex64], n: Node) -> Complex64 {
    n.map_or(Complex64::ZERO, |i| x[i])
}

#[inline]
pub(crate) fn volt(x: &[f64], n: Node) -> f64 {
    n.map_or(0.0, |i| x[i])
}

#[inline]
pub(crate) fn add<T: Scalar>(m: &mut dyn Stamp<T>, row: Node, col: Node, v: T) {
    if let (Some(i), Some(j)) = (row, col) {
        m.add(i, j, v);
    }
}

#[inline]
pub(crate) fn stamp_conductance<T: Scalar>(m: &mut dyn Stamp<T>, a: Node, b: Node, g: T) {
    add(m, a, a, g);
    add(m, b, b, g);
    add(m, a, b, -g);
    add(m, b, a, -g);
}

/// Stamps the conductance matrix `g` (n x n) of n branches between the nodes
/// `a[k]` and `b[k]`: i = g (v_a - v_b).
pub(crate) fn stamp_conductance_matrix<T: Scalar>(
    m: &mut dyn Stamp<T>,
    a: &[Node],
    b: &[Node],
    g: &[[T; 3]; 3],
    n: usize,
) {
    for j in 0..n {
        for k in 0..n {
            let v = g[j][k];
            if v != T::ZERO {
                add(m, a[j], a[k], v);
                add(m, b[j], b[k], v);
                add(m, a[j], b[k], -v);
                add(m, b[j], a[k], -v);
            }
        }
    }
}

/// A known current `i` flowing from `a` to `b` through an element.
#[inline]
pub(crate) fn inject<T: Scalar>(rhs: &mut [T], a: Node, b: Node, i: T) {
    if let Some(k) = a {
        rhs[k] = rhs[k] - i;
    }
    if let Some(k) = b {
        rhs[k] = rhs[k] + i;
    }
}

/// Phasor branch currents y (v_a - v_b) of n coupled branches.
pub(crate) fn matrix_currents(
    y: &[[Complex64; 3]; 3],
    a: &[Node; 3],
    b: &[Node; 3],
    n: usize,
    x: &[Complex64],
) -> [Complex64; 3] {
    let mut v = [Complex64::ZERO; 3];
    for k in 0..n {
        v[k] = phasor(x, a[k]) - phasor(x, b[k]);
    }
    let mut i = [Complex64::ZERO; 3];
    for k in 0..n {
        i[k] = (0..n).fold(Complex64::ZERO, |s, j| s + y[k][j] * v[j]);
    }
    i
}

/// Couples the branch-current unknown `k` into the node equations and puts
/// V(a) - V(b) into row `k`.
pub(crate) fn stamp_branch_current(
    m: &mut dyn Stamp,
    a: Node,
    b: Node,
    k: usize,
    voltage_row: bool,
) {
    if let Some(i) = a {
        m.add(i, k, 1.0);
        if voltage_row {
            m.add(k, i, 1.0);
        }
    }
    if let Some(j) = b {
        m.add(j, k, -1.0);
        if voltage_row {
            m.add(k, j, -1.0);
        }
    }
}

pub(crate) const PHASE_NAMES: [&str; 3] = ["a", "b", "c"];

/// V(a[k]) - V(b[k]) of the first n branches in the solution `x`.
#[inline]
pub(crate) fn branch_voltages(x: &[f64], a: &[Node; 3], b: &[Node; 3], n: usize) -> [f64; 3] {
    let mut v = [0.0; 3];
    for (k, vk) in v.iter_mut().enumerate().take(n) {
        *vk = volt(x, a[k]) - volt(x, b[k]);
    }
    v
}
