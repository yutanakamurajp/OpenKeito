//! The blocks with a state: transfer function, integrator, first-order lag,
//! lead-lag and delay.
//!
//! For a state x with x' = f(x, u), the trapezoidal rule from the last grid
//! point to the present one is x_new = x + h/2 (f(x, u_prev) + f(x_new, u)).
//! The blocks here are linear in x, so each solves this for x_new in closed
//! form; only the present input u may still change while a loop settles.

use openkeito_model as model;

use super::{apply_limits, Block, Bound, Build, Context};

/// The state at t = 0 of a block that starts in the steady state of an
/// input: the given constant input, or the input at t = 0.
#[inline]
fn steady_input(given: Option<f64>, u: f64) -> f64 {
    given.unwrap_or(u)
}

/// N(s)/D(s) in controllable canonical form: x' = A x + B u, y = C x + D u,
/// with A the companion matrix of D(s). The trapezoidal rule gives
/// x_new = P x + g (u_prev + u) with P = (I - h/2 A)^-1 (I + h/2 A) and
/// g = (I - h/2 A)^-1 B h/2, computed once.
pub(super) struct TransferFunction {
    input: usize,
    n: usize,
    /// P, row by row.
    p: Vec<f64>,
    g: Vec<f64>,
    c: Vec<f64>,
    d: f64,
    /// 1 / (last denominator coefficient), 0 for a pole at s = 0: the first
    /// state per unit of a constant input.
    steady: f64,
    initial_input: Option<f64>,
    x: Vec<f64>,
    next: Vec<f64>,
    u_prev: f64,
}

impl TransferFunction {
    pub fn new(t: &model::TransferFunction, b: &Build) -> Self {
        let n = t.denominator.len() - 1;
        let lead = t.denominator[0];
        // a[k] and num[k] belong to s^(n-k).
        let a: Vec<f64> = t.denominator.iter().map(|v| v / lead).collect();
        let mut num = vec![0.0; n + 1 - t.numerator.len()];
        num.extend(t.numerator.iter().map(|v| v / lead));
        let d = num[0];
        // State j pairs with the coefficients of s^j.
        let c: Vec<f64> = (0..n).map(|j| num[n - j] - a[n - j] * d).collect();
        let half = b.h / 2.0;
        // [I - h/2 A | I + h/2 A | h/2 B], reduced to [I | P | g].
        let width = 2 * n + 1;
        let mut m = vec![0.0; n * width];
        for i in 0..n {
            m[i * width + i] = 1.0;
            m[i * width + n + i] = 1.0;
            for j in 0..n {
                let a_ij = if i + 1 < n {
                    f64::from(u8::from(j == i + 1))
                } else {
                    -a[n - j]
                };
                m[i * width + j] -= half * a_ij;
                m[i * width + n + j] += half * a_ij;
            }
        }
        if n > 0 {
            m[(n - 1) * width + 2 * n] = half;
        }
        for col in 0..n {
            let pivot = (col..n)
                .max_by(|&i, &j| {
                    m[i * width + col]
                        .abs()
                        .total_cmp(&m[j * width + col].abs())
                })
                .expect("col < n");
            for k in 0..width {
                m.swap(col * width + k, pivot * width + k);
            }
            // A pivot of 0 (a pole at exactly 2/h) leaves values that are
            // not finite; the run then stops with the signal named.
            let scale = 1.0 / m[col * width + col];
            for k in 0..width {
                m[col * width + k] *= scale;
            }
            for i in 0..n {
                let f = m[i * width + col];
                if i != col && f != 0.0 {
                    for k in 0..width {
                        m[i * width + k] -= f * m[col * width + k];
                    }
                }
            }
        }
        let p = (0..n * n).map(|k| m[(k / n) * width + n + k % n]).collect();
        let g = (0..n).map(|i| m[i * width + 2 * n]).collect();
        TransferFunction {
            input: b.signal(&t.input),
            n,
            p,
            g,
            c,
            d,
            steady: if n > 0 && a[n] != 0.0 {
                1.0 / a[n]
            } else {
                0.0
            },
            initial_input: t.initial_input,
            x: vec![0.0; n],
            next: vec![0.0; n],
            u_prev: 0.0,
        }
    }
}

impl Block for TransferFunction {
    fn evaluate(&mut self, ctx: &Context, signals: &[f64], out: &mut [f64]) {
        let u = signals[self.input];
        let n = self.n;
        if ctx.start {
            // A x + B u0 = 0: only the first state is not 0.
            self.next.fill(0.0);
            if n > 0 {
                self.next[0] = self.steady * steady_input(self.initial_input, u);
            }
        } else {
            let drive = self.u_prev + u;
            for i in 0..n {
                let row = &self.p[i * n..(i + 1) * n];
                self.next[i] =
                    row.iter().zip(&self.x).map(|(p, x)| p * x).sum::<f64>() + self.g[i] * drive;
            }
        }
        out[0] = self
            .c
            .iter()
            .zip(&self.next)
            .map(|(c, x)| c * x)
            .sum::<f64>()
            + self.d * u;
    }
    fn has_state(&self) -> bool {
        true
    }
    fn commit(&mut self, _ctx: &Context, signals: &[f64]) {
        self.x.copy_from_slice(&self.next);
        self.u_prev = signals[self.input];
    }
}

/// x' = K u, held within limits or folded back into them.
pub(super) struct Integrator {
    input: usize,
    /// K h/2.
    half_gain: f64,
    min: Option<Bound>,
    max: Option<Bound>,
    wrap: bool,
    initial: f64,
    /// Signal whose value at t = 0 is the state at t = 0.
    initial_signal: Option<usize>,
    x: f64,
    next: f64,
    u_prev: f64,
}

impl Integrator {
    pub fn new(i: &model::Integrator, b: &Build) -> Self {
        let (initial, initial_signal) = match &i.initial_value {
            model::StartValue::Value(v) => (*v, None),
            model::StartValue::Signal(s) => (0.0, Some(b.signal(s))),
        };
        Integrator {
            input: b.signal(&i.input),
            half_gain: i.gain_per_s * b.h / 2.0,
            min: Bound::from_limit(i.minimum.as_ref(), b),
            max: Bound::from_limit(i.maximum.as_ref(), b),
            wrap: i.wrap,
            initial,
            initial_signal,
            x: 0.0,
            next: 0.0,
            u_prev: 0.0,
        }
    }

    /// The state within its limits: held at a limit, so that it leaves the
    /// limit as soon as the input changes sign, or folded back.
    fn limited(&self, x: f64, signals: &[f64]) -> f64 {
        let min = self.min.as_ref().map(|b| b.get(signals));
        let max = self.max.as_ref().map(|b| b.get(signals));
        if self.wrap {
            match (min, max) {
                (Some(lo), Some(hi))
                    if x.is_finite() && lo.is_finite() && hi.is_finite() && hi > lo =>
                {
                    lo + wrapped(x - lo, hi - lo)
                }
                _ => f64::NAN,
            }
        } else {
            apply_limits(x, min, max)
        }
    }
}

/// `x` folded into [0, period).
#[inline]
pub(super) fn wrapped(x: f64, period: f64) -> f64 {
    let r = x.rem_euclid(period);
    // A tiny negative x rounds to the period itself.
    if r >= period {
        0.0
    } else {
        r
    }
}

impl Block for Integrator {
    fn evaluate(&mut self, ctx: &Context, signals: &[f64], out: &mut [f64]) {
        let x = if ctx.start {
            self.initial_signal.map_or(self.initial, |s| signals[s])
        } else {
            self.x + self.half_gain * (self.u_prev + signals[self.input])
        };
        self.next = self.limited(x, signals);
        out[0] = self.next;
    }
    fn has_state(&self) -> bool {
        true
    }
    fn commit(&mut self, _ctx: &Context, signals: &[f64]) {
        self.x = self.next;
        self.u_prev = signals[self.input];
    }
}

/// T y' = K u - y, with y held within limits (non-windup).
pub(super) struct Lag {
    input: usize,
    gain: f64,
    /// h / (2 T).
    a: f64,
    min: Option<Bound>,
    max: Option<Bound>,
    initial_input: Option<f64>,
    y: f64,
    next: f64,
    u_prev: f64,
}

impl Lag {
    pub fn new(l: &model::Lag, b: &Build) -> Self {
        Lag {
            input: b.signal(&l.input),
            gain: l.gain,
            a: b.h / (2.0 * l.time_constant_s),
            min: Bound::from_limit(l.minimum.as_ref(), b),
            max: Bound::from_limit(l.maximum.as_ref(), b),
            initial_input: l.initial_input,
            y: 0.0,
            next: 0.0,
            u_prev: 0.0,
        }
    }
}

impl Block for Lag {
    fn evaluate(&mut self, ctx: &Context, signals: &[f64], out: &mut [f64]) {
        let u = signals[self.input];
        let min = self.min.as_ref().map(|b| b.get(signals));
        let max = self.max.as_ref().map(|b| b.get(signals));
        let y = if ctx.start {
            self.gain * steady_input(self.initial_input, u)
        } else {
            ((1.0 - self.a) * self.y + self.a * self.gain * (self.u_prev + u)) / (1.0 + self.a)
        };
        // The state itself is limited: it leaves a limit as soon as K u is
        // back inside it.
        self.next = apply_limits(y, min, max);
        out[0] = self.next;
    }
    fn has_state(&self) -> bool {
        true
    }
    fn commit(&mut self, _ctx: &Context, signals: &[f64]) {
        self.y = self.next;
        self.u_prev = signals[self.input];
    }
}

/// K (1 + s T1)/(1 + s T2) as y = K (r u + (1 - r) x) with r = T1/T2 and
/// the lag T2 x' = u - x.
pub(super) struct LeadLag {
    input: usize,
    gain: f64,
    ratio: f64,
    /// h / (2 T2).
    a: f64,
    initial_input: Option<f64>,
    x: f64,
    next: f64,
    u_prev: f64,
}

impl LeadLag {
    pub fn new(l: &model::LeadLag, b: &Build) -> Self {
        LeadLag {
            input: b.signal(&l.input),
            gain: l.gain,
            ratio: l.lead_time_constant_s / l.lag_time_constant_s,
            a: b.h / (2.0 * l.lag_time_constant_s),
            initial_input: l.initial_input,
            x: 0.0,
            next: 0.0,
            u_prev: 0.0,
        }
    }
}

impl Block for LeadLag {
    fn evaluate(&mut self, ctx: &Context, signals: &[f64], out: &mut [f64]) {
        let u = signals[self.input];
        self.next = if ctx.start {
            steady_input(self.initial_input, u)
        } else {
            ((1.0 - self.a) * self.x + self.a * (self.u_prev + u)) / (1.0 + self.a)
        };
        out[0] = self.gain * (self.ratio * u + (1.0 - self.ratio) * self.next);
    }
    fn has_state(&self) -> bool {
        true
    }
    fn commit(&mut self, _ctx: &Context, signals: &[f64]) {
        self.x = self.next;
        self.u_prev = signals[self.input];
    }
}

/// u(t - d) with d = (k + r) h, 0 <= r < 1: the inputs of the last k + 1
/// grid points are kept and the two around t - d joined by a straight line.
pub(super) struct Delay {
    input: usize,
    k: usize,
    r: f64,
    initial: Option<f64>,
    /// The inputs of the last k + 1 grid points; `head` is the latest.
    ring: Vec<f64>,
    head: usize,
}

impl Delay {
    pub fn new(d: &model::Delay, b: &Build) -> Self {
        let steps = d.delay_s / b.h;
        // Within rounding of a whole number of steps counts as that number.
        let k = (steps + 1e-9).floor();
        let r = if steps - k < 1e-9 { 0.0 } else { steps - k };
        let k = k as usize;
        Delay {
            input: b.signal(&d.input),
            k,
            r,
            initial: d.initial_output,
            ring: vec![0.0; k + 1],
            head: 0,
        }
    }

    /// The input `back` grid points before the last one.
    #[inline]
    fn past(&self, back: usize) -> f64 {
        let n = self.ring.len();
        self.ring[(self.head + n - back) % n]
    }
}

impl Block for Delay {
    fn evaluate(&mut self, ctx: &Context, signals: &[f64], out: &mut [f64]) {
        let u = signals[self.input];
        if ctx.start {
            // Before t = 0 the input was the initial output, or what it is
            // at t = 0.
            self.ring.fill(self.initial.unwrap_or(u));
        }
        let newer = if self.k == 0 {
            u
        } else {
            self.past(self.k - 1)
        };
        out[0] = if self.r == 0.0 {
            newer
        } else {
            (1.0 - self.r) * newer + self.r * self.past(self.k)
        };
    }
    fn has_state(&self) -> bool {
        true
    }
    fn commit(&mut self, _ctx: &Context, signals: &[f64]) {
        self.head = (self.head + 1) % self.ring.len();
        self.ring[self.head] = signals[self.input];
    }
}
