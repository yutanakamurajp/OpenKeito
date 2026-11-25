//! Control blocks, three-phase measurements and controlled sources against
//! closed-form solutions.

mod common;

use std::f64::consts::PI;

use common::{diagnostics, try_run, Run};
use openkeito_model::Severity;
use serde_json::{json, Value};

impl Run {
    /// The values of the column `name`, or of the signal of that name.
    fn column(&self, name: &str) -> Vec<f64> {
        let signal = format!("S({name})");
        let k = self
            .columns
            .iter()
            .position(|n| n == name || *n == signal)
            .unwrap_or_else(|| panic!("no column {name} among {:?}", self.columns));
        self.rows.iter().map(|r| r[k]).collect()
    }

    /// Largest |column - exact(t)| over the rows from time `from` on.
    fn error_from(&self, name: &str, from: f64, exact: impl Fn(f64) -> f64) -> f64 {
        self.t
            .iter()
            .zip(self.column(name))
            .filter(|(t, _)| **t >= from)
            .map(|(t, v)| (v - exact(*t)).abs())
            .fold(0.0, f64::max)
    }

    fn error(&self, name: &str, exact: impl Fn(f64) -> f64) -> f64 {
        self.error_from(name, 0.0, exact)
    }
}

fn run(model: &Value) -> Run {
    try_run(model).expect("run completes")
}

/// A model whose circuit is a source across a resistor, which the controls
/// do not touch: the blocks alone, with the signals `signals` recorded.
fn blocks(h: f64, end: f64, controls: Value, signals: &[&str]) -> Value {
    json!({
        "format": "openkeito-model",
        "version": "0.5",
        "simulation": {"time_step_s": h, "end_time_s": end},
        "components": [
            {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {"type": "dc", "value": 1.0}},
            {"type": "resistor", "name": "R1", "nodes": ["n1", "gnd"], "resistance_ohm": 1.0},
        ],
        "controls": controls,
        "outputs": signals.iter().map(|s| json!({"type": "signal", "signal": s})).collect::<Vec<_>>(),
    })
}

fn blocks_07(h: f64, end: f64, controls: Value, signals: &[&str]) -> Value {
    let mut m = blocks(h, end, controls, signals);
    m["version"] = json!("0.7");
    m
}

/// The error of a run with the step `h` and of one with half of it.
fn errors_at(h: f64, error: impl Fn(f64) -> f64) -> (f64, f64) {
    (error(h), error(h / 2.0))
}

/// Halving the step of a method of second order divides the error by 4.
fn assert_second_order(what: &str, (coarse, fine): (f64, f64)) {
    let ratio = coarse / fine;
    assert!(
        (3.7..4.3).contains(&ratio),
        "{what}: halving the step changed the error from {coarse:e} to {fine:e}, by {ratio}"
    );
}

/// e^(-sigma t) (cos(wd t) + k sin(wd t)).
fn damped(t: f64, sigma: f64, wd: f64, k: f64) -> f64 {
    (-sigma * t).exp() * ((wd * t).cos() + k * (wd * t).sin())
}

#[test]
fn first_order_lag_and_transfer_function_follow_the_exponential() {
    // 2 / (1 + 0.01 s) from rest with the input 1 from t = 0:
    // y = 2 (1 - e^(-t/T)). The trapezoidal rule multiplies the deviation
    // from the final value by (1 - a)/(1 + a) per step, a = h/2T, which is
    // e^(-h/T) (1 - (h/T)^3/12): after t the error is
    // K (t/T) e^(-t/T) (h/T)^2 / 12, at most K (h/T)^2 / (12 e).
    let (gain, tau) = (2.0, 0.01);
    let model = |h: f64| {
        blocks(
            h,
            0.05,
            json!([
                {"type": "constant", "name": "u", "value": 1.0},
                {"type": "lag", "name": "y", "input": "u", "gain": gain, "time_constant_s": tau, "initial_input": 0.0},
                {"type": "transfer_function", "name": "g", "input": "u", "numerator": [gain], "denominator": [tau, 1.0], "initial_input": 0.0},
            ]),
            &["y", "g"],
        )
    };
    let exact = |t: f64| gain * (1.0 - (-t / tau).exp());
    let h = 5e-4;
    let bound = gain * (h / tau).powi(2) / (12.0 * 1.0_f64.exp());
    let errors = errors_at(h, |h| run(&model(h)).error("y", exact));
    // The estimate is the leading term in h/T = 0.05.
    assert!(
        errors.0 < 1.02 * bound && errors.0 > 0.98 * bound,
        "{errors:?} against {bound:e}"
    );
    assert_second_order("lag", errors);
    // The transfer function is the same block in another form.
    let r = run(&model(h));
    for (y, g) in r.column("y").iter().zip(r.column("g")) {
        assert!((y - g).abs() < 1e-13, "{y} and {g}");
    }
    assert_eq!(r.rows[0], [0.0, 0.0]);
}

#[test]
fn second_order_transfer_function_follows_the_damped_oscillation() {
    // wn^2 / (s^2 + 2 zeta wn s + wn^2) from rest, input 1:
    // y = 1 - e^(-zeta wn t) (cos(wd t) + zeta/sqrt(1 - zeta^2) sin(wd t)).
    // The trapezoidal rule lowers the frequency by (wn h)^2/12 of itself, so
    // the phase is late by wn t (wn h)^2 / 12; with the envelope
    // e^(-zeta wn t) / sqrt(1 - zeta^2) the error is at most
    // (wn h)^2 / (12 e zeta sqrt(1 - zeta^2)), 4.3e-5 here (and a little
    // more from the damping, which is of the same order).
    let (wn, zeta): (f64, f64) = (100.0, 0.3);
    let root = (1.0 - zeta * zeta).sqrt();
    let model = |h: f64| {
        blocks(
            h,
            0.1,
            json!([
                {"type": "constant", "name": "u", "value": 1.0},
                {"type": "transfer_function", "name": "y", "input": "u", "numerator": [wn * wn],
                 "denominator": [1.0, 2.0 * zeta * wn, wn * wn], "initial_input": 0.0},
            ]),
            &["y"],
        )
    };
    let exact = |t: f64| 1.0 - damped(t, zeta * wn, wn * root, zeta / root);
    let h: f64 = 2e-4;
    let bound = (wn * h).powi(2) / (12.0 * 1.0_f64.exp() * zeta * root);
    let errors = errors_at(h, |h| run(&model(h)).error("y", exact));
    assert!(errors.0 < 1.5 * bound, "{errors:?} against {bound:e}");
    assert_second_order("second order", errors);
    // Without initial_input the block starts in the steady state of its
    // input at t = 0 and stays there.
    let mut steady = model(h);
    steady["controls"][1]
        .as_object_mut()
        .unwrap()
        .remove("initial_input");
    let e = run(&steady).error("y", |_| 1.0);
    assert!(e < 1e-12, "{e:e}");
}

#[test]
fn a_ramp_is_integrated_exactly_and_a_proper_transfer_function_passes_its_input() {
    // The trapezoidal rule is exact for a ramp: with u = 3 t and gain 2,
    // y = 0.5 + 3 t^2, to rounding.
    let r = run(&blocks(
        1e-3,
        1.0,
        json!([
            {"type": "ramp", "name": "u", "slope_per_s": 3.0},
            {"type": "integrator", "name": "y", "input": "u", "gain_per_s": 2.0, "initial_value": 0.5},
            // (2 s + 6)/(s + 3) = 2: the direct term alone.
            {"type": "transfer_function", "name": "g", "input": "u", "numerator": [2.0, 6.0], "denominator": [1.0, 3.0]},
        ]),
        &["y", "g"],
    ));
    let e = r.error("y", |t| 0.5 + 3.0 * t * t);
    assert!(e < 1e-12, "{e:e}");
    let e = r.error("g", |t| 6.0 * t);
    assert!(e < 1e-12, "{e:e}");
}

#[test]
fn lead_lag_follows_its_step_response() {
    // K (1 + s T1)/(1 + s T2) from rest, input 1 from t = 0:
    // y = K (1 + (T1/T2 - 1) e^(-t/T2)); it jumps to K T1/T2 at t = 0. The
    // error is that of the lag inside, K |T1/T2 - 1| (h/T2)^2 / (12 e).
    let (k, t1, t2) = (1.5, 0.03, 0.01);
    let model = |h: f64| {
        blocks(
            h,
            0.05,
            json!([
                {"type": "constant", "name": "u", "value": 1.0},
                {"type": "lead_lag", "name": "y", "input": "u", "gain": k,
                 "lead_time_constant_s": t1, "lag_time_constant_s": t2, "initial_input": 0.0},
            ]),
            &["y"],
        )
    };
    let exact = |t: f64| k * (1.0 + (t1 / t2 - 1.0) * (-t / t2).exp());
    let h = 5e-4;
    let bound = k * (t1 / t2 - 1.0) * (h / t2).powi(2) / (12.0 * 1.0_f64.exp());
    let errors = errors_at(h, |h| run(&model(h)).error("y", exact));
    assert!(
        errors.0 < 1.02 * bound && errors.0 > 0.98 * bound,
        "{errors:?} against {bound:e}"
    );
    assert_second_order("lead-lag", errors);
    assert!((run(&model(h)).rows[0][0] - 4.5).abs() < 1e-15);
}

#[test]
fn a_delay_gives_the_input_of_that_time_before() {
    // The input sin(w t) is kept at the grid points and joined by straight
    // lines: a delay of k + r steps is off by at most r (1 - r)/2 (w h)^2
    // (the chord of a sine), and exact for a whole number of steps.
    let (h, f) = (1e-4, 50.0);
    let w = 2.0 * PI * f;
    let delayed = |steps: f64| {
        let d = steps * h;
        let r = run(&blocks(
            h,
            0.04,
            json!([
                {"type": "sine_wave", "name": "u", "amplitude": 1.0, "frequency_hz": f},
                {"type": "delay", "name": "y", "input": "u", "delay_s": d, "initial_output": 0.25},
            ]),
            &["y"],
        ));
        let y = r.column("y");
        // Before the delay has passed the output is the initial output.
        for (t, v) in r.t.iter().zip(&y) {
            if *t < d - h {
                assert_eq!(*v, 0.25, "t = {t}");
            }
        }
        r.error_from("y", d + h, |t| (w * (t - d)).sin())
    };
    for (steps, r) in [(12.25_f64, 0.25_f64), (24.5, 0.5), (0.4, 0.4)] {
        let bound = r * (1.0 - r) / 2.0 * (w * h).powi(2);
        let e = delayed(steps);
        assert!(
            e < 1.001 * bound && e > 0.95 * bound,
            "{steps} steps: {e:e} against {bound:e}"
        );
    }
    for steps in [0.0, 1.0, 12.0] {
        let e = delayed(steps);
        assert!(e < 1e-14, "{steps} steps: {e:e}");
    }
    // Without an initial output the input at t = 0 is taken.
    let r = run(&blocks(
        h,
        2e-3,
        json!([
            {"type": "ramp", "name": "u", "slope_per_s": 1.0, "initial": 3.0},
            {"type": "delay", "name": "y", "input": "u", "delay_s": 1e-3},
        ]),
        &["y"],
    ));
    let e = r.error("y", |t| 3.0 + (t - 1e-3).max(0.0));
    assert!(e < 1e-15, "{e:e}");
}

#[test]
fn a_limited_integrator_leaves_its_limit_when_the_input_changes_sign() {
    // x' = A cos(w t) with A = 2 w, limited to x <= 1. Without the limit
    // x = 2 sin(w t). It reaches 1 at w t = pi/6 and stays until the input
    // changes sign at w t = pi/2 (t = 0.05 s); from there
    // x = 1 + 2 (sin(w t) - 1). An integrator that wound up would follow
    // 2 sin(w t) and come back below 1 only at w t = 5 pi/6.
    //
    // Error: the trapezoidal rule integrates the cosine to (w h)^2/12 of the
    // amplitude 2, and in the step in which the input changes sign the
    // limited sum is off by at most A w h^2 / 2 = (w h)^2. Together
    // (7/6) (w h)^2.
    let w = 2.0 * PI * 5.0;
    let model = |h: f64| {
        blocks(
            h,
            0.2,
            json!([
                {"type": "sine_wave", "name": "u", "amplitude": 2.0 * w, "frequency_hz": 5.0, "phase_deg": 90.0},
                {"type": "integrator", "name": "x", "input": "u", "maximum": 1.0},
            ]),
            &["x"],
        )
    };
    let exact = |t: f64| {
        let x = 2.0 * (w * t).sin();
        if w * t <= PI / 2.0 {
            x.min(1.0)
        } else {
            x - 1.0
        }
    };
    let h = 1e-4;
    let r = run(&model(h));
    let e = r.error("x", exact);
    assert!(e < 7.0 / 6.0 * (w * h).powi(2), "{e:e}");
    // It leaves the limit within a step of t = 0.05 s.
    let x = r.column("x");
    let at_limit = |t: f64| x[(t / h).round() as usize] == 1.0;
    assert!(at_limit(0.02) && at_limit(0.05 - h));
    assert!(!at_limit(0.05 + 2.0 * h));
}

#[test]
fn a_limited_lag_does_not_wind_up() {
    // 1/(1 + s T) limited to 0.5, input 1 until t1 = 0.03 s and 0 from there
    // on: y = 1 - e^(-t/T) until it reaches 0.5 (t = T ln 2), 0.5 until t1,
    // then 0.5 e^(-(t - t1)/T) at once. Without the non-windup limit the
    // state would be at 0.95 at t1 and the output would stay at 0.5 for
    // another T ln 1.9 = 6.4 ms.
    //
    // At the grid point t1 the mean of the old and the new input equals the
    // limit, so the state stays; from there it decays with the factor of the
    // trapezoidal rule: error at most 0.5 (h/T)^2 / (12 e), and
    // (h/T)^2 / (12 e) on the way up.
    let (tau, t1, h) = (0.01, 0.03, 1e-4);
    let r = run(&blocks(
        h,
        0.06,
        json!([
            {"type": "step", "name": "u", "time_s": t1, "before": 1.0, "after": 0.0},
            {"type": "lag", "name": "y", "input": "u", "time_constant_s": tau, "maximum": 0.5, "initial_input": 0.0},
        ]),
        &["y"],
    ));
    let exact = |t: f64| {
        if t <= t1 {
            (1.0 - (-t / tau).exp()).min(0.5)
        } else {
            0.5 * (-(t - t1) / tau).exp()
        }
    };
    let e = r.error("y", exact);
    assert!(e < (h / tau).powi(2) / (12.0 * 1.0_f64.exp()), "{e:e}");
}

#[test]
fn a_rate_limiter_moves_with_its_rates() {
    // The input jumps from 0 to 1 at 0.01 s and to 0.4 at 0.2 s. The output
    // rises by 10 per second and falls by 20 per second; at the grid point
    // of a jump it has already moved by one step.
    let h = 1e-3;
    let r = run(&blocks(
        h,
        0.3,
        json!([
            {"type": "time_series", "name": "u", "times_s": [0.0, 0.01, 0.2], "values": [0.0, 1.0, 0.4]},
            {"type": "rate_limiter", "name": "y", "input": "u", "rising_rate_per_s": 10.0, "falling_rate_per_s": 20.0},
        ]),
        &["y"],
    ));
    let exact = |t: f64| {
        if t < 0.01 - h / 2.0 {
            0.0
        } else if t < 0.2 - h / 2.0 {
            (10.0 * (t - 0.01 + h)).min(1.0)
        } else {
            (1.0 - 20.0 * (t - 0.2 + h)).max(0.4)
        }
    };
    let e = r.error("y", exact);
    assert!(e < 1e-12, "{e:e}");
}

#[test]
fn the_blocks_without_a_state_give_their_closed_forms() {
    // u runs from -1 to 1.
    let r = run(&blocks(
        1e-3,
        2.0,
        json!([
            {"type": "ramp", "name": "u", "slope_per_s": 1.0, "initial": -1.0},
            {"type": "constant", "name": "half", "value": 0.5},
            {"type": "limiter", "name": "limited", "input": "u", "minimum": -0.3, "maximum": 0.6},
            {"type": "dead_band", "name": "band", "input": "u", "lower": -0.2, "upper": 0.4},
            {"type": "product", "name": "square", "inputs": ["u", "u"]},
            {"type": "quotient", "name": "ratio", "inputs": ["half", "u"], "minimum_denominator": 0.1},
            {"type": "function", "name": "sin", "function": "sin", "inputs": ["u"]},
            {"type": "function", "name": "cos", "function": "cos", "inputs": ["u"]},
            {"type": "function", "name": "abs", "function": "abs", "inputs": ["u"]},
            {"type": "function", "name": "root", "function": "sqrt", "inputs": ["square"]},
            {"type": "function", "name": "low", "function": "min", "inputs": ["u", "half", "square"]},
            {"type": "function", "name": "high", "function": "max", "inputs": ["u", "half"]},
            {"type": "function", "name": "angle", "function": "atan2", "inputs": ["u", "half"]},
            {"type": "function", "name": "length", "function": "hypot", "inputs": ["u", "half"]},
            {"type": "function", "name": "sign", "function": "sign", "inputs": ["u"]},
            {"type": "comparator", "name": "positive", "inputs": ["u", "band"]},
            {"type": "selector", "name": "chosen", "condition": "positive", "inputs": ["half", "u"]},
            {"type": "initial_value", "name": "first", "input": "u"},
            {"type": "time_series", "name": "line", "times_s": [0.5, 1.0, 1.5], "values": [1.0, 3.0, 2.0], "interpolation": "linear"},
            {"type": "pi", "name": "pi", "input": "half", "proportional_gain": 2.0, "integral_gain_per_s": 4.0, "initial_integral": "first"},
        ]),
        &[
            "limited", "band", "square", "ratio", "sin", "cos", "abs", "root", "low", "high",
            "angle", "length", "sign", "chosen", "first", "line", "pi",
        ],
    ));
    let check = |name: &str, exact: &dyn Fn(f64) -> f64| {
        let e = r.error(name, |t| exact(t - 1.0));
        assert!(e < 1e-12, "{name}: {e:e}");
    };
    check("limited", &|u| u.clamp(-0.3, 0.6));
    check("band", &|u| {
        if u > 0.4 {
            u - 0.4
        } else if u < -0.2 {
            u + 0.2
        } else {
            0.0
        }
    });
    check("square", &|u| u * u);
    check("sin", &|u| u.sin());
    check("cos", &|u| u.cos());
    check("abs", &|u| u.abs());
    check("root", &|u| u.abs());
    check("low", &|u| u.min(0.5).min(u * u));
    check("high", &|u| u.max(0.5));
    check("angle", &|u| u.atan2(0.5));
    check("length", &|u| u.hypot(0.5));
    check("first", &|_| -1.0);
    // The integral starts at the value of "first" at t = 0.
    check("pi", &|u| 1.0 - 1.0 + 2.0 * (u + 1.0));
    check("line", &|u| {
        let t = u + 1.0;
        if t < 0.5 {
            1.0
        } else if t < 1.0 {
            1.0 + 4.0 * (t - 0.5)
        } else if t < 1.5 {
            3.0 - 2.0 * (t - 1.0)
        } else {
            2.0
        }
    });
    // Away from the grid points at which a comparison changes.
    for (t, row) in r.t.iter().zip(&r.rows) {
        let u = t - 1.0;
        let near = |x: f64| (u - x).abs() < 1e-9;
        if near(0.0) || near(0.1) || near(-0.1) {
            continue;
        }
        let ratio = 0.5
            / if u.abs() < 0.1 {
                0.1_f64.copysign(u)
            } else {
                u
            };
        assert!((row[3] - ratio).abs() < 1e-9, "ratio at u = {u}");
        assert_eq!(row[12], u.signum(), "sign at u = {u}");
        // positive: u > band, which is u > 0 inside the band and above it.
        let chosen = if u > 0.0 { 0.5 } else { u };
        assert!((row[13] - chosen).abs() < 1e-12, "chosen at u = {u}");
    }
}

/// A balanced set of phase signals of amplitude 1 whose angle is the signal
/// `angle`: the blocks, and the names of the three signals.
fn balanced(angle: &str) -> Value {
    json!([
        {"type": "constant", "name": "one", "value": 1.0},
        {"type": "constant", "name": "nothing", "value": 0.0},
        {"type": "dq0_to_abc", "name": "x", "d": "one", "q": "nothing", "angle": angle},
    ])
}

fn with(mut list: Value, more: Value) -> Value {
    list.as_array_mut()
        .unwrap()
        .extend(more.as_array().unwrap().iter().cloned());
    list
}

/// The difference of two angles, folded into (-pi, pi].
fn angle_difference(a: f64, b: f64) -> f64 {
    let d = (a - b).rem_euclid(2.0 * PI);
    if d > PI {
        d - 2.0 * PI
    } else {
        d
    }
}

/// Kp and Ki of a loop with natural frequency `wn` and damping `zeta`: the
/// error of the angle follows s^2 / (s^2 + Kp s + Ki).
fn pll_gains(wn: f64, zeta: f64) -> (f64, f64) {
    (2.0 * zeta * wn, wn * wn)
}

fn pll_block(kp: f64, ki: f64, initial_angle: Option<f64>) -> Value {
    let mut pll = json!({"type": "pll", "name": "pll", "inputs": ["x.a", "x.b", "x.c"],
        "nominal_frequency_hz": 50.0, "rated_amplitude": 1.0,
        "proportional_gain": kp, "integral_gain_per_s": ki});
    if let Some(a) = initial_angle {
        pll["initial_angle_rad"] = json!(a);
    }
    json!([pll])
}

#[test]
fn a_pll_answers_a_step_of_the_phase_like_its_linear_model() {
    // The inputs are 0.02 rad ahead of the loop at t = 0. With the error
    // e = angle of the inputs - theta, the loop is e'' + Kp sin(e)' + Ki
    // sin(e) = 0; for a small error, e = delta E(t) with
    // E = e^(-sigma t) (cos(wd t) - sigma/wd sin(wd t)), sigma = zeta wn.
    // The loop shows q = sin(e) and omega = omega0 - e'.
    //
    // Tolerance: sin(e) differs from e by at most delta^3/6, and that
    // difference drives the loop with gains of the order of its own, so the
    // response differs by a few times delta^3/6: delta^3/2 = 4e-6 is allowed
    // (2e-4 of the step). The trapezoidal rule adds delta (wn h)^2 / 12 =
    // 7e-8.
    let (wn, zeta, delta) = (2.0 * PI * 20.0, 0.7, 0.02_f64);
    let (kp, ki) = pll_gains(wn, zeta);
    let (sigma, wd) = (zeta * wn, wn * (1.0 - zeta * zeta).sqrt());
    let w0 = 2.0 * PI * 50.0;
    let controls = with(
        with(
            json!([{"type": "ramp", "name": "angle", "slope_per_s": w0, "initial": delta}]),
            balanced("angle"),
        ),
        pll_block(kp, ki, Some(0.0)),
    );
    let r = run(&blocks(
        5e-5,
        0.1,
        controls,
        &["pll.q", "pll.frequency_hz", "pll.theta", "pll.d", "angle"],
    ));
    let tolerance = delta.powi(3) / 2.0;
    let e = r.error("pll.q", |t| delta * damped(t, sigma, wd, -sigma / wd));
    assert!(e < tolerance, "q: {e:e}");
    // omega - omega0 = -e' = delta e^(-sigma t) (2 sigma cos(wd t)
    // + (wd - sigma^2/wd) sin(wd t)), which starts at Kp delta.
    let e = r.error("pll.frequency_hz", |t| {
        50.0 + delta * 2.0 * sigma * damped(t, sigma, wd, (wd - sigma * sigma / wd) / (2.0 * sigma))
            / (2.0 * PI)
    });
    assert!(e < kp * tolerance / (2.0 * PI), "frequency: {e:e}");
    // theta stays in [0, 2 pi) and ends locked: on the angle of the inputs,
    // with d their amplitude.
    let (theta, angle) = (r.column("pll.theta"), r.column("angle"));
    assert!(theta.iter().all(|v| (0.0..2.0 * PI).contains(v)));
    let last = theta.len() - 1;
    // e^(-sigma t) is 1.5e-4 at the end.
    assert!(angle_difference(theta[last], angle[last]).abs() < 2.0 * (-sigma * 0.1).exp() * delta);
    assert!((r.column("pll.d")[last] - 1.0).abs() < 1e-9);
}

#[test]
fn a_pll_answers_a_step_of_the_frequency_like_its_linear_model() {
    // The loop starts locked (no initial angle given) on inputs that are
    // 0.5 Hz above its nominal frequency: e'' + Kp e' + Ki e = 0 with
    // e(0) = 0 and e'(0) = dw, so e = dw/wd e^(-sigma t) sin(wd t) and
    // omega = omega0 + dw (1 - E(t)), E as for the step of the phase.
    //
    // Tolerance as there, with the largest error e_max = dw/wn e^(-...) <
    // dw/wn = 0.025 rad in place of delta.
    let (wn, zeta, dw) = (2.0 * PI * 20.0, 0.7, 2.0 * PI * 0.5);
    let (kp, ki) = pll_gains(wn, zeta);
    let (sigma, wd) = (zeta * wn, wn * (1.0 - zeta * zeta).sqrt());
    let w0 = 2.0 * PI * 50.0;
    let controls = with(
        with(
            json!([{"type": "ramp", "name": "angle", "slope_per_s": w0 + dw, "initial": 0.3}]),
            balanced("angle"),
        ),
        pll_block(kp, ki, None),
    );
    let r = run(&blocks(
        5e-5,
        0.1,
        controls,
        &["pll.q", "pll.frequency_hz", "pll.theta"],
    ));
    assert!((r.rows[0][2] - 0.3).abs() < 1e-15);
    assert!(r.rows[0][0].abs() < 1e-15);
    let tolerance = (dw / wn).powi(3) / 2.0;
    let e = r.error("pll.q", |t| dw / wd * (-sigma * t).exp() * (wd * t).sin());
    assert!(e < tolerance, "q: {e:e}");
    let e = r.error("pll.frequency_hz", |t| {
        50.0 + dw * (1.0 - damped(t, sigma, wd, -sigma / wd)) / (2.0 * PI)
    });
    assert!(e < kp * tolerance / (2.0 * PI), "frequency: {e:e}");
}

#[test]
fn a_pll_built_from_blocks_equals_the_pll_block() {
    // abc_to_dq0 with the loop's own angle, a PI controller on q, the
    // nominal frequency added, and an integrator folded into [0, 2 pi): a
    // loop through the integrator, which is iterated at every grid point.
    let (kp, ki) = pll_gains(2.0 * PI * 20.0, 0.7);
    let w0 = 2.0 * PI * 50.0;
    let parts = json!([
        {"type": "abc_to_dq0", "name": "v", "inputs": ["x.a", "x.b", "x.c"], "angle": "theta"},
        {"type": "pi", "name": "dw", "input": "v.q", "proportional_gain": kp, "integral_gain_per_s": ki},
        {"type": "constant", "name": "w0", "value": w0},
        {"type": "sum", "name": "w", "inputs": ["w0", "dw"], "signs": [1.0, 1.0]},
        {"type": "integrator", "name": "theta", "input": "w", "minimum": 0.0, "maximum": 2.0 * PI, "wrap": true},
        {"type": "gain", "name": "f", "input": "w", "gain": 1.0 / (2.0 * PI)},
    ]);
    let controls = with(
        with(
            with(
                json!([{"type": "ramp", "name": "angle", "slope_per_s": w0 + 3.0, "initial": 0.5}]),
                balanced("angle"),
            ),
            pll_block(kp, ki, Some(0.0)),
        ),
        parts,
    );
    let r = run(&blocks(
        5e-5,
        0.1,
        controls,
        &[
            "pll.theta",
            "theta",
            "pll.frequency_hz",
            "f",
            "pll.q",
            "v.q",
        ],
    ));
    for row in &r.rows {
        // The loop of blocks is iterated to 1e-12 of its largest signal
        // (omega, 320 rad/s).
        assert!(angle_difference(row[0], row[1]).abs() < 1e-11, "{row:?}");
        assert!((row[2] - row[3]).abs() < 1e-9, "{row:?}");
        assert!((row[4] - row[5]).abs() < 1e-11, "{row:?}");
    }
    assert!(r.stats.control_loop_iterations > r.rows.len() as u64);
}

#[test]
fn a_loop_through_blocks_with_a_state_is_of_second_order() {
    // r -> (+) -> K/(1 + s T) -> 1/s -> y, with y fed back: the closed loop
    // is wn^2/(s^2 + 2 zeta wn s + wn^2) with wn^2 = K/T and
    // 2 zeta wn = 1/T. The loop is iterated at every grid point until the
    // blocks agree, which is the trapezoidal rule for the whole loop: the
    // error is that of the transfer function of the same poles,
    // (wn h)^2 / (12 e zeta sqrt(1 - zeta^2)) up to a small factor, and a
    // quarter of it with half the step.
    let (k, tau): (f64, f64) = (2500.0, 0.01);
    let wn = (k / tau).sqrt();
    let zeta = 1.0 / (2.0 * tau * wn);
    let root = (1.0 - zeta * zeta).sqrt();
    let model = |h: f64| {
        blocks(
            h,
            0.1,
            json!([
                {"type": "integrator", "name": "y", "input": "x"},
                {"type": "lag", "name": "x", "input": "e", "gain": k, "time_constant_s": tau, "initial_input": 0.0},
                {"type": "sum", "name": "e", "inputs": ["r", "y"], "signs": [1.0, -1.0]},
                {"type": "constant", "name": "r", "value": 1.0},
            ]),
            &["y"],
        )
    };
    let exact = |t: f64| 1.0 - damped(t, zeta * wn, wn * root, zeta / root);
    let h: f64 = 1e-4;
    let bound = (wn * h).powi(2) / (12.0 * 1.0_f64.exp() * zeta * root);
    let errors = errors_at(h, |h| run(&model(h)).error("y", exact));
    assert!(errors.0 < 1.5 * bound, "{errors:?} against {bound:e}");
    assert_second_order("loop", errors);
    // Each pass over the loop multiplies the error by (h/2T) K h/2 = 6e-4:
    // about three passes per grid point after the first.
    let r = run(&model(h));
    let per_point = r.stats.control_loop_iterations as f64 / r.stats.steps as f64;
    assert!((2.0..4.0).contains(&per_point), "{per_point}");
}

/// The codes of the errors of a model.
fn error_codes(model: &Value) -> Vec<&'static str> {
    diagnostics(model)
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| d.code)
        .collect()
}

#[test]
fn loops_and_names_of_signals_are_checked() {
    let model = |controls: Value| blocks(1e-3, 0.01, controls, &[]);
    // A loop of blocks that all depend on their present input.
    let direct = model(json!([
        {"type": "sum", "name": "e", "inputs": ["y", "y"], "signs": [1.0, -1.0]},
        {"type": "limiter", "name": "y", "input": "e", "maximum": 1.0},
    ]));
    let d = diagnostics(&direct);
    assert_eq!(d[0].code, "E171");
    assert!(d[0].message.contains("e -> y"), "{}", d[0].message);
    assert!(d[0].hint.contains("integrator"));
    // A delay closes a loop from one time step on.
    let delayed = |delay_s: f64| {
        model(json!([
            {"type": "gain", "name": "e", "input": "y", "gain": 0.5},
            {"type": "delay", "name": "y", "input": "e", "delay_s": delay_s, "initial_output": 1.0},
        ]))
    };
    assert_eq!(error_codes(&delayed(0.5e-3)), ["E171"]);
    assert!(error_codes(&delayed(1e-3)).is_empty());
    // A lead-lag and a proper transfer function depend on their input.
    let lead = model(json!([
        {"type": "gain", "name": "e", "input": "y", "gain": 0.5},
        {"type": "lead_lag", "name": "y", "input": "e", "lead_time_constant_s": 0.1, "lag_time_constant_s": 0.2},
    ]));
    assert_eq!(error_codes(&lead), ["E171"]);
    // A block named like an output of another.
    let twice = model(json!([
        {"type": "constant", "name": "k", "value": 1.0},
        {"type": "abc_to_dq0", "name": "v", "inputs": ["k", "k", "k"], "angle": "k"},
        {"type": "constant", "name": "v.q", "value": 1.0},
    ]));
    let d = diagnostics(&twice);
    assert_eq!(
        (d[0].code, d[0].path.as_str()),
        ("E240", "controls[2].name")
    );
    // The name of a block with several outputs is not a signal.
    let whole = model(json!([
        {"type": "constant", "name": "k", "value": 1.0},
        {"type": "abc_to_dq0", "name": "v", "inputs": ["k", "k", "k"], "angle": "k"},
        {"type": "gain", "name": "g", "input": "v", "gain": 1.0},
    ]));
    let d = diagnostics(&whole);
    assert_eq!(
        (d[0].code, d[0].path.as_str()),
        ("E170", "controls[2].input")
    );
    assert!(d[0].hint.contains("v.d, v.q, v.zero"), "{}", d[0].hint);
    // A model of version 0.4 does not have the blocks of 0.5.
    let mut old = model(json!([{"type": "ramp", "name": "u", "slope_per_s": 1.0}]));
    old["version"] = json!("0.4");
    let d = diagnostics(&old);
    assert_eq!(d[0].code, "E102");
    assert!(d[0].hint.contains("\"version\": \"0.5\""), "{}", d[0].hint);
}

#[test]
fn the_parameters_of_the_blocks_are_checked() {
    let code = |block: Value| {
        let d = diagnostics(&blocks(
            1e-3,
            0.01,
            json!([{"type": "constant", "name": "u", "value": 1.0}, block]),
            &[],
        ));
        let errors: Vec<_> = d.iter().filter(|d| d.severity == Severity::Error).collect();
        assert_eq!(errors.len(), 1, "{d:#?}");
        assert!(!errors[0].hint.is_empty());
        (errors[0].code, errors[0].path.clone())
    };
    let tf = |numerator: Value, denominator: Value| json!({"type": "transfer_function", "name": "y", "input": "u", "numerator": numerator, "denominator": denominator});
    assert_eq!(
        code(tf(json!([1.0, 0.0, 1.0]), json!([1.0, 1.0]))).0,
        "E241"
    );
    assert_eq!(
        code(tf(json!([1.0]), json!([0.0, 1.0]))).1,
        "controls[1].denominator[0]"
    );
    assert_eq!(code(tf(json!([]), json!([1.0]))).1, "controls[1].numerator");
    let mut pole = tf(json!([1.0]), json!([1.0, 0.0]));
    pole["initial_input"] = json!(1.0);
    assert_eq!(
        code(pole),
        ("E241", "controls[1].initial_input".to_string())
    );
    assert_eq!(
        code(
            json!({"type": "time_series", "name": "y", "times_s": [0.0, 1.0, 1.0], "values": [1.0, 2.0, 3.0]})
        ),
        ("E242", "controls[1].times_s[2]".to_string())
    );
    assert_eq!(
        code(json!({"type": "time_series", "name": "y", "times_s": [0.0, 1.0], "values": [1.0]})).0,
        "E242"
    );
    assert_eq!(
        code(json!({"type": "lag", "name": "y", "input": "u", "time_constant_s": 0.0})),
        ("E130", "controls[1].time_constant_s".to_string())
    );
    assert_eq!(
        code(
            json!({"type": "integrator", "name": "y", "input": "u", "minimum": 0.0, "wrap": true})
        ),
        ("E132", "controls[1].wrap".to_string())
    );
    assert_eq!(
        code(json!({"type": "limiter", "name": "y", "input": "u", "minimum": 1.0, "maximum": 0.0})),
        ("E132", "controls[1].maximum".to_string())
    );
    assert_eq!(
        code(json!({"type": "rate_limiter", "name": "y", "input": "u", "rising_rate_per_s": 1.0, "falling_rate_per_s": -1.0})).0,
        "E130"
    );
    assert_eq!(
        code(json!({"type": "function", "name": "y", "function": "atan2", "inputs": ["u"]})),
        ("E172", "controls[1].inputs".to_string())
    );
    assert_eq!(
        code(json!({"type": "delay", "name": "y", "input": "u", "delay_s": -1.0})).0,
        "E130"
    );
    assert_eq!(
        code(
            json!({"type": "pll", "name": "y", "inputs": ["u", "u", "u"], "nominal_frequency_hz": 50.0,
            "rated_amplitude": 0.0, "proportional_gain": 1.0, "integral_gain_per_s": 1.0})
        ),
        ("E130", "controls[1].rated_amplitude".to_string())
    );
}

#[test]
fn a_run_stops_when_a_loop_does_not_settle_or_a_signal_is_not_finite() {
    // x' = -30000 x with h = 1e-3: each pass over the loop multiplies the
    // difference by 15.
    let stiff = blocks(
        1e-3,
        0.01,
        json!([
            {"type": "gain", "name": "e", "input": "x", "gain": -30000.0},
            {"type": "integrator", "name": "x", "input": "e", "initial_value": 1.0},
        ]),
        &["x"],
    );
    let err = try_run(&stiff).err().expect("the loop does not settle");
    let d = err.to_diagnostic();
    assert_eq!((d.code, d.path.as_str()), ("E250", "x, e"));
    assert!(d.message.contains("t = 1e-3 s"), "{}", d.message);
    assert!(d.hint.contains("time_step_s"));
    // A division by a signal that passes through 0.
    let divided = |guard: Option<f64>| {
        let mut q = json!({"type": "quotient", "name": "q", "inputs": ["one", "u"]});
        if let Some(g) = guard {
            q["minimum_denominator"] = json!(g);
        }
        blocks(
            1e-3,
            0.01,
            json!([
                {"type": "constant", "name": "one", "value": 1.0},
                {"type": "ramp", "name": "u", "slope_per_s": 1.0, "initial": -0.005},
                q,
            ]),
            &["q"],
        )
    };
    let err = try_run(&divided(None)).err().expect("division by 0");
    let d = err.to_diagnostic();
    assert_eq!((d.code, d.path.as_str()), ("E251", "q"));
    assert!(d.message.contains("quotient"), "{}", d.message);
    assert!(d.hint.contains("minimum_denominator"));
    let q = run(&divided(Some(1e-3))).column("q");
    assert_eq!(q[4], -1000.0);
    assert_eq!(q[5], 1000.0);
    // A NaN inside an algebraic loop must end the loop immediately with E251
    // rather than iterating to E250 when another block in the loop keeps
    // changing (a comparator after `bad` turns NaN back into a finite output,
    // so `change` stays finite and only the direct finiteness check catches it).
    let nan_loop = blocks(
        1e-3,
        0.01,
        json!([
            {"type": "constant", "name": "minus_two", "value": -2.0},
            {"type": "gain", "name": "g", "input": "x", "gain": -30000.0},
            {"type": "sum", "name": "neg_in", "inputs": ["x", "minus_two"], "signs": [1.0, 1.0]},
            {"type": "function", "name": "bad", "inputs": ["neg_in"], "function": "sqrt"},
            {"type": "comparator", "name": "cmp", "inputs": ["bad", "minus_two"]},
            {"type": "sum", "name": "sum_in", "inputs": ["g", "cmp"], "signs": [1.0, 1.0]},
            {"type": "integrator", "name": "x", "input": "sum_in", "initial_value": 1.0},
        ]),
        &["x"],
    );
    let err = try_run(&nan_loop).err().expect("NaN in loop");
    let d = err.to_diagnostic();
    assert_eq!((d.code, d.path.as_str()), ("E251", "bad"), "{d:#?}");
    assert!(!d.message.contains("  "), "{}", d.message);
    assert!(!d.hint.contains("  "), "{}", d.hint);
}

/// A controlled voltage source `E1` that drives R and L in series; the
/// signal of the source is "u".
fn driven_rl(method: &str, h: f64, end: f64, r: f64, l: f64, controls: Value) -> Value {
    json!({
        "format": "openkeito-model",
        "version": "0.5",
        "simulation": {"time_step_s": h, "end_time_s": end, "method": method},
        "components": [
            {"type": "controlled_voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "signal": "u"},
            {"type": "resistor", "name": "R1", "nodes": ["n1", "n2"], "resistance_ohm": r},
            {"type": "inductor", "name": "L1", "nodes": ["n2", "gnd"], "inductance_h": l},
        ],
        "controls": controls,
        "outputs": [
            {"type": "branch_current", "component": "L1"},
            {"type": "node_voltage", "node": "n1"},
            {"type": "signal", "signal": "u"},
        ],
    })
}

const METHODS: [&str; 2] = ["2s-dirk", "trapezoidal"];

#[test]
fn a_controlled_source_follows_a_step_one_time_step_late() {
    // The signal steps from 0 to V at the grid point t0. The source follows
    // one step late and in a straight line: 0 until t0, V (t - t0)/h until
    // t0 + h, V from there on. The current of R and L in series is then
    //   i = V/(R h) (s - tau (1 - e^(-s/tau)))             for s = t - t0 <= h,
    //   i = V/R (1 - tau/h (e^(h/tau) - 1) e^(-s/tau))     for s >= h.
    // The source is continuous with a kink at each end of the ramp, which
    // both methods integrate to second order.
    let (r, l, v, t0) = (1.0, 0.01, 10.0, 2e-3);
    let tau = l / r;
    let model = |method: &str, h: f64| {
        driven_rl(
            method,
            h,
            0.03,
            r,
            l,
            json!([{"type": "step", "name": "u", "time_s": t0, "after": v}]),
        )
    };
    for method in METHODS {
        let error = |h: f64| {
            let run = run(&model(method, h));
            // The source itself: exact at the grid points.
            let e = run.error("V(n1)", |t| v * ((t - t0) / h).clamp(0.0, 1.0));
            assert!(e < 1e-12, "{method}: source {e:e}");
            run.error("I(L1)", |t| {
                let s = t - t0;
                if s <= 0.0 {
                    0.0
                } else if s <= h {
                    v / (r * h) * (s - tau * (1.0 - (-s / tau).exp()))
                } else {
                    v / r * (1.0 - tau / h * ((h / tau).exp() - 1.0) * (-s / tau).exp())
                }
            })
        };
        let errors = errors_at(1e-4, error);
        // (h/tau)^2 / 12 of V/R for the trapezoidal rule, less for 2S-DIRK.
        assert!(errors.0 < 1e-4, "{method}: {errors:?}");
        assert_second_order(method, errors);
    }
}

#[test]
fn a_controlled_source_follows_a_sine_one_time_step_late() {
    // The signal A sin(w t) reaches the circuit as A sin(w (t - h)) from
    // t = h on (0 before: the signal at t = 0), joined by straight lines
    // between grid points. With s = t - h, the current of R and L in series
    // is i = A/|Z| (sin(w s - phi) + sin(phi) e^(-s/tau)).
    //
    // The chords of the sine are off by at most (w h)^2/8 of A between grid
    // points and exact at them, so the error stays of second order: the
    // trapezoidal rule, which looks at grid points only, is off by
    // (w h)^2/12 of the amplitude A/|Z| of the current, and the first stage
    // of 2S-DIRK, between grid points, sees the chord: at most (w h)^2/8.
    let (r, l, a, f): (f64, f64, f64, f64) = (1.0, 0.01, 10.0, 50.0);
    let w = 2.0 * PI * f;
    let (z, phi, tau) = (r.hypot(w * l), (w * l / r).atan(), l / r);
    let model = |method: &str, h: f64| {
        driven_rl(
            method,
            h,
            0.04,
            r,
            l,
            json!([{"type": "sine_wave", "name": "u", "amplitude": a, "frequency_hz": f}]),
        )
    };
    for method in METHODS {
        let error = |h: f64| {
            run(&model(method, h)).error("I(L1)", |t| {
                let s = (t - h).max(0.0);
                a / z * ((w * s - phi).sin() + phi.sin() * (-s / tau).exp())
            })
        };
        let errors = errors_at(1e-4, error);
        let bound = (w * 1e-4).powi(2) / 8.0 * a / z;
        assert!(errors.0 < bound, "{method}: {errors:?} against {bound:e}");
        assert_second_order(method, errors);
        // Against the signal without the delay the difference is of first
        // order: w h of the amplitude.
        let undelayed = run(&model(method, 1e-4)).error_from("I(L1)", 0.03, |t| {
            a / z * ((w * t - phi).sin() + phi.sin() * (-t / tau).exp())
        });
        assert!(
            undelayed > 0.8 * w * 1e-4 * a / z,
            "{method}: {undelayed:e}"
        );
    }
}

/// The current of R and L in series behind a voltage v(t - h), where
/// v = Kp (reference - i) + Ki integral of (reference - i) dt and v(t) = v(0) for
/// t < 0: integrated here with the classical Runge-Kutta method and `fine`
/// steps per h, with the delayed voltage taken from its own history.
/// Returns the current at the multiples of h.
fn delayed_current_control(h: f64, steps: usize, r: f64, l: f64, kp: f64, ki: f64) -> Vec<f64> {
    let fine = 64;
    let dt = h / fine as f64;
    let reference = 1.0;
    let (mut i, mut x) = (0.0_f64, 0.0_f64);
    let mut v = vec![kp * (reference - i) + x];
    let mut at_grid = vec![i];
    // v(t - h) at the fine point j and half a fine step after it.
    let delayed = |v: &[f64], j: usize| {
        let at = |k: isize| v[k.max(0) as usize];
        let k = j as isize - fine as isize;
        (at(k), 0.5 * (at(k) + at(k + 1)))
    };
    for j in 0..steps * fine {
        let (w0, w_half) = delayed(&v, j);
        let (w1, _) = delayed(&v, j + 1);
        let f = |i: f64, w: f64| ((w - r * i) / l, ki * (reference - i));
        let (k1i, k1x) = f(i, w0);
        let (k2i, k2x) = f(i + 0.5 * dt * k1i, w_half);
        let (k3i, k3x) = f(i + 0.5 * dt * k2i, w_half);
        let (k4i, k4x) = f(i + dt * k3i, w1);
        i += dt / 6.0 * (k1i + 2.0 * k2i + 2.0 * k3i + k4i);
        x += dt / 6.0 * (k1x + 2.0 * k2x + 2.0 * k3x + k4x);
        v.push(kp * (reference - i) + x);
        if (j + 1) % fine == 0 {
            at_grid.push(i);
        }
    }
    at_grid
}

#[test]
fn a_current_loop_through_the_circuit_answers_with_its_delay_of_one_step() {
    // Circuit -> measurement -> PI -> controlled source: the current of R
    // and L in series is controlled to 1 A. With Kp = L wc and Ki = R wc the
    // zero of the controller cancels the pole of the circuit, and without
    // any delay the current would be 1 - e^(-wc t). The source follows the
    // controller one step late, so the loop is the delay equation
    // L i' + R i = v(t - h): solved independently above.
    //
    // The run differs from that solution by the trapezoidal rule of the PI
    // block, the method of the circuit and the straight lines between the
    // values of v, all of second order; from the response without a delay
    // it differs by about wc h of the reference, which is of first order.
    let (r, l, wc) = (1.0, 0.01, 500.0);
    let (kp, ki) = (l * wc, r * wc);
    let model = |method: &str, h: f64| {
        driven_rl(
            method,
            h,
            0.02,
            r,
            l,
            json!([
                {"type": "measurement", "name": "i", "quantity": {"type": "branch_current", "component": "L1"}},
                {"type": "constant", "name": "ref", "value": 1.0},
                {"type": "sum", "name": "e", "inputs": ["ref", "i"], "signs": [1.0, -1.0]},
                {"type": "pi", "name": "u", "input": "e", "proportional_gain": kp, "integral_gain_per_s": ki},
            ]),
        )
    };
    for method in METHODS {
        let error = |h: f64| {
            let run = run(&model(method, h));
            let exact = delayed_current_control(h, run.rows.len() - 1, r, l, kp, ki);
            run.column("I(L1)")
                .iter()
                .zip(&exact)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0, f64::max)
        };
        let h = 1e-4;
        let errors = errors_at(h, error);
        // (wc h)^2 = 2.5e-3: the error is a small part of it.
        assert!(errors.0 < 0.1 * (wc * h).powi(2), "{method}: {errors:?}");
        let ratio = errors.0 / errors.1;
        assert!((3.5..4.5).contains(&ratio), "{method}: {errors:?}");
        let without_delay = run(&model(method, h)).error("I(L1)", |t| 1.0 - (-wc * t).exp());
        assert!(
            without_delay > 0.2 * wc * h && without_delay > 50.0 * errors.0,
            "{method}: {without_delay:e}"
        );
    }
}

/// A 66 kV source, a pi line, a YNd11 transformer and a load.
fn network(controls: Value, outputs: Value) -> Value {
    json!({
        "format": "openkeito-model",
        "version": "0.5",
        "simulation": {"time_step_s": 5e-5, "end_time_s": 0.04, "initialization": "steady_state"},
        "components": [
            {"type": "three_phase_source", "name": "G1", "bus": "b1", "line_voltage_rms_v": 66000.0, "frequency_hz": 50.0, "angle_deg": 25.0,
             "impedance": {"type": "sequence", "r1_ohm": 0.5, "l1_h": 0.0159, "r0_ohm": 1.0, "l0_h": 0.0477}},
            {"type": "pi_line", "name": "LINE1", "buses": ["b1", "b2"], "length_km": 50.0, "sections": 2,
             "per_km": {"type": "sequence", "r1_ohm_per_km": 0.03, "l1_h_per_km": 0.0009, "c1_f_per_km": 1.25e-8,
                        "r0_ohm_per_km": 0.2, "l0_h_per_km": 0.003, "c0_f_per_km": 6.0e-9}},
            {"type": "three_phase_transformer", "name": "T1", "buses": ["b2", "b3"], "vector_group": "YNd11",
             "rated_voltage_kv": [66.0, 6.6], "rated_power_mva": 30.0, "rated_frequency_hz": 50.0,
             "impedance_percent": 10.0, "resistance_percent": 0.5},
            {"type": "three_phase_load", "name": "LD1", "bus": "b3", "rated_voltage_kv": 6.6, "rated_frequency_hz": 50.0,
             "active_power_mw": 5.0, "reactive_power_mvar": 1.5},
            {"type": "fault", "name": "F1", "bus": "b2", "phases": ["a"], "ground": true, "start_time_s": 0.02, "clear_time_s": 0.03},
        ],
        "controls": controls,
        "outputs": outputs,
    })
}

#[test]
fn measurements_of_three_phase_components_equal_their_columns() {
    let model = network(
        json!([
            {"type": "measurement", "name": "line", "quantity": {"type": "branch_current", "component": "LINE1", "terminal": 0, "phase": "a"}},
            {"type": "measurement", "name": "trafo", "quantity": {"type": "branch_current", "component": "T1", "terminal": 1, "phase": "c"}},
            {"type": "measurement", "name": "source", "quantity": {"type": "branch_current", "component": "G1", "phase": "b"}},
            {"type": "measurement", "name": "ground", "quantity": {"type": "branch_current", "component": "F1", "phase": "g"}},
            {"type": "measurement", "name": "u_load", "quantity": {"type": "branch_voltage", "component": "LD1", "phase": "b"}},
            {"type": "three_phase_measurement", "name": "i", "quantity": {"type": "branch_current", "component": "LINE1", "terminal": 1}},
            {"type": "three_phase_measurement", "name": "it", "quantity": {"type": "branch_current", "component": "T1", "terminal": 0}},
            {"type": "three_phase_measurement", "name": "il", "quantity": {"type": "branch_current", "component": "LD1"}},
            {"type": "three_phase_measurement", "name": "v", "quantity": {"type": "bus_voltage", "bus": "b2"}},
        ]),
        json!([
            {"type": "branch_current", "component": "LINE1"},
            {"type": "branch_current", "component": "T1"},
            {"type": "branch_current", "component": "G1"},
            {"type": "branch_current", "component": "F1"},
            {"type": "branch_current", "component": "LD1"},
            {"type": "branch_voltage", "component": "LD1"},
            {"type": "bus_voltage", "bus": "b2"},
            {"type": "signal", "signal": "line"}, {"type": "signal", "signal": "trafo"},
            {"type": "signal", "signal": "source"}, {"type": "signal", "signal": "ground"},
            {"type": "signal", "signal": "u_load"},
            {"type": "signal", "signal": "i.a"}, {"type": "signal", "signal": "i.b"}, {"type": "signal", "signal": "i.c"},
            {"type": "signal", "signal": "it.a"}, {"type": "signal", "signal": "it.b"}, {"type": "signal", "signal": "it.c"},
            {"type": "signal", "signal": "il.a"}, {"type": "signal", "signal": "il.b"}, {"type": "signal", "signal": "il.c"},
            {"type": "signal", "signal": "v.a"}, {"type": "signal", "signal": "v.b"}, {"type": "signal", "signal": "v.c"},
        ]),
    );
    let r = run(&model);
    // The fault makes the phases differ and gives a current to ground.
    assert_eq!(r.stats.events.len(), 2);
    for (signal, column) in [
        ("S(line)", "I(LINE1.0.a)"),
        ("S(trafo)", "I(T1.1.c)"),
        ("S(source)", "I(G1.b)"),
        ("S(ground)", "I(F1.g)"),
        ("S(u_load)", "U(LD1.b)"),
        ("S(i.a)", "I(LINE1.1.a)"),
        ("S(i.b)", "I(LINE1.1.b)"),
        ("S(i.c)", "I(LINE1.1.c)"),
        ("S(it.a)", "I(T1.0.a)"),
        ("S(it.b)", "I(T1.0.b)"),
        ("S(it.c)", "I(T1.0.c)"),
        ("S(il.a)", "I(LD1.a)"),
        ("S(il.b)", "I(LD1.b)"),
        ("S(il.c)", "I(LD1.c)"),
        ("S(v.a)", "V(b2.a)"),
        ("S(v.b)", "V(b2.b)"),
        ("S(v.c)", "V(b2.c)"),
    ] {
        let (s, c) = (r.column(signal), r.column(column));
        // The same terms of the same solution. The first row is solved
        // twice when a controlled source reads a signal; not here.
        assert_eq!(s, c, "{signal} and {column}");
        assert!(c.iter().any(|v| v.abs() > 1.0), "{column} is not zero");
    }
}

#[test]
fn a_choice_that_names_no_quantity_is_refused_with_the_quantities_there_are() {
    let measured = |quantity: Value| {
        let d = diagnostics(&network(
            json!([{"type": "measurement", "name": "m", "quantity": quantity}]),
            json!([{"type": "signal", "signal": "m"}]),
        ));
        let errors: Vec<_> = d
            .into_iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert_eq!(errors.len(), 1, "{errors:#?}");
        errors.into_iter().next().unwrap()
    };
    // A line has two terminals.
    let d = measured(json!({"type": "branch_current", "component": "LINE1", "phase": "a"}));
    assert_eq!((d.code, d.path.as_str()), ("E173", "controls[0].quantity"));
    assert!(d.message.contains("I(LINE1.a)"), "{}", d.message);
    assert!(
        d.hint.contains("I(LINE1.0.a)") && d.hint.contains("I(LINE1.1.c)"),
        "{}",
        d.hint
    );
    // Neither terminal nor phase.
    let d = measured(json!({"type": "branch_current", "component": "LINE1"}));
    assert_eq!(
        (d.code, d.path.as_str()),
        ("E173", "controls[0].quantity.component")
    );
    assert!(d.hint.contains("\"terminal\""), "{}", d.hint);
    // A transformer has no branch voltages.
    let d = measured(json!({"type": "branch_voltage", "component": "T1", "phase": "a"}));
    assert_eq!(d.code, "E173");
    assert!(d.hint.contains("node_voltage"), "{}", d.hint);
    // A source offers no signals.
    let d = measured(json!({"type": "component_signal", "component": "G1", "signal": "speed"}));
    assert_eq!(
        (d.code, d.path.as_str()),
        ("E244", "controls[0].quantity.signal")
    );
    // The three phases of a component with one current.
    let d = diagnostics(&driven_rl(
        "2s-dirk",
        1e-4,
        1e-3,
        1.0,
        0.01,
        json!([
            {"type": "constant", "name": "u", "value": 1.0},
            {"type": "three_phase_measurement", "name": "m", "quantity": {"type": "branch_current", "component": "L1"}},
        ]),
    ));
    assert_eq!(d[0].code, "E173");
}

#[test]
fn dq0_of_a_bus_voltage_is_its_phasor_and_the_way_back_gives_the_voltage() {
    // An ideal source of 6.6 kV at 25 degrees: the phasor of phase a is
    // V = 6600/sqrt(3) at 25 degrees (rms, cosine reference). With
    // theta = w t + theta0, d + j q = sqrt(2) V e^(-j theta0), constant.
    let (h, theta0) = (5e-5, 0.3);
    let w = 2.0 * PI * 50.0;
    let peak = (2.0_f64 / 3.0).sqrt() * 6600.0;
    let r = run(&json!({
        "format": "openkeito-model",
        "version": "0.5",
        "simulation": {"time_step_s": h, "end_time_s": 0.04},
        "components": [
            {"type": "three_phase_source", "name": "G1", "bus": "b1", "line_voltage_rms_v": 6600.0, "frequency_hz": 50.0, "angle_deg": 25.0},
            {"type": "three_phase_load", "name": "LD1", "bus": "b1", "rated_voltage_kv": 6.6, "rated_frequency_hz": 50.0,
             "active_power_mw": 1.0, "reactive_power_mvar": 0.0},
        ],
        "controls": [
            {"type": "three_phase_measurement", "name": "v", "quantity": {"type": "bus_voltage", "bus": "b1"}},
            {"type": "ramp", "name": "theta", "slope_per_s": w, "initial": theta0},
            {"type": "abc_to_dq0", "name": "dq", "inputs": ["v.a", "v.b", "v.c"], "angle": "theta"},
            {"type": "dq0_to_abc", "name": "back", "d": "dq.d", "q": "dq.q", "zero": "dq.zero", "angle": "theta"},
        ],
        "outputs": [
            {"type": "bus_voltage", "bus": "b1"},
            {"type": "signal", "signal": "dq.d"}, {"type": "signal", "signal": "dq.q"}, {"type": "signal", "signal": "dq.zero"},
            {"type": "signal", "signal": "back.a"}, {"type": "signal", "signal": "back.b"}, {"type": "signal", "signal": "back.c"},
        ],
    }));
    let angle = 25.0_f64.to_radians() - theta0;
    // The first row is the solution at t = 0+, 1e-6 of a step after 0: the
    // voltage has turned by 1e-6 w h there, the angle of the block has not.
    for (k, row) in r.rows.iter().enumerate() {
        let tolerance = if k == 0 { 2e-6 * w * h } else { 1e-12 } * peak;
        assert!(
            (row[3] - peak * angle.cos()).abs() < tolerance,
            "d in row {k}"
        );
        assert!(
            (row[4] - peak * angle.sin()).abs() < tolerance,
            "q in row {k}"
        );
        assert!(row[5].abs() < 1e-12 * peak, "zero in row {k}");
        // The way back is the identity to rounding.
        for p in 0..3 {
            assert!(
                (row[6 + p] - row[p]).abs() < 1e-12 * peak,
                "phase {p} in row {k}"
            );
        }
    }
}

/// A source behind an impedance, a load, and a three-phase controlled
/// current source at the same bus. The controls hold the current that the
/// source has in the phasor solution: a PLL on the bus voltage, the current
/// at t = 0 in its frame as the set point, and the way back to the phases
/// with the angle `advance_steps` time steps ahead.
fn held_injection(h: f64, advance_steps: f64) -> Value {
    let w = 2.0 * PI * 50.0;
    let (kp, ki) = pll_gains(2.0 * PI * 20.0, 0.7);
    json!({
        "format": "openkeito-model",
        "version": "0.5",
        "simulation": {"time_step_s": h, "end_time_s": 0.1, "initialization": "steady_state"},
        "components": [
            {"type": "three_phase_source", "name": "G1", "bus": "b1", "line_voltage_rms_v": 6600.0, "frequency_hz": 50.0,
             "impedance": {"type": "sequence", "r1_ohm": 0.1, "l1_h": 0.002, "r0_ohm": 0.3, "l0_h": 0.006}},
            {"type": "three_phase_load", "name": "LD1", "bus": "b1", "rated_voltage_kv": 6.6, "rated_frequency_hz": 50.0,
             "active_power_mw": 2.0, "reactive_power_mvar": 0.5},
            {"type": "three_phase_controlled_current_source", "name": "INV1", "bus": "b1", "signals": ["i_ref.a", "i_ref.b", "i_ref.c"],
             "steady_state": {"current_rms_a": 200.0, "angle_deg": -15.0}},
        ],
        "controls": [
            {"type": "three_phase_measurement", "name": "v", "quantity": {"type": "bus_voltage", "bus": "b1"}},
            {"type": "three_phase_measurement", "name": "i", "quantity": {"type": "branch_current", "component": "INV1"}},
            {"type": "pll", "name": "pll", "inputs": ["v.a", "v.b", "v.c"], "nominal_frequency_hz": 50.0,
             "rated_amplitude": 5388.9, "proportional_gain": kp, "integral_gain_per_s": ki},
            {"type": "abc_to_dq0", "name": "i_dq", "inputs": ["i.a", "i.b", "i.c"], "angle": "pll.theta"},
            {"type": "initial_value", "name": "id_ref", "input": "i_dq.d"},
            {"type": "initial_value", "name": "iq_ref", "input": "i_dq.q"},
            {"type": "constant", "name": "advance", "value": advance_steps * w * h},
            {"type": "sum", "name": "theta_next", "inputs": ["pll.theta", "advance"], "signs": [1.0, 1.0]},
            {"type": "dq0_to_abc", "name": "i_ref", "d": "id_ref", "q": "iq_ref", "angle": "theta_next"},
        ],
        "outputs": [
            {"type": "bus_voltage", "bus": "b1"},
            {"type": "branch_current", "component": "INV1"},
            {"type": "branch_current", "component": "G1"},
            {"type": "signal", "signal": "pll.frequency_hz"},
            {"type": "signal", "signal": "pll.q"},
            {"type": "signal", "signal": "id_ref"},
        ],
    })
}

#[test]
fn a_run_with_controls_stays_in_its_steady_state() {
    // The source has the phasor 200 A at -15 degrees in the phasor solution.
    // The controls reproduce it from t = 0 on: the set point is the current
    // of t = 0 in the frame of the PLL, which starts locked, and the angle
    // of the way back is one step ahead, because the source follows its
    // signal one step late.
    //
    // What is left is the error of the method in the steady state (0.01
    // (w h)^2 of the amplitudes for this circuit with a plain current
    // source) and the chords of the controlled source between grid points,
    // which the first stage of 2S-DIRK sees: at most (w h)^2/8 of the
    // injected current, which is a little more than half of the largest
    // current here. Both are of second order, and neither grows with time.
    let h = 5e-5;
    let model = held_injection(h, 1.0);
    let warnings: Vec<_> = diagnostics(&model).iter().map(|d| d.code).collect();
    assert_eq!(warnings, ["W183"]);
    let r = run(&model);
    let report = r.stats.initialization.as_ref().expect("steady state");
    assert_eq!(report.checked_until_s, 0.1);
    let wh2 = (2.0 * PI * 50.0 * h).powi(2);
    let (dv, di) = (
        report.relative_voltage_deviation,
        report.relative_current_deviation.unwrap(),
    );
    assert!(dv < wh2 / 8.0 && di < wh2 / 8.0, "{dv:e}, {di:e}");
    let half = run(&held_injection(h / 2.0, 1.0));
    let report = half.stats.initialization.as_ref().unwrap();
    let ratio = dv / report.relative_voltage_deviation;
    assert!((3.5..4.5).contains(&ratio), "{ratio}");
    // The PLL stays locked at 50 Hz: with q of the bus voltage within dv of
    // its amplitude, the frequency is within Kp dv / 2 pi = 5e-4 Hz.
    let e = r.error("S(pll.frequency_hz)", |_| 50.0);
    assert!(e < 5e-4, "{e:e}");
    // The set point is the d current of the phasor: 200 sqrt(2) A times the
    // cosine of its angle against the bus voltage, a few degrees from 15.
    let id = r.column("S(id_ref)")[0];
    assert!((250.0..282.9).contains(&id), "{id}");

    // Without the step ahead the source lags by w h = 0.9 degrees: the run
    // leaves the steady state by about that part of the injected current.
    let lagging = run(&held_injection(h, 0.0));
    let report = lagging.stats.initialization.as_ref().unwrap();
    let di = report.max_current_deviation_a.unwrap();
    let expected = 2.0 * PI * 50.0 * h * 200.0 * 2.0_f64.sqrt();
    assert!(
        di > 0.5 * expected && di < 2.0 * expected,
        "{di} against {expected}"
    );
}

#[test]
fn a_controlled_source_without_a_phasor_is_zero_in_the_steady_state() {
    let mut model = held_injection(5e-5, 1.0);
    model["components"][2]
        .as_object_mut()
        .unwrap()
        .remove("steady_state");
    let d = diagnostics(&model);
    let codes: Vec<_> = d.iter().map(|d| d.code).collect();
    assert_eq!(codes, ["W240", "W183"]);
    assert_eq!(d[0].path, "components[2]");
    // The set point is then the current of t = 0, which is 0.
    let r = run(&model);
    assert!(r.column("I(INV1.a)").iter().all(|v| v.abs() < 1e-9));
}

#[test]
fn three_phase_controlled_sources_follow_their_signals() {
    // A three-phase controlled voltage source behind R and L feeds a star of
    // resistors. Its signals are a balanced set from dq0_to_abc, so each
    // phase is the single-phase circuit of R + R_load and L, driven by
    // A cos(w (t - h) + phase) from t = h on and by A cos(phase) before.
    // The current source at the second bus injects the same signals into
    // resistors: its voltages are R times the delayed signals.
    let (h, a, f, r_s, l_s, r_load) = (5e-5, 100.0, 50.0, 0.5, 0.01, 4.5);
    let w = 2.0 * PI * f;
    let (r, tau): (f64, f64) = (r_s + r_load, l_s / (r_s + r_load));
    let (z, phi) = (r.hypot(w * l_s), (w * l_s / r).atan());
    let resistors = |name: &str, bus: &str, ohm: f64| -> Vec<Value> {
        ["a", "b", "c"]
            .iter()
            .map(|p| json!({"type": "resistor", "name": format!("{name}{p}"), "nodes": [format!("{bus}.{p}"), "gnd"], "resistance_ohm": ohm}))
            .collect()
    };
    let mut components = vec![
        json!({"type": "three_phase_controlled_source", "name": "VSC1", "bus": "b1", "signals": ["x.a", "x.b", "x.c"],
            "impedance": {"type": "sequence", "r1_ohm": r_s, "l1_h": l_s, "r0_ohm": r_s, "l0_h": l_s}}),
        json!({"type": "three_phase_controlled_current_source", "name": "INV1", "bus": "b2", "signals": ["x.a", "x.b", "x.c"]}),
    ];
    components.extend(resistors("RL", "b1", r_load));
    components.extend(resistors("RS", "b2", 2.0));
    let out = run(&json!({
        "format": "openkeito-model",
        "version": "0.5",
        "simulation": {"time_step_s": h, "end_time_s": 0.04},
        "components": components,
        "controls": [
            {"type": "constant", "name": "amplitude", "value": a},
            {"type": "constant", "name": "nothing", "value": 0.0},
            {"type": "ramp", "name": "angle", "slope_per_s": w},
            {"type": "dq0_to_abc", "name": "x", "d": "amplitude", "q": "nothing", "angle": "angle"},
        ],
        "outputs": [
            {"type": "branch_current", "component": "VSC1"},
            {"type": "bus_voltage", "bus": "b2"},
            {"type": "branch_current", "component": "INV1"},
        ],
    }));
    for (p, name) in ["a", "b", "c"].iter().enumerate() {
        let shift = -2.0 * PI / 3.0 * p as f64;
        // i' tau + i = A/R cos(w s + shift): the forced response and the
        // decay of what it lacks at s = 0, where the current is that of the
        // constant voltage A cos(shift) before... the circuit starts at rest
        // with that voltage from t = 0, so i(s = 0) follows from the first
        // step: i = A cos(shift)/R (1 - e^(-h/tau)).
        let start = a * shift.cos() / r * (1.0 - (-h / tau).exp());
        let forced = |s: f64| a / z * (w * s + shift - phi).cos();
        let current = |t: f64| {
            if t <= h {
                a * shift.cos() / r * (1.0 - (-t / tau).exp())
            } else {
                let s = t - h;
                forced(s) + (start - forced(0.0)) * (-s / tau).exp()
            }
        };
        // The current flows from the bus into the source: the source
        // delivers -I(VSC1.p). (w h)^2/12 of A/|Z| = 17 A is 3.6e-4 A.
        let e = out.error(&format!("I(VSC1.{name})"), |t| -current(t));
        assert!(e < 3.6e-4, "phase {name}: {e:e}");
        let injected = |t: f64| a * (w * (t - h).max(0.0) + shift).cos();
        let e = out.error(&format!("I(INV1.{name})"), injected);
        assert!(e < 1e-11, "phase {name}: {e:e}");
        let e = out.error(&format!("V(b2.{name})"), |t| 2.0 * injected(t));
        assert!(e < 1e-10, "phase {name}: {e:e}");
    }
}

#[test]
fn a_measurement_reads_the_flux_that_a_nonlinear_inductor_offers() {
    // V sin(w t) across a nonlinear inductor that starts without flux: the
    // flux linkage is the integral of the voltage, V/w (1 - cos(w t)),
    // whatever the characteristic. The signal is that of the grid point.
    // Error: (w h)^2/12 of V/w for the trapezoidal rule, less for 2S-DIRK.
    let (v, f, h) = (100.0, 50.0, 1e-4);
    let w = 2.0 * PI * f;
    let model = json!({
        "format": "openkeito-model",
        "version": "0.5",
        "simulation": {"time_step_s": h, "end_time_s": 0.04},
        "components": [
            {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {"type": "sine", "amplitude": v, "frequency_hz": f}},
            {"type": "nonlinear_inductor", "name": "L1", "nodes": ["n1", "gnd"], "current_a": [1.0, 100.0], "flux_linkage_wb": [0.3, 0.36]},
        ],
        "controls": [
            {"type": "measurement", "name": "flux", "quantity": {"type": "component_signal", "component": "L1", "signal": "flux_wb"}},
            {"type": "gain", "name": "twice", "input": "flux", "gain": 2.0},
        ],
        "outputs": [{"type": "signal", "signal": "flux"}, {"type": "signal", "signal": "twice"}],
    });
    let r = run(&model);
    let e = r.error("flux", |t| v / w * (1.0 - (w * t).cos()));
    assert!(e < (w * h).powi(2) / 12.0 * v / w, "{e:e}");
    // The flux passes the knee at 0.3 Wb: the run has changed segments.
    assert!(r.stats.operating_point_iterations > 0);
    assert!(r.column("flux").iter().any(|x| *x > 0.5));
    // A signal the inductor does not offer.
    let mut wrong = model.clone();
    wrong["controls"][0]["quantity"]["signal"] = json!("flux");
    let d = diagnostics(&wrong);
    assert_eq!(d[0].code, "E244");
    assert!(d[0].hint.contains("flux_wb"), "{}", d[0].hint);
}

#[test]
fn a_controlled_source_keeps_its_straight_line_through_a_step_that_is_split() {
    // The circuit of the delayed sine, and beside it a switch that a PWM
    // signal drives, with the switching instants placed within the steps:
    // a quarter of the steps are then solved in two or three parts. The
    // controlled source reads its signal at grid points only and takes its
    // value from the time within the step.
    //
    // It is compared with the same run in which a plain source gives the
    // delayed sine A sin(w (t - h)): the two differ by the chords of the
    // controlled source only, at most (w h)^2/8 of A, once the difference
    // of the first step (the plain source is not 0 before t = h) has decayed
    // (10 time constants). Under the trapezoidal rule every switching
    // action is followed by backward-Euler steps for the whole circuit,
    // with either source, so both runs are then of first order against the
    // closed form; 2S-DIRK keeps the bound of the run without a switch.
    let (r, l, a, f): (f64, f64, f64, f64) = (1.0, 0.01, 10.0, 50.0);
    let w = 2.0 * PI * f;
    let (z, phi, tau) = (r.hypot(w * l), (w * l / r).atan(), l / r);
    let h = 1e-4;
    let bound = (w * h).powi(2) / 8.0 * a / z;
    for method in METHODS {
        let mut model = driven_rl(
            method,
            h,
            0.12,
            r,
            l,
            json!([
                {"type": "sine_wave", "name": "u", "amplitude": a, "frequency_hz": f},
                {"type": "constant", "name": "duty", "value": 0.37},
                {"type": "triangle_wave", "name": "carrier", "frequency_hz": 1300.0},
                {"type": "comparator", "name": "pwm", "inputs": ["duty", "carrier"]},
            ]),
        );
        model["simulation"]["switching"] = json!("interpolated");
        let more = json!([
            {"type": "voltage_source", "name": "E2", "nodes": ["m1", "gnd"], "waveform": {"type": "dc", "value": 5.0}},
            {"type": "controlled_switch", "name": "S1", "nodes": ["m1", "m2"], "gate": "pwm"},
            {"type": "resistor", "name": "R2", "nodes": ["m2", "gnd"], "resistance_ohm": 2.0},
        ]);
        model["components"]
            .as_array_mut()
            .unwrap()
            .extend(more.as_array().unwrap().iter().cloned());
        let mut plain = model.clone();
        plain["components"][0] = json!({"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"],
            "waveform": {"type": "sine", "amplitude": a, "frequency_hz": f, "phase_deg": (-w * h).to_degrees()}});
        let (controlled, plain) = (run(&model), run(&plain));
        assert!(
            controlled.stats.split_steps > 200,
            "{}",
            controlled.stats.split_steps
        );
        assert_eq!(controlled.stats.split_steps, plain.stats.split_steps);
        let difference = controlled
            .t
            .iter()
            .zip(controlled.column("I(L1)"))
            .zip(plain.column("I(L1)"))
            .filter(|((t, _), _)| **t >= 0.1)
            .map(|((_, x), y)| (x - y).abs())
            .fold(0.0, f64::max);
        assert!(
            difference < bound,
            "{method}: {difference:e} against {bound:e}"
        );
        // The source itself at the grid points: the signal of the grid
        // point before.
        let e = controlled.error("V(n1)", |t| a * (w * (t - h).max(0.0)).sin());
        assert!(e < 1e-9, "{method}: source {e:e}");
        if method == "2s-dirk" {
            let e = controlled.error("I(L1)", |t| {
                let s = (t - h).max(0.0);
                a / z * ((w * s - phi).sin() + phi.sin() * (-s / tau).exp())
            });
            assert!(e < bound, "{e:e} against {bound:e}");
        }
    }
}

#[test]
fn piecewise_linear_interpolates_clamps_and_rejects_non_finite() {
    // Single-point table holds y[0] everywhere.
    let single = blocks_07(
        1e-3,
        4e-3,
        json!([
            {"type": "ramp", "name": "u", "initial": -2.0, "slope_per_s": 1000.0},
            {"type": "piecewise_linear", "name": "y", "input": "u", "x": [0.5], "y": [3.25]},
        ]),
        &["u", "y"],
    );
    let r = run(&single);
    for &v in &r.column("y") {
        assert_eq!(v, 3.25);
    }

    // Multi-segment table: x = [-1, 0, 2, 5], y = [10, 0, 4, 1].
    // u(t) = -2 + 1000 * t sweeps from -2 to +6 over [0, 8 ms].
    let multi = blocks_07(
        5e-4,
        8e-3,
        json!([
            {"type": "ramp", "name": "u", "initial": -2.0, "slope_per_s": 1000.0},
            {"type": "piecewise_linear", "name": "y", "input": "u", "x": [-1.0, 0.0, 2.0, 5.0], "y": [10.0, 0.0, 4.0, 1.0]},
        ]),
        &["u", "y"],
    );
    let r = run(&multi);
    let exact = |t: f64| {
        let u = -2.0 + 1000.0 * t;
        if u <= -1.0 {
            10.0
        } else if u < 0.0 {
            10.0 + (u + 1.0) * (-10.0)
        } else if u < 2.0 {
            0.0 + (u / 2.0) * 4.0
        } else if u < 5.0 {
            4.0 + ((u - 2.0) / 3.0) * (-3.0)
        } else {
            1.0
        }
    };
    let e = r.error("y", exact);
    assert!(e < 1e-12, "piecewise_linear error {e:e}");

    // Non-finite input stops with E251 naming the piecewise_linear signal.
    let nan_model = blocks_07(
        1e-3,
        1e-3,
        json!([
            {"type": "constant", "name": "one", "value": 1.0},
            {"type": "constant", "name": "zero", "value": 0.0},
            {"type": "quotient", "name": "q", "inputs": ["one", "zero"]},
            {"type": "piecewise_linear", "name": "lut", "input": "q", "x": [0.0, 1.0], "y": [0.0, 1.0]},
        ]),
        &["lut"],
    );
    let err = try_run(&nan_model).err().expect("non-finite input fails");
    let d = err.to_diagnostic();
    assert_eq!((d.code, d.path.as_str()), ("E251", "q"));
}

#[test]
fn signal_limits_match_constant_limits_on_lag_pi_and_limiter() {
    let h = 2e-4;
    let end = 0.03;
    let numeric = blocks_07(
        h,
        end,
        json!([
            {"type": "sine_wave", "name": "u", "amplitude": 2.0, "frequency_hz": 50.0},
            {"type": "limiter", "name": "lim", "input": "u", "minimum": -0.7, "maximum": 0.8},
            {"type": "lag", "name": "lag", "input": "u", "gain": 1.5, "time_constant_s": 0.005, "minimum": -0.7, "maximum": 0.8, "initial_input": 0.0},
            {"type": "pi", "name": "pi", "input": "u", "proportional_gain": 0.5, "integral_gain_per_s": 80.0, "output_min": -0.7, "output_max": 0.8},
        ]),
        &["lim", "lag", "pi"],
    );
    let signaled = blocks_07(
        h,
        end,
        json!([
            {"type": "sine_wave", "name": "u", "amplitude": 2.0, "frequency_hz": 50.0},
            {"type": "constant", "name": "lo", "value": -0.7},
            {"type": "constant", "name": "hi", "value": 0.8},
            {"type": "limiter", "name": "lim", "input": "u", "minimum": "lo", "maximum": "hi"},
            {"type": "lag", "name": "lag", "input": "u", "gain": 1.5, "time_constant_s": 0.005, "minimum": "lo", "maximum": "hi", "initial_input": 0.0},
            {"type": "pi", "name": "pi", "input": "u", "proportional_gain": 0.5, "integral_gain_per_s": 80.0, "output_min": "lo", "output_max": "hi"},
        ]),
        &["lim", "lag", "pi"],
    );
    let r_num = run(&numeric);
    let r_sig = run(&signaled);
    for col in ["lim", "lag", "pi"] {
        let diff = r_num
            .column(col)
            .iter()
            .zip(r_sig.column(col))
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f64::max);
        assert!(diff < 1e-14, "{col}: diff {diff:e}");
    }
}

#[test]
fn lag_with_dynamic_signal_limit_tracks_without_windup_and_converges_at_second_order() {
    // 1) Step limit: u = 2, tau = 0.01, max steps from 0.5 to 1.5 at t = 0.01.
    //    Lag is clamped at 0.5 before t = 0.01 and immediately rises at t = 0.01.
    let step_model = blocks_07(
        2e-4,
        0.02,
        json!([
            {"type": "constant", "name": "u", "value": 2.0},
            {"type": "step", "name": "hi", "time_s": 0.01, "before": 0.5, "after": 1.5},
            {"type": "lag", "name": "y", "input": "u", "gain": 1.0, "time_constant_s": 0.01, "maximum": "hi", "initial_input": 0.0},
        ]),
        &["y"],
    );
    let r = run(&step_model);
    let k_before = r.t.iter().position(|&t| (t - 0.0098).abs() < 1e-9).unwrap();
    let k_at = r.t.iter().position(|&t| (t - 0.01).abs() < 1e-9).unwrap();
    assert_eq!(r.column("y")[k_before], 0.5);
    assert!(r.column("y")[k_at] > 0.5, "must leave 0.5 immediately");

    // 2) Smooth Ramp limit: max(t) = 0.2 + 50 t, u = 2, tau = 0.01, y(0) = 0.
    //    y hits max(t) around t = 1.45 ms, tracks it until t_d = 0.026 where
    //    (2 - max)/tau = 50 = max'(t) (C^1 detachment), then follows
    //    2 - 0.5 exp(-(t - 0.026)/0.01).
    let ramp_model = |h: f64| {
        blocks_07(
            h,
            0.05,
            json!([
                {"type": "constant", "name": "u", "value": 2.0},
                {"type": "ramp", "name": "hi", "initial": 0.2, "slope_per_s": 50.0},
                {"type": "lag", "name": "y", "input": "u", "gain": 1.0, "time_constant_s": 0.01, "maximum": "hi", "initial_input": 0.0},
            ]),
            &["y"],
        )
    };
    let err_at = |h: f64| {
        run(&ramp_model(h)).error_from("y", 0.026, |t| 2.0 - 0.5 * (-(t - 0.026) / 0.01).exp())
    };
    assert_second_order("lag dynamic limit (h, h/2)", errors_at(4e-4, err_at));
    assert_second_order("lag dynamic limit (h/2, h/4)", errors_at(2e-4, err_at));
}

#[test]
fn pi_with_dynamic_signal_limit_holds_and_converges_at_second_order() {
    // kp = 0, ki = 100, e(t) = cos(50 pi t).
    // For t < 0.01, e(t) > 0 and output_max = 0.5 holds integral at 0.5.
    // At t = 0.01, e(0.01) = 0 and output_max steps to 2.0; for t in [0.01, 0.02],
    // e(t) < 0 so integral(t) = 0.5 + (100 / (50 pi)) (sin(50 pi t) - 1).
    let model = |h: f64| {
        blocks_07(
            h,
            0.02,
            json!([
                {"type": "sine_wave", "name": "e", "amplitude": 1.0, "frequency_hz": 25.0, "phase_deg": 90.0},
                {"type": "step", "name": "hi", "time_s": 0.01, "before": 0.5, "after": 2.0},
                {"type": "pi", "name": "u", "input": "e", "proportional_gain": 0.0, "integral_gain_per_s": 100.0, "initial_integral": 0.5, "output_max": "hi"},
            ]),
            &["u"],
        )
    };
    let r = run(&model(1e-4));
    let k_before = r.t.iter().position(|&t| (t - 0.0099).abs() < 1e-9).unwrap();
    assert_eq!(r.column("u")[k_before], 0.5);

    let exact = |t: f64| 0.5 + (100.0 / (50.0 * PI)) * ((50.0 * PI * t).sin() - 1.0);
    let err_at = |h: f64| run(&model(h)).error_from("u", 0.01, exact);
    assert_second_order("pi dynamic limit (h, h/2)", errors_at(2e-4, err_at));
    assert_second_order("pi dynamic limit (h/2, h/4)", errors_at(1e-4, err_at));
}

#[test]
fn limiter_with_dynamic_limits_handles_inversion_and_non_finite() {
    // When min > max transiently, min wins and there is no panic.
    let inv = blocks_07(
        1e-3,
        4e-3,
        json!([
            {"type": "constant", "name": "u", "value": 0.5},
            {"type": "step", "name": "lo", "time_s": 2e-3, "before": -0.2, "after": 1.2},
            {"type": "constant", "name": "hi", "value": 0.8},
            {"type": "limiter", "name": "y", "input": "u", "minimum": "lo", "maximum": "hi"},
        ]),
        &["y"],
    );
    let r = run(&inv);
    let y = r.column("y");
    assert_eq!(y[0], 0.5);
    assert_eq!(y[1], 0.5);
    assert_eq!(y[2], 1.2);
    assert_eq!(y[3], 1.2);
    assert_eq!(y[4], 1.2);

    // Non-finite limit signal stops with E251.
    let bad_lim = blocks_07(
        1e-3,
        1e-3,
        json!([
            {"type": "constant", "name": "u", "value": 0.5},
            {"type": "constant", "name": "zero", "value": 0.0},
            {"type": "quotient", "name": "q", "inputs": ["u", "zero"]},
            {"type": "limiter", "name": "y", "input": "u", "maximum": "q"},
        ]),
        &["y"],
    );
    let err = try_run(&bad_lim).err().expect("non-finite limit fails");
    assert_eq!(err.to_diagnostic().code, "E251");
}

#[test]
fn out_of_bounds_initial_states_clamp_at_t0_for_pi_and_lag() {
    let h = 1e-3;
    // PI: initial_integral = 0.0 (numeric and signal), output_min = 0.9 (numeric and signal).
    // e = -0.1 for t < 3 ms, then e = +0.1 for t >= 3 ms.
    // Must start at 0.9, hold integral at 0.9 for negative e, and rise above 0.9 at t = 3 ms.
    for (init_json, min_json, extra) in [
        (json!(0.0), json!(0.9), vec![]),
        (
            json!(0.0),
            json!("lo"),
            vec![json!({"type": "constant", "name": "lo", "value": 0.9})],
        ),
        (
            json!("init0"),
            json!(0.9),
            vec![json!({"type": "constant", "name": "init0", "value": 0.0})],
        ),
        (
            json!("init0"),
            json!("lo"),
            vec![
                json!({"type": "constant", "name": "init0", "value": 0.0}),
                json!({"type": "constant", "name": "lo", "value": 0.9}),
            ],
        ),
    ] {
        let mut ctrl = vec![
            json!({"type": "step", "name": "e", "time_s": 2.5e-3, "before": -0.1, "after": 0.1}),
            json!({
                "type": "pi",
                "name": "u",
                "input": "e",
                "proportional_gain": 0.0,
                "integral_gain_per_s": 100.0,
                "initial_integral": init_json,
                "output_min": min_json,
            }),
        ];
        ctrl.extend(extra);
        let r = run(&blocks_07(h, 4e-3, json!(ctrl), &["u"]));
        let u = r.column("u");
        assert_eq!(u[0], 0.9);
        assert_eq!(u[1], 0.9);
        assert_eq!(u[2], 0.9);
        // At t = 3 ms: delta = 100 * 0.5 * (-0.1 + 0.1) * 1e-3 = 0.0;
        // at t = 4 ms: delta = 100 * 0.5 * (0.1 + 0.1) * 1e-3 = 0.01 -> 0.91.
        assert!((u[4] - 0.91).abs() < 1e-12, "no wind-up deficit: {}", u[4]);
    }

    // Lag: gain * initial_input = 2.0 > maximum = 0.9 (numeric and signal).
    // u = 0.0 from t = h, so it must start at 0.9 and immediately decay at t = h.
    for (max_json, extra) in [
        (json!(0.9), vec![]),
        (
            json!("hi"),
            vec![json!({"type": "constant", "name": "hi", "value": 0.9})],
        ),
    ] {
        let mut ctrl = vec![
            json!({"type": "constant", "name": "u", "value": 0.0}),
            json!({
                "type": "lag",
                "name": "y",
                "input": "u",
                "gain": 2.0,
                "time_constant_s": 0.01,
                "initial_input": 1.0,
                "maximum": max_json,
            }),
        ];
        ctrl.extend(extra);
        let r = run(&blocks_07(h, 2e-3, json!(ctrl), &["y"]));
        let y = r.column("y");
        assert_eq!(y[0], 0.9);
        let a = h / (2.0 * 0.01);
        let expected_1 = 0.9 * (1.0 - a) / (1.0 + a);
        assert!((y[1] - expected_1).abs() < 1e-12, "y[1] = {}", y[1]);
    }
}

#[test]
fn integrator_with_signal_limits_matches_constants_tracks_and_converges_at_second_order() {
    // 1) Constant signal limits match numeric limits to 1e-15.
    let h = 2e-4;
    let end = 0.02;
    let numeric = blocks_07(
        h,
        end,
        json!([
            {"type": "sine_wave", "name": "u", "amplitude": 200.0, "frequency_hz": 50.0},
            {"type": "integrator", "name": "clamp_i", "input": "u", "gain_per_s": 1.0, "minimum": -0.3, "maximum": 0.4},
        ]),
        &["clamp_i"],
    );
    let signaled = blocks_07(
        h,
        end,
        json!([
            {"type": "sine_wave", "name": "u", "amplitude": 200.0, "frequency_hz": 50.0},
            {"type": "constant", "name": "lo", "value": -0.3},
            {"type": "constant", "name": "hi", "value": 0.4},
            {"type": "integrator", "name": "clamp_i", "input": "u", "gain_per_s": 1.0, "minimum": "lo", "maximum": "hi"},
        ]),
        &["clamp_i"],
    );
    let r_num = run(&numeric);
    let r_sig = run(&signaled);
    let diff = r_num
        .column("clamp_i")
        .iter()
        .zip(r_sig.column("clamp_i"))
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f64::max);
    assert!(diff < 1e-15, "clamp_i: diff {diff:e}");

    // 2) Step limit: non-windup release & tracking on moving limit, plus out-of-bounds t = 0 clamp.
    let step_lim = blocks_07(
        2e-4,
        0.02,
        json!([
            {"type": "constant", "name": "u", "value": 100.0},
            {"type": "constant", "name": "lo", "value": 0.2},
            {"type": "step", "name": "hi", "time_s": 0.01, "before": 0.5, "after": 1.5},
            {"type": "integrator", "name": "y", "input": "u", "gain_per_s": 1.0, "initial_value": 0.0, "minimum": "lo", "maximum": "hi"},
        ]),
        &["y"],
    );
    let r = run(&step_lim);
    let y = r.column("y");
    assert_eq!(y[0], 0.2);
    let k_before = r.t.iter().position(|&t| (t - 0.0098).abs() < 1e-9).unwrap();
    let k_at = r.t.iter().position(|&t| (t - 0.01).abs() < 1e-9).unwrap();
    assert_eq!(y[k_before], 0.5);
    assert!(y[k_at] > 0.5, "must leave 0.5 immediately at step");

    // 3) Smooth Ramp limit tracking: x'(t) = 100, max(t) = 0.2 + 50 t -> tracks max(t) once reached.
    let ramp_lim = blocks_07(
        2e-4,
        0.01,
        json!([
            {"type": "constant", "name": "u", "value": 1.0},
            {"type": "ramp", "name": "hi", "initial": 0.2, "slope_per_s": 50.0},
            {"type": "integrator", "name": "y", "input": "u", "gain_per_s": 100.0, "maximum": "hi"},
        ]),
        &["y"],
    );
    let r_ramp = run(&ramp_lim);
    let k_end = r_ramp.t.len() - 1;
    let t_end = r_ramp.t[k_end];
    assert!(
        (r_ramp.column("y")[k_end] - (0.2 + 50.0 * t_end)).abs() < 1e-12,
        "must track moving ramp limit"
    );

    // 4) Second-order trapezoidal convergence on dynamic signal limit.
    let conv_model = |h: f64| {
        blocks_07(
            h,
            0.02,
            json!([
                {"type": "sine_wave", "name": "e", "amplitude": 1.0, "frequency_hz": 25.0, "phase_deg": 90.0},
                {"type": "step", "name": "hi", "time_s": 0.01, "before": 0.5, "after": 2.0},
                {"type": "integrator", "name": "y", "input": "e", "gain_per_s": 100.0, "initial_value": 0.5, "maximum": "hi"},
            ]),
            &["y"],
        )
    };
    let exact = |t: f64| 0.5 + (100.0 / (50.0 * PI)) * ((50.0 * PI * t).sin() - 1.0);
    let err_at = |h: f64| run(&conv_model(h)).error_from("y", 0.01, exact);
    assert_second_order("integrator dynamic limit (h, h/2)", errors_at(2e-4, err_at));
    assert_second_order(
        "integrator dynamic limit (h/2, h/4)",
        errors_at(1e-4, err_at),
    );

    // 5) Transient min > max: min wins, no panic.
    let inv = blocks_07(
        1e-3,
        3e-3,
        json!([
            {"type": "constant", "name": "u", "value": 0.0},
            {"type": "step", "name": "lo", "time_s": 1.5e-3, "before": -0.2, "after": 1.2},
            {"type": "constant", "name": "hi", "value": 0.8},
            {"type": "integrator", "name": "y", "input": "u", "gain_per_s": 1.0, "initial_value": 0.5, "minimum": "lo", "maximum": "hi"},
        ]),
        &["y"],
    );
    let r_inv = run(&inv);
    assert_eq!(r_inv.column("y")[0], 0.5);
    assert_eq!(r_inv.column("y")[2], 1.2);

    // 6) Non-finite signal limit stops with E251.
    let bad_lim = blocks_07(
        1e-3,
        1e-3,
        json!([
            {"type": "constant", "name": "u", "value": 1.0},
            {"type": "constant", "name": "zero", "value": 0.0},
            {"type": "quotient", "name": "q", "inputs": ["u", "zero"]},
            {"type": "integrator", "name": "y", "input": "u", "gain_per_s": 1.0, "maximum": "q"},
        ]),
        &["y"],
    );
    let err = try_run(&bad_lim).err().expect("non-finite limit fails");
    assert_eq!(err.to_diagnostic().code, "E251");
}

#[test]
fn pi_state_anti_windup_matches_block_composition_converges_and_differs_from_conditional() {
    // 1) Agrees within 1e-13 with gain + integrator(min, max) + sum + limiter.
    let h = 2e-4;
    let end = 0.03;
    let m = blocks_07(
        h,
        end,
        json!([
            {"type": "sine_wave", "name": "e", "amplitude": 1.5, "frequency_hz": 50.0},
            {"type": "constant", "name": "lo", "value": -0.7},
            {"type": "step", "name": "hi", "time_s": 0.015, "before": 0.6, "after": 1.1},
            {"type": "pi", "name": "pi_state", "input": "e", "proportional_gain": 0.6, "integral_gain_per_s": 120.0, "initial_integral": 0.1, "output_min": "lo", "output_max": "hi", "anti_windup": "state"},
            {"type": "gain", "name": "p_part", "input": "e", "gain": 0.6},
            {"type": "integrator", "name": "i_part", "input": "e", "gain_per_s": 120.0, "initial_value": 0.1, "minimum": "lo", "maximum": "hi"},
            {"type": "sum", "name": "raw_sum", "inputs": ["p_part", "i_part"], "signs": [1.0, 1.0]},
            {"type": "limiter", "name": "ref_out", "input": "raw_sum", "minimum": "lo", "maximum": "hi"},
        ]),
        &["pi_state", "ref_out"],
    );
    let r = run(&m);
    let diff = r
        .column("pi_state")
        .iter()
        .zip(r.column("ref_out"))
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f64::max);
    assert!(
        diff < 1e-13,
        "state anti-windup vs composition diff {diff:e}"
    );

    // 2) Second-order convergence for anti_windup = "state".
    let conv_model = |h: f64| {
        blocks_07(
            h,
            0.02,
            json!([
                {"type": "sine_wave", "name": "e", "amplitude": 1.0, "frequency_hz": 25.0, "phase_deg": 90.0},
                {"type": "step", "name": "hi", "time_s": 0.01, "before": 0.5, "after": 2.0},
                {"type": "pi", "name": "u", "input": "e", "proportional_gain": 0.0, "integral_gain_per_s": 100.0, "initial_integral": 0.5, "output_max": "hi", "anti_windup": "state"},
            ]),
            &["u"],
        )
    };
    let exact = |t: f64| 0.5 + (100.0 / (50.0 * PI)) * ((50.0 * PI * t).sin() - 1.0);
    let err_at = |h: f64| run(&conv_model(h)).error_from("u", 0.01, exact);
    assert_second_order("pi state anti_windup (h, h/2)", errors_at(2e-4, err_at));
    assert_second_order("pi state anti_windup (h/2, h/4)", errors_at(1e-4, err_at));

    // 3) Dynamic recovery peak difference vs "conditional":
    //    For t < 0.02, e = 1.2 so kp*e = 1.2 > max = 1.0.
    //    "conditional" holds integral at 0.0, whereas "state" integrates it up to max = 1.0.
    //    When e steps to -0.5 at t = 0.02, "conditional" recovers to -0.5 while "state" recovers to +0.5.
    let cmp = blocks_07(
        2e-4,
        0.025,
        json!([
            {"type": "step", "name": "e", "time_s": 0.02, "before": 1.2, "after": -0.5},
            {"type": "pi", "name": "cond", "input": "e", "proportional_gain": 1.0, "integral_gain_per_s": 200.0, "initial_integral": 0.0, "output_min": -1.0, "output_max": 1.0, "anti_windup": "conditional"},
            {"type": "pi", "name": "state", "input": "e", "proportional_gain": 1.0, "integral_gain_per_s": 200.0, "initial_integral": 0.0, "output_min": -1.0, "output_max": 1.0, "anti_windup": "state"},
        ]),
        &["cond", "state"],
    );
    let r_cmp = run(&cmp);
    let k_rec = r_cmp
        .t
        .iter()
        .position(|&t| (t - 0.0202).abs() < 1e-9)
        .unwrap();
    assert!(
        r_cmp.column("cond")[k_rec] < -0.4,
        "cond: {}",
        r_cmp.column("cond")[k_rec]
    );
    assert!(
        r_cmp.column("state")[k_rec] > 0.4,
        "state: {}",
        r_cmp.column("state")[k_rec]
    );
}
