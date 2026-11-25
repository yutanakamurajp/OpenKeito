//! Sources: voltage and current sources and their waveforms, and the
//! sources whose values are control signals.

use openkeito_model as model;

use super::{
    inject, instant, stamp_branch_current, Element, Node, SignalResponse, Stage, SwitchAction,
};
use crate::linalg::{Complex64, Stamp};
use crate::sim::Stages;

/// Rms phasor (cosine reference) of a waveform at angular frequency `omega`:
/// A sin(wt + phi) is (A/sqrt 2) e^{j(phi - 90 deg)}. A source of another
/// frequency, and a dc or step source, contributes nothing (validation
/// allows only dc sources of 0 under steady-state initialization).
pub(crate) fn waveform_phasor(w: &model::Waveform, omega: f64) -> Complex64 {
    match *w {
        model::Waveform::Sine {
            amplitude,
            frequency_hz,
            phase_deg,
            ..
        } if (2.0 * std::f64::consts::PI * frequency_hz - omega).abs() <= 1e-12 * omega => {
            Complex64::from_polar(
                amplitude / std::f64::consts::SQRT_2,
                (phase_deg - 90.0).to_radians(),
            )
        }
        _ => Complex64::ZERO,
    }
}

/// Value of a source at time `t`. A step is not a function of `t` alone:
/// like a switch, it acts at a grid point, after the output of that point
/// (`stepped`, set by `step_due`). A stage that ends at the grid point of the
/// step would otherwise already see the value after it, and the response
/// would start up to half a step early.
pub(crate) fn waveform(w: &model::Waveform, t: f64, stepped: bool) -> f64 {
    match *w {
        model::Waveform::Dc { value } => value,
        model::Waveform::Step { before, after, .. } => {
            if stepped {
                after
            } else {
                before
            }
        }
        model::Waveform::Sine {
            amplitude,
            frequency_hz,
            phase_deg,
            offset,
        } => {
            offset
                + amplitude
                    * (2.0 * std::f64::consts::PI * frequency_hz * t + phase_deg.to_radians()).sin()
        }
    }
}

/// True at the first grid point `t` at or after the time of a step waveform
/// that has not stepped yet.
fn step_due(w: &model::Waveform, stepped: bool, t: f64, tol: f64) -> bool {
    matches!(*w, model::Waveform::Step { time_s, .. } if !stepped && t >= time_s - tol)
}

/// The time of a step waveform, as the action time of its source.
fn step_times(w: &model::Waveform) -> Vec<f64> {
    match *w {
        model::Waveform::Step { time_s, .. } => vec![time_s],
        _ => Vec::new(),
    }
}

pub(crate) struct VoltageSource {
    pub name: String,
    pub a: Node,
    pub b: Node,
    pub waveform: model::Waveform,
    /// A step waveform has its value after the step.
    pub stepped: bool,
    pub k: usize,
}

impl Element for VoltageSource {
    fn name(&self) -> &str {
        &self.name
    }
    fn aux_count(&self) -> usize {
        1
    }
    fn set_aux(&mut self, index: usize) {
        self.k = index;
    }
    fn stamp(&self, _alpha: f64, m: &mut dyn Stamp) {
        stamp_branch_current(m, self.a, self.b, self.k, true);
    }
    fn rhs(&self, t: f64, _stage: &Stage, b: &mut [f64]) {
        b[self.k] += waveform(&self.waveform, t, self.stepped);
    }
    fn pre_step(&mut self, t: f64, tol: f64, actions: &mut Vec<(String, SwitchAction)>) {
        if step_due(&self.waveform, self.stepped, t, tol) {
            self.stepped = true;
            actions.push((self.name.clone(), SwitchAction::Step));
        }
    }
    fn action_times(&self) -> Vec<f64> {
        step_times(&self.waveform)
    }
    fn current(&self, _k: usize, _t: f64, _stage: &Stage, x: &[f64]) -> f64 {
        x[self.k]
    }
    fn rhs_phasor(&self, omega: f64, b: &mut [Complex64]) {
        b[self.k] += waveform_phasor(&self.waveform, omega);
    }
    fn current_phasor(&self, _k: usize, _omega: f64, x: &[Complex64]) -> Complex64 {
        x[self.k]
    }
}

pub(crate) struct CurrentSource {
    pub name: String,
    pub a: Node,
    pub b: Node,
    pub waveform: model::Waveform,
    /// A step waveform has its value after the step.
    pub stepped: bool,
}

impl Element for CurrentSource {
    fn name(&self) -> &str {
        &self.name
    }
    fn stamp(&self, _alpha: f64, _m: &mut dyn Stamp) {}
    fn rhs(&self, t: f64, _stage: &Stage, b: &mut [f64]) {
        inject(b, self.a, self.b, waveform(&self.waveform, t, self.stepped));
    }
    fn current(&self, _k: usize, t: f64, _stage: &Stage, _x: &[f64]) -> f64 {
        waveform(&self.waveform, t, self.stepped)
    }
    fn pre_step(&mut self, t: f64, tol: f64, actions: &mut Vec<(String, SwitchAction)>) {
        if step_due(&self.waveform, self.stepped, t, tol) {
            self.stepped = true;
            actions.push((self.name.clone(), SwitchAction::Step));
        }
    }
    fn action_times(&self) -> Vec<f64> {
        step_times(&self.waveform)
    }
    fn rhs_phasor(&self, omega: f64, b: &mut [Complex64]) {
        inject(b, self.a, self.b, waveform_phasor(&self.waveform, omega));
    }
    fn current_phasor(&self, _k: usize, omega: f64, _x: &[Complex64]) -> Complex64 {
        waveform_phasor(&self.waveform, omega)
    }
}

/// The value of a controlled source: a control signal, one time step late.
///
/// The signal is known at grid points only, and the value computed at a
/// grid point is the first that the circuit can use, over the step that
/// follows. Over that step the source moves in a straight line from the
/// value of the grid point before to this one: source(t) = u(t - h), where u
/// is the signal joined by straight lines between grid points. The source
/// is then continuous, every stage of a step sees the value of its own
/// time, and the only error against the signal is the delay of one step.
/// Holding the value over the step instead would add a jump at every grid
/// point, which the first stage of 2S-DIRK (at t + 0.29 h) sees as a delay
/// that is shorter by a fraction of a step, an error of first order.
///
/// Before t = 0 the signal is taken to have had its value at t = 0: the
/// source has that value at t = 0 and over the first step. A run that
/// starts from the steady state knows the source at t = 0 from its phasor
/// instead: over the first step the source moves from that value to the
/// signal at t = 0.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Followed {
    /// Index of the signal.
    pub signal: usize,
    /// Value at the start of the present step.
    start: f64,
    /// Value at its end.
    end: f64,
    /// The value at t = 0 was given by the steady state.
    preset: bool,
    /// Number of times the signal has been read: once at t = 0 and once at
    /// the start of every step after the first.
    reads: u64,
    /// The time step.
    h: f64,
}

impl Followed {
    pub fn new(signal: usize) -> Followed {
        Followed {
            signal,
            start: 0.0,
            end: 0.0,
            preset: false,
            reads: 0,
            h: 1.0,
        }
    }

    pub(crate) fn prepare(&mut self, stages: &Stages) {
        self.h = stages.time_step_s;
    }

    /// The value at time `t`, within the present step. The fraction of the
    /// step is taken from the time and not from the stage: the solution at
    /// t = 0+ and the parts of a step that is split at a switching instant
    /// have stages that count within their own interval.
    #[inline]
    pub(crate) fn at(&self, t: f64) -> f64 {
        let step = self.reads.saturating_sub(1) as f64;
        let fraction = (t - step * self.h) / self.h;
        // The end of the step, within the rounding of its time.
        let fraction = if (fraction - 1.0).abs() < 1e-9 {
            1.0
        } else {
            fraction.clamp(0.0, 1.0)
        };
        self.start + fraction * (self.end - self.start)
    }

    /// The value at t = 0 of the steady state the run starts from.
    pub(crate) fn set(&mut self, value: f64) {
        self.start = value;
        self.end = value;
        self.preset = true;
    }

    /// Takes the signal at the start of a step (at a grid point, once);
    /// true when the values of the step differ from those of the step
    /// before.
    pub(crate) fn follow(&mut self, signals: &[f64]) -> bool {
        let v = signals[self.signal];
        let before = (self.start, self.end);
        self.start = if self.reads > 0 || self.preset {
            self.end
        } else {
            v
        };
        self.end = v;
        self.reads += 1;
        (self.start, self.end) != before
    }
}

#[inline]
fn response(changed: bool) -> SignalResponse {
    if changed {
        SignalResponse::ValueChanged
    } else {
        SignalResponse::Unchanged
    }
}

/// A voltage source whose value is a control signal.
pub(crate) struct ControlledVoltageSource {
    pub name: String,
    pub a: Node,
    pub b: Node,
    pub value: Followed,
    /// Phasor in the steady-state initialization (rms, cosine reference).
    pub phasor: Complex64,
    pub k: usize,
}

impl Element for ControlledVoltageSource {
    fn name(&self) -> &str {
        &self.name
    }
    fn aux_count(&self) -> usize {
        1
    }
    fn set_aux(&mut self, index: usize) {
        self.k = index;
    }
    fn stamp(&self, _alpha: f64, m: &mut dyn Stamp) {
        stamp_branch_current(m, self.a, self.b, self.k, true);
    }
    fn prepare(&mut self, stages: &Stages) -> Result<(), String> {
        self.value.prepare(stages);
        Ok(())
    }
    fn rhs(&self, t: f64, _stage: &Stage, b: &mut [f64]) {
        b[self.k] += self.value.at(t);
    }
    fn current(&self, _k: usize, _t: f64, _stage: &Stage, x: &[f64]) -> f64 {
        x[self.k]
    }
    fn reads_signals(&self) -> bool {
        true
    }
    fn read_signals(&mut self, signals: &[f64]) -> SignalResponse {
        response(self.value.follow(signals))
    }
    fn rhs_phasor(&self, _omega: f64, b: &mut [Complex64]) {
        b[self.k] += self.phasor;
    }
    fn current_phasor(&self, _k: usize, _omega: f64, x: &[Complex64]) -> Complex64 {
        x[self.k]
    }
    fn init_from_phasor(&mut self, _omega: f64, _x: &[Complex64]) {
        self.value.set(instant(self.phasor));
    }
}

/// A current source whose value is a control signal, flowing from `a` to
/// `b` through the source.
pub(crate) struct ControlledCurrentSource {
    pub name: String,
    pub a: Node,
    pub b: Node,
    pub value: Followed,
    /// Phasor in the steady-state initialization (rms, cosine reference).
    pub phasor: Complex64,
}

impl Element for ControlledCurrentSource {
    fn name(&self) -> &str {
        &self.name
    }
    fn stamp(&self, _alpha: f64, _m: &mut dyn Stamp) {}
    fn prepare(&mut self, stages: &Stages) -> Result<(), String> {
        self.value.prepare(stages);
        Ok(())
    }
    fn rhs(&self, t: f64, _stage: &Stage, b: &mut [f64]) {
        inject(b, self.a, self.b, self.value.at(t));
    }
    fn current(&self, _k: usize, t: f64, _stage: &Stage, _x: &[f64]) -> f64 {
        self.value.at(t)
    }
    fn reads_signals(&self) -> bool {
        true
    }
    fn read_signals(&mut self, signals: &[f64]) -> SignalResponse {
        response(self.value.follow(signals))
    }
    fn rhs_phasor(&self, _omega: f64, b: &mut [Complex64]) {
        inject(b, self.a, self.b, self.phasor);
    }
    fn current_phasor(&self, _k: usize, _omega: f64, _x: &[Complex64]) -> Complex64 {
        self.phasor
    }
    fn init_from_phasor(&mut self, _omega: f64, _x: &[Complex64]) {
        self.value.set(instant(self.phasor));
    }
}
