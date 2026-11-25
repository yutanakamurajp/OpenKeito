//! Blocks that act on a vector in the rotating frame: the limit of its
//! magnitude with a priority, and the pair of PI controllers with a common
//! limit.

use openkeito_model as model;

use super::{Block, Build, Context};

/// (d, q) with a magnitude of at most `max`. With a priority, that
/// component is clamped to +-max and the other one to what is left
/// [WECC REEC_A, current limit logic]; without, both are scaled.
#[inline]
pub(super) fn limited(d: f64, q: f64, max: f64, priority: model::DqPriority) -> [f64; 2] {
    // The first component is clamped, the second gets the rest.
    let first = |a: f64, b: f64| {
        let a = a.clamp(-max, max);
        let rest = (max * max - a * a).max(0.0).sqrt();
        [a, b.clamp(-rest, rest)]
    };
    match priority {
        model::DqPriority::D => first(d, q),
        model::DqPriority::Q => {
            let [q, d] = first(q, d);
            [d, q]
        }
        model::DqPriority::Proportional => {
            let magnitude = d.hypot(q);
            if magnitude > max {
                [d * max / magnitude, q * max / magnitude]
            } else {
                // Also a value that is not a number stays what it is.
                [d, q]
            }
        }
    }
}

pub(super) struct DqLimiter {
    d: usize,
    q: usize,
    max: f64,
    priority: model::DqPriority,
}

impl DqLimiter {
    pub fn new(l: &model::DqLimiter, b: &Build) -> Self {
        DqLimiter {
            d: b.signal(&l.d),
            q: b.signal(&l.q),
            max: l.maximum,
            priority: l.priority,
        }
    }

    /// The outputs for the inputs in `signals`.
    fn write(&self, signals: &[f64], out: &mut [f64]) {
        let (d, q) = (signals[self.d], signals[self.q]);
        // f64::clamp keeps NaN, so a signal that is not a number is reported.
        out.copy_from_slice(&limited(d, q, self.max, self.priority));
    }
}

impl Block for DqLimiter {
    fn evaluate(&mut self, _ctx: &Context, signals: &[f64], out: &mut [f64]) {
        self.write(signals, out);
    }
    fn preview(&self, _t: f64, signals: &[f64], out: &mut [f64]) {
        self.write(signals, out);
    }
}

/// Two PI controllers with a common limit of the magnitude of their output
/// vector. Like `Pi`, the integrals follow the trapezoidal rule and are held
/// while the output is beyond the limit and the step would drive it further
/// out; here "further out" is a step of the integrals with a component along
/// the unlimited output vector.
pub(super) struct PiDq {
    inputs: [usize; 2],
    feedforward: Option<[usize; 2]>,
    kp: f64,
    ki: f64,
    max: Option<f64>,
    integral: [f64; 2],
    /// Signals whose values at t = 0 are the integrals at t = 0.
    initial: [Option<usize>; 2],
    /// Inputs and time of the last grid point.
    previous: Option<([f64; 2], f64)>,
    /// The integrals at the grid point being evaluated.
    pending: [f64; 2],
}

impl PiDq {
    pub fn new(p: &model::PiDq, b: &Build) -> Self {
        let mut integral = [0.0; 2];
        let mut initial = [None; 2];
        for k in 0..2 {
            match &p.initial_integral[k] {
                model::StartValue::Value(v) => integral[k] = *v,
                model::StartValue::Signal(s) => initial[k] = Some(b.signal(s)),
            }
        }
        PiDq {
            inputs: [b.signal(&p.inputs[0]), b.signal(&p.inputs[1])],
            feedforward: p
                .feedforward
                .as_ref()
                .map(|f| [b.signal(&f[0]), b.signal(&f[1])]),
            kp: p.proportional_gain,
            ki: p.integral_gain_per_s,
            max: p.output_max,
            integral,
            initial,
            previous: None,
            pending: integral,
        }
    }
}

impl Block for PiDq {
    fn evaluate(&mut self, ctx: &Context, signals: &[f64], out: &mut [f64]) {
        let e = self.inputs.map(|k| signals[k]);
        if ctx.start {
            for k in 0..2 {
                if let Some(s) = self.initial[k] {
                    self.integral[k] = signals[s];
                }
            }
        }
        let ff = self.feedforward.map_or([0.0; 2], |f| f.map(|k| signals[k]));
        let output = |x: [f64; 2]| [self.kp * e[0] + x[0] + ff[0], self.kp * e[1] + x[1] + ff[1]];
        let mut integral = self.integral;
        if let Some((e_prev, t_prev)) = self.previous {
            let half = self.ki * 0.5 * (ctx.t - t_prev);
            let delta = [half * (e[0] + e_prev[0]), half * (e[1] + e_prev[1])];
            let stepped = [integral[0] + delta[0], integral[1] + delta[1]];
            let u = output(stepped);
            let held = self.max.is_some_and(|m| u[0].hypot(u[1]) > m)
                && delta[0] * u[0] + delta[1] * u[1] > 0.0;
            if !held {
                integral = stepped;
            }
        }
        self.pending = integral;
        let u = output(integral);
        let y = match self.max {
            Some(m) => limited(u[0], u[1], m, model::DqPriority::Proportional),
            None => u,
        };
        out.copy_from_slice(&y);
    }
    fn has_state(&self) -> bool {
        true
    }
    fn commit(&mut self, ctx: &Context, signals: &[f64]) {
        self.integral = self.pending;
        self.previous = Some((self.inputs.map(|k| signals[k]), ctx.t));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::Controls;

    fn controls(h: f64, json: serde_json::Value) -> Controls {
        let blocks: Vec<model::Control> = serde_json::from_value(json).unwrap();
        Controls::new(&blocks, h, |_| 0)
    }

    #[test]
    fn the_limit_keeps_the_component_with_priority() {
        use model::DqPriority::{Proportional, D, Q};
        // Inside the limit nothing changes.
        for priority in [D, Q, Proportional] {
            assert_eq!(limited(0.6, -0.5, 1.0, priority), [0.6, -0.5]);
        }
        // (0.8, -0.9) has the magnitude 1.204. With priority d, d stays and
        // q gets sqrt(1 - 0.64) = 0.6 with its sign; with priority q, q stays
        // and d gets sqrt(1 - 0.81).
        let [d, q] = limited(0.8, -0.9, 1.0, D);
        assert!(
            (d - 0.8).abs() < 1e-15 && (q + 0.6).abs() < 1e-15,
            "{d} {q}"
        );
        let [d, q] = limited(0.8, -0.9, 1.0, Q);
        assert!((q + 0.9).abs() < 1e-15 && (d - 0.19_f64.sqrt()).abs() < 1e-15);
        // Proportional: the direction stays.
        let [d, q] = limited(0.8, -0.9, 1.0, Proportional);
        assert!((d.hypot(q) - 1.0).abs() < 1e-15);
        assert!((d / q - 0.8 / -0.9).abs() < 1e-15);
        // The component with priority alone above the limit takes all.
        assert_eq!(limited(-1.5, 0.3, 1.1, D), [-1.1, 0.0]);
        assert_eq!(limited(0.3, 2.0, 1.1, Q), [0.0, 1.1]);
        // A value that is not a number is not hidden.
        assert!(limited(f64::NAN, 0.0, 1.0, D)[0].is_nan());
        assert!(limited(f64::NAN, 0.0, 1.0, Proportional)[0].is_nan());
    }

    #[test]
    fn pi_dq_is_two_pi_blocks_while_inside_its_limit() {
        // The same inputs through pi_dq and through two pi blocks with the
        // feedforward added by sums: equal to rounding.
        let h = 1e-3;
        let mut k = controls(
            h,
            serde_json::json!([
                {"type": "measurement", "name": "ed", "quantity": {"type": "node_voltage", "node": "n1"}},
                {"type": "gain", "name": "eq", "input": "ed", "gain": -0.5},
                {"type": "ramp", "name": "f", "slope_per_s": 3.0},
                {"type": "pi_dq", "name": "u", "inputs": ["ed", "eq"], "feedforward": ["f", "ed"],
                 "proportional_gain": 2.0, "integral_gain_per_s": 100.0, "initial_integral": [0.5, "eq"]},
                {"type": "pi", "name": "pd", "input": "ed", "proportional_gain": 2.0, "integral_gain_per_s": 100.0, "initial_integral": 0.5},
                {"type": "pi", "name": "pq", "input": "eq", "proportional_gain": 2.0, "integral_gain_per_s": 100.0, "initial_integral": "eq"},
                {"type": "sum", "name": "rd", "inputs": ["pd", "f"], "signs": [1.0, 1.0]},
                {"type": "sum", "name": "rq", "inputs": ["pq", "ed"], "signs": [1.0, 1.0]},
            ]),
        );
        for n in 0..50 {
            let t = n as f64 * h;
            k.evaluate(t, &[(40.0 * t).sin() + 0.2]).unwrap();
            let v = &k.values;
            assert!((v[3] - v[7]).abs() < 1e-13, "d at step {n}");
            assert!((v[4] - v[8]).abs() < 1e-13, "q at step {n}");
        }
    }

    #[test]
    fn pi_dq_holds_its_integrals_at_the_limit_and_leaves_it_at_once() {
        let h = 1e-3;
        let mut k = controls(
            h,
            serde_json::json!([
                {"type": "measurement", "name": "e", "quantity": {"type": "node_voltage", "node": "n1"}},
                {"type": "constant", "name": "zero", "value": 0.0},
                {"type": "pi_dq", "name": "u", "inputs": ["e", "zero"], "proportional_gain": 1.0,
                 "integral_gain_per_s": 100.0, "output_max": 2.05},
            ]),
        );
        // A constant error of 1: the integral grows by 0.1 per step. After
        // 10 steps the output is 2; the next step would take it to 2.1,
        // beyond the limit, so the integral is held at 1 from there on (it
        // would be 21 after 200 more steps without the hold).
        for n in 0..=210 {
            k.evaluate(n as f64 * h, &[1.0]).unwrap();
            let expected = 1.0 + 0.1 * n.min(10) as f64;
            assert!((k.values[2] - expected).abs() < 1e-12, "step {n}");
        }
        // The error reverses: the output is -0.5 + 1 + 100 h (1 - 0.5)/2 at
        // once, far from the limit.
        k.evaluate(211.0 * h, &[-0.5]).unwrap();
        assert!(
            (k.values[2] - (0.5 + 0.025)).abs() < 1e-12,
            "{}",
            k.values[2]
        );
        assert_eq!(k.values[3], 0.0);
    }

    #[test]
    fn pi_dq_scales_the_vector_and_integrates_what_brings_it_back() {
        // The feedforward alone is beyond the limit along d. An error along
        // -d brings the vector back and is integrated; an error along +d is
        // not.
        let h = 1e-3;
        let build = |error: f64| {
            let mut k = controls(
                h,
                serde_json::json!([
                    {"type": "constant", "name": "ed", "value": error},
                    {"type": "constant", "name": "eq", "value": 0.0},
                    {"type": "constant", "name": "fd", "value": 3.0},
                    {"type": "constant", "name": "fq", "value": 4.0},
                    {"type": "pi_dq", "name": "u", "inputs": ["ed", "eq"], "feedforward": ["fd", "fq"],
                     "proportional_gain": 0.0, "integral_gain_per_s": 1000.0, "output_max": 2.5},
                ]),
            );
            for n in 0..=2 {
                k.evaluate(n as f64 * h, &[]).unwrap();
            }
            (k.values[4], k.values[5])
        };
        // Held: the output is (3, 4) scaled to the magnitude 2.5.
        let (d, q) = build(1.0);
        assert!(
            (d - 1.5).abs() < 1e-12 && (q - 2.0).abs() < 1e-12,
            "{d} {q}"
        );
        // Integrated: after two steps the integral is -2, the vector (1, 4)
        // scaled to 2.5.
        let (d, q) = build(-1.0);
        let m = 17.0_f64.sqrt();
        assert!(
            (d - 2.5 / m).abs() < 1e-12 && (q - 10.0 / m).abs() < 1e-12,
            "{d} {q}"
        );
    }
}
