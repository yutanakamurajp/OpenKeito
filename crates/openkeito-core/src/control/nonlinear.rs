//! The blocks that are not linear: limiter, rate limiter, dead band,
//! product, quotient, function, selector and the value at t = 0.

use openkeito_model as model;

use super::{apply_limits, Block, Bound, Build, Context};

pub(super) struct Limiter {
    input: usize,
    min: Option<Bound>,
    max: Option<Bound>,
}

impl Limiter {
    pub fn new(l: &model::Limiter, b: &Build) -> Self {
        Limiter {
            input: b.signal(&l.input),
            min: Bound::from_limit(l.minimum.as_ref(), b),
            max: Bound::from_limit(l.maximum.as_ref(), b),
        }
    }
}

impl Limiter {
    /// The outputs for the inputs in `signals`.
    fn write(&self, signals: &[f64], out: &mut [f64]) {
        let min = self.min.as_ref().map(|b| b.get(signals));
        let max = self.max.as_ref().map(|b| b.get(signals));
        out[0] = apply_limits(signals[self.input], min, max);
    }
}

impl Block for Limiter {
    fn evaluate(&mut self, _ctx: &Context, signals: &[f64], out: &mut [f64]) {
        self.write(signals, out);
    }
    fn preview(&self, _t: f64, signals: &[f64], out: &mut [f64]) {
        self.write(signals, out);
    }
}

/// The output moves towards the input by at most `rise` up and `fall` down
/// per time step.
pub(super) struct RateLimiter {
    input: usize,
    rise: f64,
    fall: f64,
    initial: Option<f64>,
    y: f64,
    next: f64,
}

impl RateLimiter {
    pub fn new(l: &model::RateLimiter, b: &Build) -> Self {
        RateLimiter {
            input: b.signal(&l.input),
            rise: l.rising_rate_per_s * b.h,
            fall: l.falling_rate_per_s * b.h,
            initial: l.initial_output,
            y: 0.0,
            next: 0.0,
        }
    }
}

impl Block for RateLimiter {
    fn evaluate(&mut self, ctx: &Context, signals: &[f64], out: &mut [f64]) {
        let u = signals[self.input];
        self.next = if ctx.start {
            self.initial.unwrap_or(u)
        } else {
            self.y + (u - self.y).clamp(-self.fall, self.rise)
        };
        out[0] = self.next;
    }
    fn has_state(&self) -> bool {
        true
    }
    fn commit(&mut self, _ctx: &Context, _signals: &[f64]) {
        self.y = self.next;
    }
}

pub(super) struct DeadBand {
    input: usize,
    lower: f64,
    upper: f64,
}

impl DeadBand {
    pub fn new(d: &model::DeadBand, b: &Build) -> Self {
        DeadBand {
            input: b.signal(&d.input),
            lower: d.lower,
            upper: d.upper,
        }
    }
}

impl DeadBand {
    /// The outputs for the inputs in `signals`.
    fn write(&self, signals: &[f64], out: &mut [f64]) {
        let u = signals[self.input];
        out[0] = if u > self.upper {
            u - self.upper
        } else if u < self.lower {
            u - self.lower
        } else {
            0.0
        };
    }
}

impl Block for DeadBand {
    fn evaluate(&mut self, _ctx: &Context, signals: &[f64], out: &mut [f64]) {
        self.write(signals, out);
    }
    fn preview(&self, _t: f64, signals: &[f64], out: &mut [f64]) {
        self.write(signals, out);
    }
}

pub(super) struct Product(Vec<usize>);

impl Product {
    pub fn new(p: &model::Product, b: &Build) -> Self {
        Product(b.signals(&p.inputs))
    }
}

impl Product {
    /// The outputs for the inputs in `signals`.
    fn write(&self, signals: &[f64], out: &mut [f64]) {
        out[0] = self.0.iter().map(|&k| signals[k]).product();
    }
}

impl Block for Product {
    fn evaluate(&mut self, _ctx: &Context, signals: &[f64], out: &mut [f64]) {
        self.write(signals, out);
    }
    fn preview(&self, _t: f64, signals: &[f64], out: &mut [f64]) {
        self.write(signals, out);
    }
}

pub(super) struct Quotient {
    numerator: usize,
    denominator: usize,
    /// Smallest magnitude of the denominator.
    guard: Option<f64>,
}

impl Quotient {
    pub fn new(q: &model::Quotient, b: &Build) -> Self {
        Quotient {
            numerator: b.signal(&q.inputs[0]),
            denominator: b.signal(&q.inputs[1]),
            guard: q.minimum_denominator,
        }
    }
}

impl Quotient {
    /// The outputs for the inputs in `signals`.
    fn write(&self, signals: &[f64], out: &mut [f64]) {
        let mut den = signals[self.denominator];
        if let Some(m) = self.guard {
            if den.abs() < m {
                den = if den < 0.0 { -m } else { m };
            }
        }
        // Without a guard, a denominator of 0 gives a value that is not
        // finite, which stops the run with the signal named.
        out[0] = signals[self.numerator] / den;
    }
}

impl Block for Quotient {
    fn evaluate(&mut self, _ctx: &Context, signals: &[f64], out: &mut [f64]) {
        self.write(signals, out);
    }
    fn preview(&self, _t: f64, signals: &[f64], out: &mut [f64]) {
        self.write(signals, out);
    }
}

pub(super) struct Function {
    kind: model::FunctionKind,
    inputs: Vec<usize>,
}

impl Function {
    pub fn new(f: &model::Function, b: &Build) -> Self {
        Function {
            kind: f.function,
            inputs: b.signals(&f.inputs),
        }
    }
}

impl Function {
    /// The outputs for the inputs in `signals`.
    fn write(&self, signals: &[f64], out: &mut [f64]) {
        use model::FunctionKind as F;
        let x = signals[self.inputs[0]];
        let second = || signals[self.inputs[1]];
        let rest = || self.inputs[1..].iter().map(|&k| signals[k]);
        out[0] = match self.kind {
            F::Sin => x.sin(),
            F::Cos => x.cos(),
            F::Tan => x.tan(),
            F::Atan => x.atan(),
            F::Atan2 => x.atan2(second()),
            F::Abs => x.abs(),
            F::Sqrt => x.sqrt(),
            F::Exp => x.exp(),
            F::Ln => x.ln(),
            F::Sign => {
                if x > 0.0 {
                    1.0
                } else if x < 0.0 {
                    -1.0
                } else {
                    // Also NaN stays NaN.
                    x
                }
            }
            // A NaN among the inputs must stay one: f64::min would drop it.
            F::Min => rest().fold(x, |a, v| if v < a || v.is_nan() { v } else { a }),
            F::Max => rest().fold(x, |a, v| if v > a || v.is_nan() { v } else { a }),
            F::Hypot => x.hypot(second()),
        };
    }
}

impl Block for Function {
    fn evaluate(&mut self, _ctx: &Context, signals: &[f64], out: &mut [f64]) {
        self.write(signals, out);
    }
    fn preview(&self, _t: f64, signals: &[f64], out: &mut [f64]) {
        self.write(signals, out);
    }
}

pub(super) struct Selector {
    condition: usize,
    when_true: usize,
    when_false: usize,
}

impl Selector {
    pub fn new(s: &model::Selector, b: &Build) -> Self {
        Selector {
            condition: b.signal(&s.condition),
            when_true: b.signal(&s.inputs[0]),
            when_false: b.signal(&s.inputs[1]),
        }
    }
}

impl Selector {
    /// The outputs for the inputs in `signals`.
    fn write(&self, signals: &[f64], out: &mut [f64]) {
        out[0] = if signals[self.condition] >= 0.5 {
            signals[self.when_true]
        } else {
            signals[self.when_false]
        };
    }
}

impl Block for Selector {
    fn evaluate(&mut self, _ctx: &Context, signals: &[f64], out: &mut [f64]) {
        self.write(signals, out);
    }
    fn preview(&self, _t: f64, signals: &[f64], out: &mut [f64]) {
        self.write(signals, out);
    }
}

pub(super) struct PiecewiseLinear {
    input: usize,
    x: Vec<f64>,
    y: Vec<f64>,
}

impl PiecewiseLinear {
    pub fn new(p: &model::PiecewiseLinear, b: &Build) -> Self {
        PiecewiseLinear {
            input: b.signal(&p.input),
            x: p.x.clone(),
            y: p.y.clone(),
        }
    }

    /// The outputs for the inputs in `signals`.
    fn write(&self, signals: &[f64], out: &mut [f64]) {
        let u = signals[self.input];
        out[0] = if !u.is_finite() {
            f64::NAN
        } else if self.x.len() == 1 || u <= self.x[0] {
            self.y[0]
        } else if u >= self.x[self.x.len() - 1] {
            self.y[self.x.len() - 1]
        } else {
            let k = self.x.partition_point(|&xk| xk <= u) - 1;
            let frac = (u - self.x[k]) / (self.x[k + 1] - self.x[k]);
            self.y[k] + frac * (self.y[k + 1] - self.y[k])
        };
    }
}

impl Block for PiecewiseLinear {
    fn evaluate(&mut self, _ctx: &Context, signals: &[f64], out: &mut [f64]) {
        self.write(signals, out);
    }
    fn preview(&self, _t: f64, signals: &[f64], out: &mut [f64]) {
        self.write(signals, out);
    }
}

/// The value the input had at t = 0.
pub(super) struct InitialValue {
    input: usize,
    value: f64,
}

impl InitialValue {
    pub fn new(i: &model::InitialValue, b: &Build) -> Self {
        InitialValue {
            input: b.signal(&i.input),
            value: 0.0,
        }
    }
}

impl Block for InitialValue {
    fn evaluate(&mut self, ctx: &Context, signals: &[f64], out: &mut [f64]) {
        if ctx.start {
            self.value = signals[self.input];
        }
        out[0] = self.value;
    }
}
