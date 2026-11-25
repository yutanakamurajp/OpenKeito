//! Control blocks.
//!
//! The blocks are evaluated once per grid point, after the circuit has been
//! solved up to that point, in the order of their inputs. Measurements read
//! the circuit solution at that grid point; the outputs act on the circuit
//! over the next time step. The controls therefore reach the circuit one
//! time step later, and no equation couples them within a step.
//!
//! A block with a state integrates it with the trapezoidal rule from the
//! last grid point to the present one, which needs its input at both. In a
//! loop of blocks (allowed when it passes through such a state) the present
//! input depends on the present output: the blocks of the loop are evaluated
//! again and again, each with the latest outputs of the others, until no
//! output changes. The first pass uses the inputs of the last grid point for
//! what is not known yet, which makes it a forward-Euler prediction; the
//! passes that follow converge to the solution of the trapezoidal rule for
//! the whole loop (second order). They converge when the gain around the
//! loop within one step, about time_step_s / 2 times the loop gain in 1/s,
//! is below 1. `evaluate` therefore must not change the state of a block;
//! `commit` does, once the grid point is settled.
//!
//! A block type implements [`Block`] in a module here. To add one, write
//! the block and add one line to the list of `build`.

mod blocks;
mod dynamic;
mod nonlinear;
mod time;
mod transforms;
mod vector;

use openkeito_model as model;

#[cfg(test)]
use blocks::triangle;

/// Largest number of passes over the blocks of a loop at one grid point.
pub const MAX_LOOP_ITERATIONS: u64 = 100;

/// A loop is settled when no output changed by more than this fraction of
/// the largest output of the loop.
const LOOP_TOLERANCE: f64 = 1e-12;

/// What a block is evaluated with.
pub(crate) struct Context<'a> {
    /// Time of the grid point.
    pub t: f64,
    /// The time step; the last grid point was at `t - h`.
    pub h: f64,
    /// True at the first grid point (t = 0): a block with a state gives its
    /// initial state.
    pub start: bool,
    /// The circuit quantities read at `t`.
    pub measured: &'a [f64],
}

/// One control block.
pub(crate) trait Block {
    /// Writes the outputs of the block at `ctx.t` to `out`. `signals` holds
    /// the value of every signal: those the block reads directly are already
    /// at `ctx.t`; in a loop, others may still be those of the last pass or
    /// of the last grid point. A block with a state computes its new state
    /// from the state at the last grid point and keeps it aside: the call is
    /// repeated at the same grid point while a loop settles.
    fn evaluate(&mut self, ctx: &Context, signals: &[f64], out: &mut [f64]);

    /// True for a block that keeps something from one grid point to the
    /// next; only these are told by `commit`.
    fn has_state(&self) -> bool {
        false
    }

    /// The grid point is settled: the state of the last `evaluate` becomes
    /// the state of the block, and the inputs in `signals` its last inputs.
    fn commit(&mut self, _ctx: &Context, _signals: &[f64]) {}

    /// The outputs the block would have at a time `t` within the next time
    /// step, when every measured quantity and every state of a block stays
    /// as at the last grid point. `signals` holds these values of the
    /// inputs; `out` holds the outputs at the last grid point and is
    /// overwritten. The default leaves them, which is right for a block
    /// whose output depends on measurements or states only. A block that is
    /// a known function of time (a carrier, a step) or of its inputs alone
    /// (a comparator, a limiter) computes it, so that the solver can find
    /// the instant within a step at which a gate signal changes
    /// (`simulation.switching` = "interpolated"). The block is not changed.
    fn preview(&self, _t: f64, _signals: &[f64], _out: &mut [f64]) {}
}

/// Largest number of outputs of a block.
const MAX_OUTPUTS: usize = 4;

/// What the blocks are built from.
pub(crate) struct Build<'a> {
    /// Index of a signal by its name.
    pub index: &'a dyn Fn(&str) -> usize,
    /// Index of the first value of this block among the measured quantities
    /// (for a block that measures).
    pub slot: usize,
    /// The time step.
    pub h: f64,
}

impl Build<'_> {
    pub fn signal(&self, name: &str) -> usize {
        (self.index)(name)
    }

    pub fn signals(&self, names: &[String]) -> Vec<usize> {
        names.iter().map(|n| self.signal(n)).collect()
    }

    pub fn three(&self, names: &[String; 3]) -> [usize; 3] {
        [
            self.signal(&names[0]),
            self.signal(&names[1]),
            self.signal(&names[2]),
        ]
    }
}

/// A limit of a control block: a constant number or a signal index.
pub(super) enum Bound {
    Value(f64),
    Signal(usize),
}

impl Bound {
    pub fn from_limit(limit: Option<&model::LimitValue>, b: &Build) -> Option<Self> {
        limit.map(|l| match l {
            model::LimitValue::Value(v) => Bound::Value(*v),
            model::LimitValue::Signal(s) => Bound::Signal(b.signal(s)),
        })
    }

    #[inline]
    pub fn get(&self, signals: &[f64]) -> f64 {
        match *self {
            Bound::Value(v) => v,
            Bound::Signal(i) => signals[i],
        }
    }
}

/// Clamps `y` into `[min, max]`, returning `f64::NAN` if `y` or any present
/// bound is non-finite so `Controls::evaluate` stops with `ControlError::NotFinite`.
/// When signal limits transiently invert (`min > max`), `min` overrides `max`.
#[inline]
pub(super) fn apply_limits(mut y: f64, min: Option<f64>, max: Option<f64>) -> f64 {
    if !y.is_finite() || min.is_some_and(|m| !m.is_finite()) || max.is_some_and(|m| !m.is_finite())
    {
        return f64::NAN;
    }
    if let Some(m) = max {
        y = y.min(m);
    }
    if let Some(m) = min {
        y = y.max(m);
    }
    y
}

/// Builds the block of a validated control.
fn build(c: &model::Control, b: &Build) -> Box<dyn Block> {
    // One line per block type.
    match c {
        model::Control::Constant(c) => Box::new(blocks::Constant::new(c)),
        model::Control::Gain(c) => Box::new(blocks::Gain::new(c, b)),
        model::Control::Sum(c) => Box::new(blocks::Sum::new(c, b)),
        model::Control::TriangleWave(c) => Box::new(blocks::Triangle::new(c)),
        model::Control::Comparator(c) => Box::new(blocks::Comparator::new(c, b)),
        model::Control::Pi(c) => Box::new(blocks::Pi::new(c, b)),
        model::Control::Measurement(_) => Box::new(blocks::Measurement::new(b.slot)),
        model::Control::TransferFunction(c) => Box::new(dynamic::TransferFunction::new(c, b)),
        model::Control::Integrator(c) => Box::new(dynamic::Integrator::new(c, b)),
        model::Control::Lag(c) => Box::new(dynamic::Lag::new(c, b)),
        model::Control::LeadLag(c) => Box::new(dynamic::LeadLag::new(c, b)),
        model::Control::Delay(c) => Box::new(dynamic::Delay::new(c, b)),
        model::Control::Limiter(c) => Box::new(nonlinear::Limiter::new(c, b)),
        model::Control::RateLimiter(c) => Box::new(nonlinear::RateLimiter::new(c, b)),
        model::Control::DeadBand(c) => Box::new(nonlinear::DeadBand::new(c, b)),
        model::Control::Product(c) => Box::new(nonlinear::Product::new(c, b)),
        model::Control::Quotient(c) => Box::new(nonlinear::Quotient::new(c, b)),
        model::Control::Function(c) => Box::new(nonlinear::Function::new(c, b)),
        model::Control::Expression(c) => Box::new(nonlinear::Expression::new(c, b)),
        model::Control::Selector(c) => Box::new(nonlinear::Selector::new(c, b)),
        model::Control::PiecewiseLinear(c) => Box::new(nonlinear::PiecewiseLinear::new(c, b)),
        model::Control::InitialValue(c) => Box::new(nonlinear::InitialValue::new(c, b)),
        model::Control::Step(c) => Box::new(time::Step::new(c)),
        model::Control::Ramp(c) => Box::new(time::Ramp::new(c)),
        model::Control::SineWave(c) => Box::new(time::SineWave::new(c)),
        model::Control::TimeSeries(c) => Box::new(time::TimeSeries::new(c)),
        model::Control::AbcToDq0(c) => Box::new(transforms::AbcToDq0::new(c, b)),
        model::Control::Dq0ToAbc(c) => Box::new(transforms::Dq0ToAbc::new(c, b)),
        model::Control::Pll(c) => Box::new(transforms::Pll::new(c, b)),
        model::Control::ThreePhaseMeasurement(_) => {
            Box::new(blocks::ThreePhaseMeasurement::new(b.slot))
        }
        model::Control::DqLimiter(c) => Box::new(vector::DqLimiter::new(c, b)),
        model::Control::PiDq(c) => Box::new(vector::PiDq::new(c, b)),
    }
}

/// Why the controls could not be evaluated at a grid point.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ControlError {
    /// The outputs of the blocks of a loop still changed after
    /// `MAX_LOOP_ITERATIONS` passes.
    LoopNotConverged { blocks: Vec<String> },
    /// A signal is NaN or infinite.
    NotFinite {
        signal: String,
        block_type: &'static str,
    },
}

pub(crate) struct Controls {
    blocks: Vec<Box<dyn Block>>,
    /// Name and type of every block, for error messages.
    labels: Vec<(String, &'static str)>,
    /// First signal and number of signals of every block.
    ranges: Vec<(usize, usize)>,
    plan: Vec<model::EvaluationGroup>,
    /// The blocks with a state.
    stateful: Vec<usize>,
    names: Vec<String>,
    h: f64,
    started: bool,
    /// Outputs of the block being evaluated.
    scratch: Vec<f64>,
    /// Present value of every signal, indexed like `model::signal_names`.
    pub values: Vec<f64>,
    /// Passes over the blocks of loops beyond the first, in the whole run.
    pub loop_iterations: u64,
}

impl Controls {
    /// Builds the blocks of a validated model for the time step `h`.
    /// `measurement` maps the index of a measuring block to the index of its
    /// first value in the slice that `evaluate` receives.
    pub fn new(
        controls: &[model::Control],
        h: f64,
        measurement: impl Fn(usize) -> usize,
    ) -> Controls {
        let names = model::signal_names(controls);
        let index = |name: &str| {
            names
                .iter()
                .position(|n| n == name)
                .expect("validated signal name")
        };
        let mut ranges = Vec::with_capacity(controls.len());
        let mut first = 0;
        for c in controls {
            let count = c.signal_names().len();
            ranges.push((first, count));
            first += count;
        }
        let blocks: Vec<Box<dyn Block>> = controls
            .iter()
            .enumerate()
            .map(|(i, c)| {
                build(
                    c,
                    &Build {
                        index: &index,
                        slot: measurement(i),
                        h,
                    },
                )
            })
            .collect();
        let plan = model::evaluation_plan(controls, h).expect("validated: no algebraic loop");
        let stateful = (0..blocks.len())
            .filter(|&i| blocks[i].has_state())
            .collect();
        let widest = ranges.iter().map(|r| r.1).max().unwrap_or(0);
        assert!(widest <= MAX_OUTPUTS, "a block has {widest} outputs");
        Controls {
            labels: controls
                .iter()
                .map(|c| (c.name().to_string(), c.type_name()))
                .collect(),
            blocks,
            ranges,
            plan,
            stateful,
            values: vec![0.0; names.len()],
            names,
            h,
            started: false,
            scratch: vec![0.0; widest],
            loop_iterations: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// Evaluates every block at time `t`. Call once per grid point, with
    /// `t` increasing by the time step; the first call gives the initial
    /// states.
    pub fn evaluate(&mut self, t: f64, measured: &[f64]) -> Result<(), ControlError> {
        self.evaluate_inner(t, measured, &[], true)
    }

    /// Evaluates every block at time `t` with signal `overrides` applied right
    /// after each block writes its outputs, then commits stateful blocks.
    pub fn evaluate_with_overrides(
        &mut self,
        t: f64,
        measured: &[f64],
        overrides: &[(usize, f64)],
    ) -> Result<(), ControlError> {
        self.evaluate_inner(t, measured, overrides, true)
    }

    /// Evaluates every block at time `t` with signal `overrides` without
    /// committing stateful blocks or marking `started`. Used to expose fresh
    /// measurements and upstream block outputs to a controller before overrides
    /// are chosen and committed.
    pub fn evaluate_preview_state(
        &mut self,
        t: f64,
        measured: &[f64],
        overrides: &[(usize, f64)],
    ) -> Result<(), ControlError> {
        self.evaluate_inner(t, measured, overrides, false)
    }

    #[inline(always)]
    fn evaluate_inner(
        &mut self,
        t: f64,
        measured: &[f64],
        overrides: &[(usize, f64)],
        commit: bool,
    ) -> Result<(), ControlError> {
        let ctx = Context {
            t,
            h: self.h,
            start: !self.started,
            measured,
        };
        for group in &self.plan {
            if !group.iterate {
                let i = group.blocks[0];
                let (first, n) = self.ranges[i];
                self.blocks[i].evaluate(&ctx, &self.values, &mut self.scratch[..n]);
                self.values[first..first + n].copy_from_slice(&self.scratch[..n]);
                if !overrides.is_empty() {
                    for &(sig_idx, val) in overrides {
                        if (first..first + n).contains(&sig_idx) {
                            self.values[sig_idx] = val;
                        }
                    }
                }
                continue;
            }
            let mut passes = 0;
            loop {
                let (mut change, mut scale) = (0.0_f64, 0.0_f64);
                let mut non_finite = false;
                for &i in &group.blocks {
                    let (first, n) = self.ranges[i];
                    self.blocks[i].evaluate(&ctx, &self.values, &mut self.scratch[..n]);
                    if !overrides.is_empty() {
                        for &(sig_idx, val) in overrides {
                            if (first..first + n).contains(&sig_idx) {
                                self.scratch[sig_idx - first] = val;
                            }
                        }
                    }
                    for (old, new) in self.values[first..first + n].iter_mut().zip(&self.scratch) {
                        // A value that is not finite ends the loop; it is
                        // reported below (f64::max discards NaN, so check it
                        // directly).
                        if !new.is_finite() {
                            non_finite = true;
                        }
                        change = change.max((new - *old).abs());
                        scale = scale.max(new.abs());
                        *old = *new;
                    }
                }
                passes += 1;
                if non_finite || change <= LOOP_TOLERANCE * scale {
                    break;
                }
                if passes >= MAX_LOOP_ITERATIONS {
                    return Err(ControlError::LoopNotConverged {
                        blocks: group
                            .blocks
                            .iter()
                            .map(|&i| self.labels[i].0.clone())
                            .collect(),
                    });
                }
            }
            if commit {
                self.loop_iterations += passes - 1;
            }
        }
        if let Some(k) = self.values.iter().position(|v| !v.is_finite()) {
            let block = self
                .ranges
                .iter()
                .position(|&(first, n)| (first..first + n).contains(&k))
                .expect("every signal belongs to a block");
            return Err(ControlError::NotFinite {
                signal: self.names[k].clone(),
                block_type: self.labels[block].1,
            });
        }
        if commit {
            for &i in &self.stateful {
                self.blocks[i].commit(&ctx, &self.values);
            }
            self.started = true;
        }
        Ok(())
    }

    /// Writes to `out` the signals as they would be at a time `t` within the
    /// step after the last `evaluate`, with the measured quantities and the
    /// states of the blocks held (see `Block::preview`). Nothing is changed.
    /// The blocks of a loop are previewed once, in their order: those with a
    /// state hold their outputs, and the others follow from them.
    #[cfg(test)]
    pub fn preview(&self, t: f64, out: &mut [f64]) {
        self.preview_with_overrides(t, out, &[])
    }

    /// Like `preview`, keeping any `overrides` held over the step.
    pub fn preview_with_overrides(&self, t: f64, out: &mut [f64], overrides: &[(usize, f64)]) {
        let mut outputs = [0.0; MAX_OUTPUTS];
        for group in &self.plan {
            for &i in &group.blocks {
                let (first, n) = self.ranges[i];
                outputs[..n].copy_from_slice(&self.values[first..first + n]);
                self.blocks[i].preview(t, out, &mut outputs[..n]);
                out[first..first + n].copy_from_slice(&outputs[..n]);
                if !overrides.is_empty() {
                    for &(sig_idx, val) in overrides {
                        if (first..first + n).contains(&sig_idx) {
                            out[sig_idx] = val;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocks(json: serde_json::Value) -> Vec<model::Control> {
        serde_json::from_value(json).unwrap()
    }

    /// The controls for a time step `h`, with every measurement in slot 0.
    fn controls(h: f64, json: serde_json::Value) -> Controls {
        Controls::new(&blocks(json), h, |_| 0)
    }

    #[test]
    fn triangle_rises_from_the_minimum_and_is_symmetric() {
        assert_eq!(triangle(0.0), 0.0);
        assert!((triangle(0.25) - 0.5).abs() < 1e-15);
        assert!((triangle(0.5) - 1.0).abs() < 1e-15);
        assert!((triangle(0.75) - 0.5).abs() < 1e-15);
        assert!((triangle(1.1) - triangle(0.9)).abs() < 1e-12);
        assert!((triangle(-0.1) - triangle(0.1)).abs() < 1e-12);
    }

    #[test]
    fn pwm_chain_is_evaluated_in_the_order_of_the_inputs() {
        // Listed in reverse order of evaluation on purpose.
        let mut k = controls(
            1e-4,
            serde_json::json!([
                {"type": "comparator", "name": "pwm", "inputs": ["ref", "carrier"]},
                {"type": "triangle_wave", "name": "carrier", "frequency_hz": 1000.0, "minimum": -1.0, "maximum": 1.0},
                {"type": "sum", "name": "ref", "inputs": ["half", "m"], "signs": [1.0, -1.0]},
                {"type": "gain", "name": "half", "input": "m", "gain": 3.0},
                {"type": "measurement", "name": "m", "quantity": {"type": "node_voltage", "node": "n1"}},
            ]),
        );
        // ref = 3m - m = 2m = 0.4. Carrier at t = 0.1 ms is -1 + 2*0.2 = -0.6.
        k.evaluate(1e-4, &[0.2]).unwrap();
        assert!((k.values[2] - 0.4).abs() < 1e-15);
        assert!((k.values[1] + 0.6).abs() < 1e-12);
        assert_eq!(k.values[0], 1.0);
        // At t = 0.45 ms the carrier is 1 - 2*0.1*2 = 0.8 > 0.4.
        k.evaluate(4.5e-4, &[0.2]).unwrap();
        assert_eq!(k.values[0], 0.0);
    }

    #[test]
    fn preview_moves_the_carrier_and_holds_the_measurement() {
        let mut k = controls(
            1e-4,
            serde_json::json!([
                {"type": "measurement", "name": "m", "quantity": {"type": "node_voltage", "node": "n1"}},
                {"type": "gain", "name": "ref", "input": "m", "gain": 2.0},
                {"type": "triangle_wave", "name": "carrier", "frequency_hz": 1000.0},
                {"type": "sum", "name": "d", "inputs": ["ref", "carrier"], "signs": [1.0, -1.0]},
                {"type": "comparator", "name": "pwm", "inputs": ["ref", "carrier"]},
            ]),
        );
        // ref = 0.4; the carrier at 0.1 ms is 0.2.
        k.evaluate(1e-4, &[0.2]).unwrap();
        assert_eq!(k.values[4], 1.0);
        let mut ahead = vec![0.0; 5];
        // At 0.3 ms the carrier is 0.6: the comparator has changed. The
        // measurement and what follows from it alone are held.
        k.preview(3e-4, &mut ahead);
        assert_eq!(ahead[0], 0.2);
        assert!((ahead[1] - 0.4).abs() < 1e-15);
        assert!((ahead[2] - 0.6).abs() < 1e-12);
        assert!((ahead[3] + 0.2).abs() < 1e-12);
        assert_eq!(ahead[4], 0.0);
        // The blocks and their values are as before.
        assert_eq!(k.values[4], 1.0);
        assert!((k.values[2] - 0.2).abs() < 1e-12);
    }

    #[test]
    fn preview_follows_time_and_inputs_through_the_blocks_without_a_state() {
        // A ramp through a limiter and a product, an integrator of the ramp,
        // and the frame of a rotating set of phases.
        let h = 1e-3;
        let mut k = controls(
            h,
            serde_json::json!([
                {"type": "ramp", "name": "u", "slope_per_s": 100.0},
                {"type": "limiter", "name": "limited", "input": "u", "maximum": 0.15},
                {"type": "product", "name": "square", "inputs": ["u", "limited"]},
                {"type": "integrator", "name": "x", "input": "u"},
                {"type": "sum", "name": "y", "inputs": ["x", "u"], "signs": [1.0, 1.0]},
                {"type": "step", "name": "s", "time_s": 1.5e-3, "after": 2.0},
                {"type": "constant", "name": "zero", "value": 0.0},
                {"type": "dq0_to_abc", "name": "p", "d": "u", "q": "zero", "angle": "zero"},
            ]),
        );
        k.evaluate(0.0, &[]).unwrap();
        k.evaluate(h, &[]).unwrap();
        // At 1 ms: u = 0.1, x = 5e-5.
        let x = k.values[3];
        assert!((x - 5e-5).abs() < 1e-18);
        let mut ahead = vec![0.0; k.values.len()];
        k.preview(1.8e-3, &mut ahead);
        assert!((ahead[0] - 0.18).abs() < 1e-15);
        assert_eq!(ahead[1], 0.15);
        assert!((ahead[2] - 0.18 * 0.15).abs() < 1e-15);
        // The state is held; what reads it follows the other input.
        assert_eq!(ahead[3], x);
        assert!((ahead[4] - (x + 0.18)).abs() < 1e-15);
        assert_eq!(ahead[5], 2.0);
        // Phase a of d = u at the angle 0.
        assert!((ahead[7] - 0.18).abs() < 1e-15);
        assert!((ahead[8] + 0.09).abs() < 1e-15);
        // Nothing has changed.
        assert_eq!(k.values[5], 0.0);
        assert!((k.values[0] - 0.1).abs() < 1e-15);
    }

    #[test]
    fn preview_evaluates_piecewise_linear_and_signal_limits() {
        let h = 1e-3;
        let mut k = controls(
            h,
            serde_json::json!([
                {"type": "ramp", "name": "u", "slope_per_s": 1000.0},
                {"type": "piecewise_linear", "name": "lut", "input": "u", "x": [0.0, 1.0, 2.0], "y": [0.0, 2.0, 1.0]},
                {"type": "ramp", "name": "hi", "initial": 1.5, "slope_per_s": -200.0},
                {"type": "limiter", "name": "lim", "input": "lut", "maximum": "hi"},
            ]),
        );
        k.evaluate(0.0, &[]).unwrap();
        let mut ahead = vec![0.0; k.values.len()];
        // At t = 1.5 ms: u = 1.5, lut = 1.5, hi = 1.2, lim = 1.2.
        k.preview(1.5e-3, &mut ahead);
        assert!((ahead[0] - 1.5).abs() < 1e-15);
        assert!((ahead[1] - 1.5).abs() < 1e-15);
        assert!((ahead[2] - 1.2).abs() < 1e-15);
        assert!((ahead[3] - 1.2).abs() < 1e-15);
    }

    #[test]
    fn pi_integrates_with_the_trapezoidal_rule_and_holds_at_its_limits() {
        let h = 1e-3;
        let mut k = controls(
            h,
            serde_json::json!([
                {"type": "measurement", "name": "e", "quantity": {"type": "node_voltage", "node": "n1"}},
                {"type": "pi", "name": "u", "input": "e", "proportional_gain": 2.0,
                 "integral_gain_per_s": 100.0, "initial_integral": 0.5, "output_max": 3.0},
            ]),
        );
        // A ramp e = 10 t is integrated exactly by the trapezoidal rule:
        // integral = 0.5 + 100 * 5 t^2.
        for n in 0..=10 {
            let t = n as f64 * h;
            k.evaluate(t, &[10.0 * t]).unwrap();
            let exact = 2.0 * 10.0 * t + 0.5 + 500.0 * t * t;
            assert!((k.values[1] - exact.min(3.0)).abs() < 1e-12, "t = {t}");
        }
        // At t = 0.01 the unclamped output is 0.2 + 0.55 = 0.75. A large
        // positive error drives it to the limit and holds the integral there.
        for n in 11..200 {
            k.evaluate(n as f64 * h, &[10.0]).unwrap();
        }
        assert_eq!(k.values[1], 3.0);
        // The integral was held at 0.55 during the saturation, so when the
        // error reverses the output leaves the limit at once (no wind-up):
        // -0.2 + 0.55 + 100 * 0.5 * (10 - 0.1) * 1e-3 = 0.845.
        k.evaluate(0.2, &[-0.1]).unwrap();
        assert!((k.values[1] - 0.845).abs() < 1e-12, "{}", k.values[1]);
    }

    #[test]
    fn a_loop_through_an_integrator_settles_on_the_trapezoidal_rule() {
        // x' = r - x with r = 1 and x(0) = 0. The trapezoidal rule gives
        // x_n+1 = (x_n (1 - h/2) + h) / (1 + h/2), whatever the order in
        // which the blocks are listed.
        let h = 0.1;
        let mut k = controls(
            h,
            serde_json::json!([
                {"type": "sum", "name": "e", "inputs": ["r", "x"], "signs": [1.0, -1.0]},
                {"type": "integrator", "name": "x", "input": "e"},
                {"type": "constant", "name": "r", "value": 1.0},
            ]),
        );
        let mut x = 0.0;
        for n in 0..=20 {
            k.evaluate(n as f64 * h, &[]).unwrap();
            if n > 0 {
                x = (x * (1.0 - h / 2.0) + h) / (1.0 + h / 2.0);
            }
            assert!((k.values[1] - x).abs() < 1e-12, "step {n}");
            assert!((k.values[0] - (1.0 - x)).abs() < 1e-12, "step {n}");
        }
        // Each pass multiplies the error by h/2 = 0.05: about 10 passes per
        // grid point for 1e-12.
        assert!(k.loop_iterations > 20 && k.loop_iterations < 400);
    }

    #[test]
    fn a_loop_that_does_not_settle_is_an_error_that_names_its_blocks() {
        // With h/2 * gain = 5 each pass multiplies the difference by 5.
        let mut k = controls(
            0.1,
            serde_json::json!([
                {"type": "gain", "name": "e", "input": "x", "gain": -100.0},
                {"type": "integrator", "name": "x", "input": "e", "initial_value": 1.0},
            ]),
        );
        k.evaluate(0.0, &[]).unwrap();
        assert_eq!(
            k.evaluate(0.1, &[]),
            Err(ControlError::LoopNotConverged {
                blocks: vec!["x".to_string(), "e".to_string()]
            })
        );
    }

    #[test]
    fn a_signal_that_is_not_finite_is_an_error_that_names_it() {
        let mut k = controls(
            0.1,
            serde_json::json!([
                {"type": "constant", "name": "zero", "value": 0.0},
                {"type": "constant", "name": "one", "value": 1.0},
                {"type": "quotient", "name": "q", "inputs": ["one", "zero"]},
            ]),
        );
        assert_eq!(
            k.evaluate(0.0, &[]),
            Err(ControlError::NotFinite {
                signal: "q".to_string(),
                block_type: "quotient"
            })
        );
    }

    #[test]
    fn preview_evaluates_expression_and_drives_comparator() {
        let h = 1e-3;
        let mut k = controls(
            h,
            serde_json::json!([
                {"type": "ramp", "name": "u", "slope_per_s": 1000.0},
                {"type": "expression", "name": "sq", "inputs": ["u"], "expression": "2*u^2 - 0.5"},
                {"type": "constant", "name": "zero", "value": 0.0},
                {"type": "comparator", "name": "pwm", "inputs": ["sq", "zero"]},
            ]),
        );
        k.evaluate(0.0, &[]).unwrap();
        assert_eq!(k.values[1], -0.5);
        assert_eq!(k.values[3], 0.0);
        let mut ahead = vec![0.0; k.values.len()];
        // At t = 1 ms: u = 1.0, sq = 1.5, pwm = 1.0.
        k.preview(1e-3, &mut ahead);
        assert!((ahead[0] - 1.0).abs() < 1e-15);
        assert!((ahead[1] - 1.5).abs() < 1e-15);
        assert_eq!(ahead[3], 1.0);
        // State at t = 0 is untouched.
        assert_eq!(k.values[1], -0.5);
        assert_eq!(k.values[3], 0.0);
    }

    #[test]
    fn expression_handles_more_than_32_inputs() {
        let mut list: Vec<serde_json::Value> = (0..35)
            .map(|i| serde_json::json!({"type": "constant", "name": format!("c{i}"), "value": (i + 1) as f64}))
            .collect();
        let inputs: Vec<String> = (0..35).map(|i| format!("c{i}")).collect();
        let expr = inputs.join(" + ");
        list.push(serde_json::json!({
            "type": "expression",
            "name": "total",
            "inputs": inputs,
            "expression": expr,
        }));
        let mut k = controls(1e-3, serde_json::Value::Array(list));
        k.evaluate(0.0, &[]).unwrap();
        let expected: f64 = (1..=35).sum::<usize>() as f64;
        assert_eq!(k.values[35], expected);
    }
}
