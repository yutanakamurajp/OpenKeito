//! The first block types: constant, gain, sum, triangular carrier,
//! comparator, PI controller and the measurements.

use openkeito_model as model;

use super::{apply_limits, Block, Bound, Build, Context};

pub(super) struct Constant(f64);

impl Constant {
    pub fn new(c: &model::Constant) -> Self {
        Constant(c.value)
    }
}

impl Block for Constant {
    fn evaluate(&mut self, _ctx: &Context, _signals: &[f64], out: &mut [f64]) {
        out[0] = self.0;
    }
}

pub(super) struct Gain {
    input: usize,
    gain: f64,
}

impl Gain {
    pub fn new(g: &model::Gain, b: &Build) -> Self {
        Gain {
            input: b.signal(&g.input),
            gain: g.gain,
        }
    }
}

impl Block for Gain {
    fn evaluate(&mut self, _ctx: &Context, signals: &[f64], out: &mut [f64]) {
        out[0] = self.gain * signals[self.input];
    }
    fn preview(&self, _t: f64, signals: &[f64], out: &mut [f64]) {
        out[0] = self.gain * signals[self.input];
    }
}

/// Sum of sign * input.
pub(super) enum Sum {
    Inline { terms: [(usize, f64); 4], len: u8 },
    Many(Vec<(usize, f64)>),
}

impl Sum {
    pub fn new(s: &model::Sum, b: &Build) -> Self {
        if s.inputs.len() <= 4 {
            let mut terms = [(0, 0.0); 4];
            for (slot, (n, sign)) in terms.iter_mut().zip(s.inputs.iter().zip(&s.signs)) {
                *slot = (b.signal(n), *sign);
            }
            Sum::Inline {
                terms,
                len: s.inputs.len() as u8,
            }
        } else {
            Sum::Many(
                s.inputs
                    .iter()
                    .zip(&s.signs)
                    .map(|(n, sign)| (b.signal(n), *sign))
                    .collect(),
            )
        }
    }

    #[inline]
    fn terms(&self) -> &[(usize, f64)] {
        match self {
            Sum::Inline { terms, len } => &terms[..*len as usize],
            Sum::Many(v) => v,
        }
    }
}

impl Block for Sum {
    fn evaluate(&mut self, _ctx: &Context, signals: &[f64], out: &mut [f64]) {
        out[0] = self.terms().iter().map(|(k, s)| s * signals[*k]).sum();
    }
    fn preview(&self, _t: f64, signals: &[f64], out: &mut [f64]) {
        out[0] = self.terms().iter().map(|(k, s)| s * signals[*k]).sum();
    }
}

/// Value of a symmetric triangular carrier between 0 and 1 at `cycles`
/// periods after its minimum.
pub(crate) fn triangle(cycles: f64) -> f64 {
    let f = cycles - cycles.floor();
    if f < 0.5 {
        2.0 * f
    } else {
        2.0 - 2.0 * f
    }
}

pub(super) struct Triangle {
    frequency_hz: f64,
    minimum: f64,
    maximum: f64,
    /// Phase advance as a fraction of a period.
    phase: f64,
}

impl Triangle {
    pub fn new(w: &model::TriangleWave) -> Self {
        Triangle {
            frequency_hz: w.frequency_hz,
            minimum: w.minimum,
            maximum: w.maximum,
            phase: w.phase_deg / 360.0,
        }
    }
}

impl Block for Triangle {
    fn evaluate(&mut self, ctx: &Context, _signals: &[f64], out: &mut [f64]) {
        out[0] = self.minimum
            + (self.maximum - self.minimum) * triangle(self.frequency_hz * ctx.t + self.phase);
    }
    fn preview(&self, t: f64, _signals: &[f64], out: &mut [f64]) {
        out[0] = self.minimum
            + (self.maximum - self.minimum) * triangle(self.frequency_hz * t + self.phase);
    }
    fn has_vertices(&self) -> bool {
        self.frequency_hz > 0.0 && self.maximum != self.minimum
    }
    fn vertices(&self, ta: f64, tb: f64, out: &mut Vec<f64>) {
        // The minimum and the maximum at every half period: f t + phase =
        // k / 2.
        let f = self.frequency_hz;
        let first = (2.0 * (f * ta + self.phase)).floor() as i64;
        let last = (2.0 * (f * tb + self.phase)).ceil() as i64;
        for k in first..=last {
            let t = (0.5 * k as f64 - self.phase) / f;
            if t > ta && t < tb {
                out.push(t);
            }
        }
    }
}

/// 1 when the first input is above the second, otherwise 0.
pub(super) struct Comparator(usize, usize);

impl Comparator {
    pub fn new(c: &model::Comparator, b: &Build) -> Self {
        Comparator(b.signal(&c.inputs[0]), b.signal(&c.inputs[1]))
    }
}

impl Block for Comparator {
    fn evaluate(&mut self, _ctx: &Context, signals: &[f64], out: &mut [f64]) {
        out[0] = if signals[self.0] > signals[self.1] {
            1.0
        } else {
            0.0
        };
    }
    fn preview(&self, _t: f64, signals: &[f64], out: &mut [f64]) {
        out[0] = if signals[self.0] > signals[self.1] {
            1.0
        } else {
            0.0
        };
    }
}

pub(super) struct Pi {
    input: usize,
    kp: f64,
    ki: f64,
    min: Option<Bound>,
    max: Option<Bound>,
    anti_windup: model::PiAntiWindup,
    integral: f64,
    /// Signal whose value at t = 0 is the integral at t = 0.
    initial: Option<usize>,
    /// Input and time of the last grid point.
    previous: Option<(f64, f64)>,
    /// The integral at the grid point being evaluated.
    pending: f64,
}

impl Pi {
    pub fn new(p: &model::Pi, b: &Build) -> Self {
        let (integral, initial) = match &p.initial_integral {
            model::StartValue::Value(v) => (*v, None),
            model::StartValue::Signal(s) => (0.0, Some(b.signal(s))),
        };
        Pi {
            input: b.signal(&p.input),
            kp: p.proportional_gain,
            ki: p.integral_gain_per_s,
            min: Bound::from_limit(p.output_min.as_ref(), b),
            max: Bound::from_limit(p.output_max.as_ref(), b),
            anti_windup: p.anti_windup,
            integral,
            initial,
            previous: None,
            pending: integral,
        }
    }
}

impl Block for Pi {
    fn evaluate(&mut self, ctx: &Context, signals: &[f64], out: &mut [f64]) {
        let e = signals[self.input];
        let min = self.min.as_ref().map(|b| b.get(signals));
        let max = self.max.as_ref().map(|b| b.get(signals));
        let mut integral = if ctx.start {
            let init = self.initial.map_or(self.integral, |s| signals[s]);
            apply_limits(init, min, max)
        } else {
            self.integral
        };
        if let Some((e_prev, t_prev)) = self.previous {
            // Trapezoidal rule. Either conditional integration (hold the
            // integral while the output is beyond a limit and the step would
            // drive it further out) or IEEE 421.5 non-windup state limiting.
            let delta = self.ki * 0.5 * (e + e_prev) * (ctx.t - t_prev);
            match self.anti_windup {
                model::PiAntiWindup::Conditional => {
                    let unclamped = self.kp * e + self.integral + delta;
                    let held = (max.is_some_and(|m| unclamped > m) && delta > 0.0)
                        || (min.is_some_and(|m| unclamped < m) && delta < 0.0);
                    if !held {
                        integral += delta;
                    }
                }
                model::PiAntiWindup::State => {
                    integral = apply_limits(self.integral + delta, min, max);
                }
            }
        }
        self.pending = integral;
        out[0] = apply_limits(self.kp * e + integral, min, max);
    }
    fn has_state(&self) -> bool {
        true
    }
    fn commit(&mut self, ctx: &Context, signals: &[f64]) {
        self.integral = self.pending;
        self.previous = Some((signals[self.input], ctx.t));
    }
}

/// A circuit quantity: the index into the measured values handed to
/// `evaluate`.
pub(super) struct Measurement(usize);

impl Measurement {
    pub fn new(slot: usize) -> Self {
        Measurement(slot)
    }
}

impl Block for Measurement {
    fn evaluate(&mut self, ctx: &Context, _signals: &[f64], out: &mut [f64]) {
        out[0] = ctx.measured[self.0];
    }
}

/// Three phase quantities: the index of the first among the measured values.
pub(super) struct ThreePhaseMeasurement(usize);

impl ThreePhaseMeasurement {
    pub fn new(slot: usize) -> Self {
        ThreePhaseMeasurement(slot)
    }
}

impl Block for ThreePhaseMeasurement {
    fn evaluate(&mut self, ctx: &Context, _signals: &[f64], out: &mut [f64]) {
        out.copy_from_slice(&ctx.measured[self.0..self.0 + 3]);
    }
}
