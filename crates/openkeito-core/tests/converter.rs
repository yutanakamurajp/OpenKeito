//! The averaged grid-following converter against closed-form solutions, an
//! independent model of its sampled current loop, a power flow, and the
//! same control driving a switching bridge.

mod common;

use std::f64::consts::PI;

use common::{diagnostics, run_model as run, Run};
use serde_json::{json, Value};

impl Run {
    /// The values of the column `name`, or of the signal of that name of
    /// the converter PV1.
    fn column(&self, name: &str) -> Vec<f64> {
        let signal = format!("S(PV1.{name})");
        let k = self
            .columns
            .iter()
            .position(|n| n == name || *n == signal)
            .unwrap_or_else(|| panic!("no column {name} among {:?}", self.columns));
        self.rows.iter().map(|r| r[k]).collect()
    }

    /// Largest |column - exact(t)| over the rows with from <= t <= to.
    fn error(&self, name: &str, (from, to): (f64, f64), exact: impl Fn(f64) -> f64) -> f64 {
        self.t
            .iter()
            .zip(self.column(name))
            .filter(|(t, _)| **t >= from - 1e-12 && **t <= to + 1e-12)
            .map(|(t, v)| (v - exact(*t)).abs())
            .fold(0.0, f64::max)
    }

    /// Largest and smallest value of the column over from <= t <= to.
    fn range(&self, name: &str, (from, to): (f64, f64)) -> (f64, f64) {
        self.t
            .iter()
            .zip(self.column(name))
            .filter(|(t, _)| **t >= from - 1e-12 && **t <= to + 1e-12)
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), (_, v)| {
                (lo.min(v), hi.max(v))
            })
    }

    /// The value of the column at the row closest to the time `t`.
    fn at(&self, name: &str, t: f64) -> f64 {
        let k = (0..self.t.len())
            .min_by(|&a, &b| (self.t[a] - t).abs().total_cmp(&(self.t[b] - t).abs()))
            .unwrap();
        self.column(name)[k]
    }

    fn warning_codes(&self) -> Vec<&str> {
        self.stats.warnings.iter().map(|d| d.code).collect()
    }
}

fn codes(model: &Value) -> Vec<&'static str> {
    diagnostics(model).iter().map(|d| d.code).collect()
}

/// Ratings of the converter of every test: 2 MVA, 6.6 kV, 50 Hz.
const S_MVA: f64 = 2.0;
const V_KV: f64 = 6.6;
const F_HZ: f64 = 50.0;
/// The reactor: the defaults of the component.
const X_PU: f64 = 0.15;
const R_PU: f64 = 0.005;

fn omega() -> f64 {
    2.0 * PI * F_HZ
}

/// `{"type": "signal", ...}` for the signals `names` of the converter PV1.
fn signals(names: &[&str]) -> Vec<Value> {
    names
        .iter()
        .map(|n| json!({"type": "signal", "signal": format!("PV1.{n}")}))
        .collect()
}

/// The converter PV1 at the bus b1 with `fields` beyond its ratings.
fn converter(fields: Value) -> Value {
    let mut c = json!({"type": "grid_following_converter", "name": "PV1", "bus": "b1",
        "rated_power_mva": S_MVA, "rated_voltage_kv": V_KV, "rated_frequency_hz": F_HZ});
    for (k, v) in fields.as_object().unwrap() {
        c[k] = v.clone();
    }
    c
}

/// A sequence impedance of `z_pu` per unit of the ratings with X/R = 10.
fn impedance(z_pu: f64) -> Value {
    let z_base = V_KV * V_KV / S_MVA;
    let x = z_pu * z_base / (1.0_f64 + 0.01).sqrt();
    let (r, l) = (x / 10.0, x / omega());
    json!({"type": "sequence", "r1_ohm": r, "l1_h": l, "r0_ohm": r, "l0_h": l})
}

/// The converter on an ideal source of its rated voltage (an infinite bus).
#[allow(clippy::too_many_arguments)]
fn on_infinite_bus(
    method: &str,
    h: f64,
    end: f64,
    initialization: &str,
    grid_hz: f64,
    fields: Value,
    controls: Value,
    outputs: &[&str],
) -> Value {
    json!({"format": "openkeito-model", "version": "0.5",
        "simulation": {"time_step_s": h, "end_time_s": end, "method": method, "initialization": initialization},
        "components": [
            {"type": "three_phase_source", "name": "G1", "bus": "b1", "line_voltage_rms_v": V_KV * 1e3, "frequency_hz": grid_hz},
            converter(fields),
        ],
        "controls": controls,
        "outputs": signals(outputs)})
}

type C = (f64, f64);

fn mul(a: C, b: C) -> C {
    (a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0)
}
fn add(a: C, b: C) -> C {
    (a.0 + b.0, a.1 + b.1)
}
fn scale(a: C, k: f64) -> C {
    (a.0 * k, a.1 * k)
}
fn rotation(angle: f64) -> C {
    (angle.cos(), angle.sin())
}

/// The currents (d, q) at the grid points of the current loop of a
/// converter on an infinite bus of 1 per unit, written without the solver:
/// the PI controller at the grid points (trapezoidal rule) with the
/// feedforward v + j x i, the source one time step late and in straight
/// lines between its phase values, and the reactor
/// (x / omega) di/dt = -(r + j x) i + e - v integrated with the classical
/// Runge-Kutta method in 200 parts of a step. In the frame of the bus the
/// straight line of the phase values from u_prev (given for the angle of the
/// start of the step) to u (for its end) is
/// (1 - f) u_prev e^{-j omega h f} + f u e^{j omega h (1 - f)}.
fn sampled_current_loop(
    h: f64,
    tau: f64,
    steps: usize,
    reference: impl Fn(usize) -> C,
    start: C,
) -> Vec<C> {
    let w = omega();
    let (kp, ki) = (X_PU / (w * tau), R_PU / tau);
    let z = (R_PU, X_PU);
    let v = (1.0, 0.0);
    let mut i = start;
    let mut integral = scale(i, R_PU);
    let mut u_prev = add(v, mul(z, i));
    let mut e_prev = (0.0, 0.0);
    let mut out = vec![i];
    const PARTS: usize = 200;
    for n in 0..steps {
        let r = reference(n);
        let e = (r.0 - i.0, r.1 - i.1);
        if n > 0 {
            integral = add(integral, scale(add(e, e_prev), ki * h / 2.0));
        }
        e_prev = e;
        let u = add(add(scale(e, kp), integral), add(v, mul((0.0, X_PU), i)));
        let source = |f: f64| {
            add(
                scale(mul(u_prev, rotation(-w * h * f)), 1.0 - f),
                scale(mul(u, rotation(w * h * (1.0 - f))), f),
            )
        };
        let slope = |f: f64, y: C| {
            let drop = mul(z, y);
            let s = source(f);
            scale((s.0 - v.0 - drop.0, s.1 - v.1 - drop.1), w / X_PU)
        };
        let dt = h / PARTS as f64;
        for part in 0..PARTS {
            let f0 = part as f64 / PARTS as f64;
            let (fm, f1) = (f0 + 0.5 / PARTS as f64, f0 + 1.0 / PARTS as f64);
            let k1 = slope(f0, i);
            let k2 = slope(fm, add(i, scale(k1, dt / 2.0)));
            let k3 = slope(fm, add(i, scale(k2, dt / 2.0)));
            let k4 = slope(f1, add(i, scale(k3, dt)));
            let sum = add(add(k1, scale(k2, 2.0)), add(scale(k3, 2.0), k4));
            i = add(i, scale(sum, dt / 6.0));
        }
        u_prev = u;
        out.push(i);
    }
    out
}

/// A step of the d current from 0.2 to 0.8 at 10 ms: errors against the
/// sampled loop (d, q), against the first-order lag, and the largest |i_q|.
fn current_step(method: &str, h: f64, tau: f64) -> [f64; 4] {
    let (t_step, end) = (0.01, 0.03);
    let model = on_infinite_bus(
        method,
        h,
        end,
        "steady_state",
        F_HZ,
        // A dc voltage that leaves the voltage reference unlimited.
        json!({"current_time_constant_s": tau, "dc_voltage_kv": 60.0,
            "reference": {"type": "current", "d_pu": "id_cmd"},
            "operating_point": {"active_power_mw": 0.2 * S_MVA}}),
        json!([{"type": "step", "name": "id_cmd", "time_s": t_step, "before": 0.2, "after": 0.8}]),
        &["i_d", "i_q"],
    );
    let r = run(&model);
    let steps = r.t.len() - 1;
    let k_step = (t_step / h).round() as usize;
    let reference = sampled_current_loop(
        h,
        tau,
        steps,
        |n| (if n >= k_step { 0.8 } else { 0.2 }, 0.0),
        (0.2, 0.0),
    );
    let (i_d, i_q) = (r.column("i_d"), r.column("i_q"));
    let worst = |x: &[f64], f: &dyn Fn(usize) -> f64| {
        (0..x.len())
            .map(|k| (x[k] - f(k)).abs())
            .fold(0.0, f64::max)
    };
    [
        worst(&i_d, &|k| reference[k].0),
        worst(&i_q, &|k| reference[k].1),
        worst(&i_d, &|k| {
            0.2 + 0.6 * (1.0 - (-(r.t[k] - t_step).max(0.0) / tau).exp())
        }),
        worst(&i_q, &|_| 0.0),
    ]
}

#[test]
fn a_current_step_follows_the_sampled_loop_and_the_first_order_lag() {
    // With Kp = L / tau and Ki = R / tau the zero of the controller cancels
    // the pole of the reactor: without the dead time the closed loop is
    // 1 / (tau s + 1), and the q current does not move.
    //
    // The solver adds the dead time of one step. Against the same loop
    // written by hand (`sampled_current_loop`) only the integration of the
    // reactor differs, which is of second order in h / (L / R) and in
    // omega h: measured 2.0e-5, 5.0e-6 and 1.3e-6 of the rated current at
    // 100, 50 and 25 us with 2S-DIRK (a factor of 4 per halving), and 8
    // times that with the trapezoidal rule, whose error constant is larger.
    let tau = 1e-3;
    for (method, bound) in [("2s-dirk", 4e-5), ("trapezoidal", 3.2e-4)] {
        let coarse = current_step(method, 1e-4, tau);
        let fine = current_step(method, 5e-5, tau);
        assert!(coarse[0] < bound, "{method}: d {:e}", coarse[0]);
        assert!(coarse[1] < bound / 10.0, "{method}: q {:e}", coarse[1]);
        let ratio = coarse[0] / fine[0];
        assert!((3.5..4.5).contains(&ratio), "{method}: order {ratio}");

        // Against the lag without a dead time, the response is late by the
        // step that the reference takes to reach the source and by the ramp
        // of the source over the next one: between h and 2 h, so the error
        // is at most 0.6 (2 h / tau) / e (the steepest part of the lag is
        // 0.6 / tau, and e^{-1} of it remains where the delayed and the
        // undelayed lag differ most). First order in h.
        for (h, e) in [(1e-4, coarse[2]), (5e-5, fine[2])] {
            assert!(e < 0.6 * 2.0 * h / tau / 1f64.exp(), "{method}: lag {e:e}");
            assert!(e > 0.6 * h / tau / 1f64.exp(), "{method}: lag {e:e}");
        }
        let ratio = coarse[2] / fine[2];
        assert!((1.8..2.2).contains(&ratio), "{method}: lag order {ratio}");

        // The decoupling term omega L i uses the current of the grid point
        // before: while i_d moves by 0.6 / tau per second, the q axis sees
        // the voltage x 0.6 h / tau for about tau, which moves i_q by
        // 0.6 omega h / e at most. Measured 3.6e-3 at 50 us (0.6 % of the
        // step), half of the 7.7e-3 at 100 us.
        for (h, q) in [(1e-4, coarse[3]), (5e-5, fine[3])] {
            assert!(
                q < 0.6 * omega() * h / 1f64.exp() * 1.3,
                "{method}: i_q {q:e}"
            );
        }
        let ratio = coarse[3] / fine[3];
        assert!((1.8..2.3).contains(&ratio), "{method}: i_q order {ratio}");
    }
}

#[test]
fn the_dead_time_of_one_step_limits_the_bandwidth_of_the_current_control() {
    // The loop with the dead time h is tau s + e^{-s h} = 0. Its roots are
    // real up to h / tau = 1 / e = 0.37: no overshoot. Beyond that they are
    // a damped pair, and from h / tau = pi / 2 on the loop is unstable.
    // Largest i_d after a step from 0.2 to 0.8, as overshoot over 0.8:
    let overshoot = |h: f64, tau: f64| {
        let model = on_infinite_bus(
            "2s-dirk",
            h,
            0.03,
            "steady_state",
            F_HZ,
            json!({"current_time_constant_s": tau, "dc_voltage_kv": 60.0,
                "reference": {"type": "current", "d_pu": "id_cmd"},
                "operating_point": {"active_power_mw": 0.2 * S_MVA}}),
            json!([{"type": "step", "name": "id_cmd", "time_s": 0.01, "before": 0.2, "after": 0.8}]),
            &["i_d"],
        );
        let warned = codes(&model).contains(&"W290");
        (run(&model).range("i_d", (0.0, 0.03)).1 - 0.8, warned)
    };
    // h / tau = 0.25: none (the chord of the source leaves 1e-5), and no
    // warning.
    let (over, warned) = overshoot(5e-5, 2e-4);
    assert!(over < 1e-4 && !warned, "{over:e}");
    // h / tau = 0.5: a few percent of the step, with the warning W290.
    let (over, warned) = overshoot(1e-4, 2e-4);
    assert!(over > 0.01 && over < 0.06 && warned, "{over:e}");
    // h / tau = 1: a large overshoot, still stable.
    let (over, warned) = overshoot(1e-4, 1e-4);
    assert!(over > 0.1 && over < 0.6 && warned, "{over:e}");
}

#[test]
fn the_pll_of_a_converter_follows_a_frequency_step_like_its_linear_model() {
    // The grid has 50.2 Hz, the loop starts locked at 50 Hz. With the error
    // e = angle of the voltage - theta: e'' + Kp e' + Ki e = 0, e(0) = 0,
    // e'(0) = dw, so e = dw / wd e^{-sigma t} sin(wd t) and the frequency
    // of the loop is f + df (1 - e^{-sigma t} (cos(wd t) - sigma / wd
    // sin(wd t))), with wn = 2 pi pll_natural_frequency_hz,
    // sigma = pll_damping wn and wd = wn sqrt(1 - damping^2). The converter
    // shows e as v_q = sin(e) (the bus has 1 per unit).
    //
    // Tolerance: the largest e is dw / wn e^{-pi/4} / ... < dw / wn =
    // 0.0125; sin(e) differs from e by e^3 / 6 = 3.3e-7, and that
    // difference drives the loop with gains of its own order: 1e-6 allowed
    // on v_q, and Kp times that on the angular frequency.
    let (fn_hz, zeta, df) = (16.0, 0.6, 0.2);
    let wn = 2.0 * PI * fn_hz;
    let (sigma, wd) = (zeta * wn, wn * (1.0 - zeta * zeta).sqrt());
    let dw = 2.0 * PI * df;
    let model = on_infinite_bus(
        "2s-dirk",
        5e-5,
        0.2,
        "none",
        F_HZ + df,
        json!({"pll_natural_frequency_hz": fn_hz, "pll_damping": zeta,
            "reference": {"type": "current", "d_pu": 0.0}}),
        json!([]),
        &["v_q", "pll.frequency_hz", "v_d"],
    );
    let r = run(&model);
    let all = (0.0, 0.2);
    let e = r.error("v_q", all, |t| {
        dw / wd * (-sigma * t).exp() * (wd * t).sin()
    });
    assert!(e < 1e-6, "v_q: {e:e}");
    let e = r.error("pll.frequency_hz", all, |t| {
        F_HZ + df * (1.0 - (-sigma * t).exp() * ((wd * t).cos() - sigma / wd * (wd * t).sin()))
    });
    assert!(e < 2.0 * zeta * wn * 1e-6 / (2.0 * PI), "frequency: {e:e}");
    // Locked on the new frequency at the end.
    assert!((r.at("pll.frequency_hz", 0.2) - 50.2).abs() < 1e-6);
    assert!((r.at("v_d", 0.2) - 1.0).abs() < 1e-6);
}

/// The currents (i_d, i_q) and their references at the end of a run with
/// constant power references beyond the current limit.
fn limited(priority: &str, p_mw: f64, q_mvar: f64) -> [f64; 4] {
    let model = on_infinite_bus(
        "2s-dirk",
        5e-5,
        0.4,
        "none",
        F_HZ,
        json!({"priority": priority,
            "reference": {"type": "power", "active_power_mw": p_mw, "reactive_power_mvar": q_mvar}}),
        json!([]),
        &["i_d", "i_q", "i_ref.d", "i_ref.q"],
    );
    let r = run(&model);
    ["i_d", "i_q", "i_ref.d", "i_ref.q"].map(|n| r.at(n, 0.4))
}

#[test]
fn the_current_limit_keeps_the_current_with_priority() {
    // 1 per unit of active and 0.6 of reactive power at 1 per unit of
    // voltage ask for i = (1, -0.6), 1.166 per unit; the limit is 1.1.
    // Priority of the reactive current: i_q = -0.6 stays and i_d gets
    // sqrt(1.1^2 - 0.6^2) = 0.9220. Priority of the active current: i_d = 1
    // stays and i_q gets -sqrt(1.1^2 - 1) = -0.4583.
    //
    // The references are exact. The currents follow them up to what the
    // integral still lacks 0.4 s after the start from rest: the zero of the
    // controller cancels the pole of the reactor, so a deficit of the
    // integral (R i at most, 0.0055) decays with L / R = 95 ms and shows as
    // a current error of the deficit / Kp: 0.0055 e^{-0.4 / 0.095} / 0.48 =
    // 1.7e-4. 3e-4 is allowed.
    let rest = |a: f64| (1.1_f64 * 1.1 - a * a).sqrt();
    let [i_d, i_q, ref_d, ref_q] = limited("reactive", 2.0, 1.2);
    assert!((ref_q + 0.6).abs() < 1e-12 && (ref_d - rest(0.6)).abs() < 1e-12);
    assert!(
        (i_d - rest(0.6)).abs() < 3e-4 && (i_q + 0.6).abs() < 3e-4,
        "{i_d} {i_q}"
    );
    assert!((i_d.hypot(i_q) - 1.1).abs() < 3e-4);
    let [i_d, i_q, ref_d, ref_q] = limited("active", 2.0, 1.2);
    assert!((ref_d - 1.0).abs() < 1e-12 && (ref_q + rest(1.0)).abs() < 1e-12);
    assert!(
        (i_d - 1.0).abs() < 3e-4 && (i_q + rest(1.0)).abs() < 3e-4,
        "{i_d} {i_q}"
    );
    // Inside the limit nothing is changed: 0.5 and 0.3 per unit.
    let [i_d, i_q, ref_d, ref_q] = limited("reactive", 1.0, 0.6);
    assert!((ref_d - 0.5).abs() < 1e-12 && (ref_q + 0.3).abs() < 1e-12);
    assert!(
        (i_d - 0.5).abs() < 3e-4 && (i_q + 0.3).abs() < 3e-4,
        "{i_d} {i_q}"
    );
}

#[test]
fn the_current_leaves_its_limit_as_soon_as_the_reference_does() {
    // The active power reference is 1.5 per unit until 0.3 s (the current
    // reference is at the limit of 1.1) and 0.5 per unit after it. Nothing
    // has wound up while the limit held: from the grid point of the step on
    // the current follows the first-order lag from where it is (1.1, less
    // the 5e-4 that the integral still lacks 0.3 s after the start from
    // rest) to 0.5, within the dead time of at most two steps,
    // 0.6 (2 h / tau) / e (see the step response above).
    let (h, tau, t_step) = (5e-5, 1e-3, 0.3);
    let model = on_infinite_bus(
        "2s-dirk",
        h,
        0.32,
        "none",
        F_HZ,
        json!({"reference": {"type": "power", "active_power_mw": "p_cmd"}}),
        json!([{"type": "step", "name": "p_cmd", "time_s": t_step, "before": 3.0, "after": 1.0}]),
        &["i_d", "i_ref.d", "i_d_cmd"],
    );
    let r = run(&model);
    assert!((r.at("i_d_cmd", 0.29) - 1.5).abs() < 1e-9);
    assert!((r.at("i_ref.d", 0.29) - 1.1).abs() < 1e-12);
    let before = r.at("i_d", t_step);
    assert!((before - 1.1).abs() < 1e-3, "{before}");
    let e = r.error("i_d", (t_step, 0.32), |t| {
        0.5 + (before - 0.5) * (-(t - t_step) / tau).exp()
    });
    assert!(e < 0.6 * 2.0 * h / tau / 1f64.exp(), "{e:e}");
}

#[test]
fn the_voltage_limit_slows_a_large_step_without_an_overshoot() {
    // With the default limit of 1.2 per unit, a step of the d current from
    // 0 to 1 asks for 1 + Kp = 1.48 per unit of voltage. The source stays
    // at the limit, which leaves 0.2 per unit across the reactance: the
    // current rises by 0.2 omega / x = 419 per unit per second until the
    // controller comes back inside the limit. The integrals are held
    // meanwhile, so the current arrives without overshoot. What the
    // integral then lacks (R times the step, 0.005) is a current error of
    // 0.005 / Kp = 1 % that decays with L / R = 95 ms.
    let model = on_infinite_bus(
        "2s-dirk",
        5e-5,
        0.04,
        "steady_state",
        F_HZ,
        json!({"reference": {"type": "current", "d_pu": "id_cmd"},
            "operating_point": {"active_power_mw": 0.0}}),
        json!([{"type": "step", "name": "id_cmd", "time_s": 0.01, "before": 0.0, "after": 1.0}]),
        &["i_d", "e.d", "e.q"],
    );
    let r = run(&model);
    // At the limit right after the step.
    let e = r.at("e.d", 0.0105).hypot(r.at("e.q", 0.0105));
    assert!((e - 1.2).abs() < 1e-9, "{e}");
    // The slope while it is: between 0.5 and 1.0 ms after the step. The
    // voltage across the reactance is 0.2 less the small part of the limit
    // that the q axis takes and the resistive drop.
    let slope = (r.at("i_d", 0.0110) - r.at("i_d", 0.0105)) / 0.5e-3;
    let ideal = 0.2 * omega() / X_PU;
    assert!(
        slope < ideal && slope > 0.9 * ideal,
        "{slope} against {ideal}"
    );
    // Without the limit the current would be at 1 - e^{-1} = 0.63 after
    // 1 ms; with it, it is still on the slope.
    assert!(r.at("i_d", 0.011) < 0.45);
    // No overshoot, and the reference is reached.
    let (_, highest) = r.range("i_d", (0.0, 0.04));
    assert!(highest < 1.0 + 1e-3, "{highest}");
    let last = r.at("i_d", 0.04);
    assert!(
        last > 1.0 - R_PU / (X_PU / (omega() * 1e-3)) && last < 1.0,
        "{last}"
    );
}

/// The voltage (magnitude in per unit, angle in degrees) of a bus behind
/// `z_pu` (X/R = 10) from a source of 1 per unit, when p + j q per unit are
/// injected into it: the fixed point of v = 1 + z conj(s / v).
fn two_bus_flow(z_pu: f64, p: f64, q: f64) -> (f64, f64) {
    let x = z_pu / (1.0_f64 + 0.01).sqrt();
    let z = (x / 10.0, x);
    let mut v = (1.0, 0.0);
    for _ in 0..500 {
        let m = v.0 * v.0 + v.1 * v.1;
        // conj(s / v) = conj(s) v / |v|^2.
        let i = scale(mul((p, -q), v), 1.0 / m);
        v = add((1.0, 0.0), mul(z, i));
    }
    (v.0.hypot(v.1), v.1.atan2(v.0).to_degrees())
}

/// The converter behind a grid impedance of 1 / scr per unit, with more
/// components at will.
fn behind_grid(
    simulation: Value,
    scr: f64,
    fields: Value,
    more: Value,
    controls: Value,
    outputs: &[&str],
) -> Value {
    let mut components = vec![
        json!({"type": "three_phase_source", "name": "G1", "bus": "b0", "line_voltage_rms_v": V_KV * 1e3, "frequency_hz": F_HZ}),
        json!({"type": "coupled_rl", "name": "Z1", "buses": ["b0", "b1"], "impedance": impedance(1.0 / scr)}),
        converter(fields),
    ];
    components.extend(more.as_array().unwrap().iter().cloned());
    json!({"format": "openkeito-model", "version": "0.5", "simulation": simulation,
        "components": components, "controls": controls, "outputs": signals(outputs)})
}

/// Largest departures of p, q and v_d from the operating point (p, q per
/// unit) over the first and the second half of a run of 1 s without a
/// disturbance, and the relative departure from the phasor solution.
fn held(method: &str, h: f64, scr: f64, p: f64, q: f64) -> ([f64; 3], [f64; 3], f64) {
    let (v, angle) = two_bus_flow(1.0 / scr, p, q);
    let model = behind_grid(
        json!({"time_step_s": h, "end_time_s": 1.0, "method": method, "initialization": "steady_state"}),
        scr,
        json!({"reference": {"type": "power", "active_power_mw": p * S_MVA, "reactive_power_mvar": q * S_MVA},
            "operating_point": {"active_power_mw": p * S_MVA, "reactive_power_mvar": q * S_MVA,
                "voltage_kv": v * V_KV, "angle_deg": angle}}),
        json!([]),
        json!([]),
        &["p", "q", "v_d", "pll.frequency_hz"],
    );
    let r = run(&model);
    assert!(r.warning_codes().is_empty(), "{:?}", r.stats.warnings);
    let departures = |span: (f64, f64)| {
        [("p", p), ("q", q), ("v_d", v)].map(|(name, value)| r.error(name, span, |_| value))
    };
    let report = r.stats.initialization.as_ref().expect("steady state");
    let relative = report
        .relative_voltage_deviation
        .max(report.relative_current_deviation.unwrap_or(0.0));
    (departures((0.0, 0.5)), departures((0.5, 1.0)), relative)
}

#[test]
fn a_run_from_a_power_flow_stays_at_its_operating_point() {
    // A strong grid (short-circuit ratio 20, 0.75 + j 0.15 per unit) and a
    // weak one (ratio 3, 0.9 per unit): the operating point of the power
    // flow, given to the converter, is where the run starts and stays.
    //
    // What moves it at the start is the difference between the continuous
    // steady state and that of the discrete loop: the chord of the source,
    // which follows its signal in straight lines, (omega h)^2 / 8 = 3.1e-5
    // of the voltage behind the reactor at 50 us, seen by the stages of
    // 2S-DIRK within a step (the trapezoidal rule looks at grid points only
    // and sees a tenth of it). The integrals of the current control take
    // it up with L / R = 95 ms, so the departure does not drift: it is
    // below 5e-5 in the first half second (measured 2.5e-5 at most) and a
    // hundred times smaller in the second.
    for (scr, p, q) in [(20.0, 0.75, 0.15), (3.0, 0.9, 0.0)] {
        for method in ["2s-dirk", "trapezoidal"] {
            let (first, second, relative) = held(method, 5e-5, scr, p, q);
            for k in 0..3 {
                assert!(first[k] < 5e-5, "{method} scr {scr}: {first:?}");
                // The voltage keeps the offset of the discrete steady
                // state, below the chord (measured 4.6e-6 and 1.2e-5).
                let bound = if k == 2 { 2e-5 } else { 5e-7 };
                assert!(second[k] < bound, "{method} scr {scr}: {second:?}");
            }
            // The waveforms against the phasor solution: the chord.
            assert!(relative < 4e-5, "{method} scr {scr}: {relative:e}");
            // Second order: a quarter at half the step.
            let (fine, _, _) = held(method, 2.5e-5, scr, p, q);
            let worst = |x: &[f64; 3]| x.iter().cloned().fold(0.0, f64::max);
            let ratio = worst(&fine) / worst(&first);
            assert!(ratio < 0.4, "{method} scr {scr}: {fine:?} {first:?}");
        }
    }
}

#[test]
fn a_reference_away_from_the_operating_point_is_followed_from_the_start() {
    // The model gives no operating point: the phasor solution takes the
    // references at the rated voltage and the angle 0, which is not the
    // voltage of the bus behind the grid impedance. The run starts 0.03
    // per unit of reactive power away and settles on the references.
    let model = behind_grid(
        json!({"time_step_s": 5e-5, "end_time_s": 0.6, "initialization": "steady_state"}),
        20.0,
        json!({"reference": {"type": "power", "active_power_mw": 1.5, "reactive_power_mvar": 0.3}}),
        json!([]),
        json!([]),
        &["p", "q"],
    );
    let r = run(&model);
    assert!((r.at("q", 0.0) - 0.15).abs() > 0.02);
    assert!((r.at("p", 0.6) - 0.75).abs() < 1e-4);
    assert!((r.at("q", 0.6) - 0.15).abs() < 1e-4);
    // The departure from the phasor solution is reported.
    assert!(r.warning_codes().contains(&"W220"));
}

/// The converter (0.8 per unit) behind a grid of short-circuit ratio 10,
/// with a three-phase fault through 0.035 per unit of resistance at its
/// terminals from 0.1 to 0.25 s. The fault opens at the current zeros of
/// its phases.
fn fault_case(h: f64, switching: &str, priority: &str, q: f64) -> Run {
    let (p, scr) = (0.8, 10.0);
    let (v, angle) = two_bus_flow(1.0 / scr, p, q);
    let model = behind_grid(
        json!({"time_step_s": h, "end_time_s": 0.6, "initialization": "steady_state", "switching": switching}),
        scr,
        json!({"priority": priority,
            "reference": {"type": "power", "active_power_mw": p * S_MVA, "reactive_power_mvar": q * S_MVA},
            "operating_point": {"active_power_mw": p * S_MVA, "reactive_power_mvar": q * S_MVA,
                "voltage_kv": v * V_KV, "angle_deg": angle}}),
        json!([
            {"type": "fault", "name": "F1", "bus": "b1", "phases": ["a", "b", "c"], "ground": true,
             "fault_resistance_ohm": 0.035 * V_KV * V_KV / S_MVA,
             "start_time_s": 0.1, "clear_time_s": 0.25},
        ]),
        json!([]),
        &[
            "p", "q", "v_d", "i_d", "i_q", "i_ref.d", "i_ref.q", "i_d_cmd",
        ],
    );
    run(&model)
}

#[test]
fn a_fault_at_the_terminals_holds_the_current_at_its_limit() {
    // During the fault the voltage of the terminals is what the fault
    // resistance takes of the grid voltage, 0.035 / |0.01 + j 0.0995 +
    // 0.035| = 0.32 per unit, a little more with the converter feeding in:
    // 0.35. The power reference then asks for 0.8 / 0.35 = 2.3 per unit of
    // current: the reference is at the limit of 1.1, and the current
    // follows it (measured: within 2.4e-4).
    for switching in ["grid", "interpolated"] {
        let r = fault_case(5e-5, switching, "reactive", 0.0);
        let during = (0.16, 0.25);
        let (lowest, highest) = r.range("v_d", during);
        assert!(
            lowest > 0.34 && highest < 0.36,
            "{switching}: {lowest} {highest}"
        );
        assert!(r.at("i_d_cmd", 0.2) > 2.2);
        assert!((r.at("i_ref.d", 0.2) - 1.1).abs() < 1e-12);
        let e = r.error("i_d", during, |_| 1.1);
        assert!(e < 1e-3, "{switching}: {e:e}");
        // Never far above the limit, also not when the fault starts (the
        // feedforward follows the voltage one step late: a jump of 0.65 per
        // unit of voltage for one step moves the current by
        // 0.65 omega h / x = 0.07 at most) and ends.
        let highest = r
            .column("i_d")
            .iter()
            .zip(r.column("i_q"))
            .map(|(d, q)| d.hypot(q))
            .fold(0.0, f64::max);
        assert!(highest < 1.1 + 0.07, "{switching}: {highest}");
        // Back at the operating point: the voltage filter of 20 ms, the
        // phase-locked loop and the integrals (95 ms) have settled 0.35 s
        // after the fault.
        assert!((r.at("p", 0.6) - 0.8).abs() < 5e-4, "{switching}");
        assert!(r.at("q", 0.6).abs() < 5e-4, "{switching}");
        let events: Vec<&str> = r.stats.events.iter().map(|e| e.action).collect();
        assert_eq!(events, ["close", "close", "close", "open", "open", "open"]);
    }
}

#[test]
fn the_priority_decides_what_is_left_during_a_fault() {
    // 0.8 per unit of active and 0.3 of reactive power at 0.36 per unit of
    // voltage ask for i = (2.2, -0.83). With priority of the reactive
    // current it stays and the active current gets what the limit leaves;
    // with priority of the active current that takes the whole limit.
    let r = fault_case(5e-5, "grid", "reactive", 0.3);
    let i_q = r.at("i_ref.q", 0.2);
    // 0.1 s into the fault, the 1-step measurement delay and PLL dynamics leave
    // v_d within < 1% of its settled value, so i_ref.q agrees to 0.01 pu.
    assert!((i_q + 0.3 / r.at("v_d", 0.2)).abs() < 0.01, "{i_q}");
    assert!((r.at("i_ref.d", 0.2) - (1.21 - i_q * i_q).sqrt()).abs() < 1e-12);
    let measured = r.at("i_q", 0.2);
    // 0.1 s into the fault the integral of the q axis still lacks a part
    // of the new R i_q: an error of R 0.55 / Kp e^{-0.1 / 0.095} = 2e-3.
    assert!((measured - i_q).abs() < 5e-3, "{measured} {i_q}");
    let r = fault_case(5e-5, "grid", "active", 0.3);
    assert!((r.at("i_ref.d", 0.2) - 1.1).abs() < 1e-12);
    assert!(r.at("i_ref.q", 0.2).abs() < 1e-12);
    assert!((r.at("i_d", 0.2) - 1.1).abs() < 2e-3);
}

#[test]
fn the_fault_response_converges_with_the_time_step() {
    // No closed form: the runs at 100, 50 and 25 us are compared with the
    // one at 12.5 us at the grid points of each, with the switching
    // instants placed within the steps. Right at the instants the voltage
    // jumps and the rows differ by the jump while the dead time of the
    // controls (the step itself) passes, so the largest difference is taken
    // away from them (5 ms after the start of the fault, 25 ms after the
    // clearing command, in which the three phases open), and the mean over
    // the whole run. Both shrink by more than a half per halving, between
    // first order (the dead time) and second (the integration); measured
    // for i_d: 8.2e-3, 3.4e-3, 1.1e-3 away from the instants and 1.8e-3,
    // 6.9e-4, 1.9e-4 in the mean.
    let reference = fault_case(1.25e-5, "interpolated", "reactive", 0.0);
    let difference = |h: f64| {
        let r = fault_case(h, "interpolated", "reactive", 0.0);
        let every = (h / 1.25e-5).round() as usize;
        let (mut away, mut sum, mut count) = (0.0_f64, 0.0, 0);
        for name in ["i_d", "i_q", "v_d"] {
            let (a, b) = (r.column(name), reference.column(name));
            for k in 0..a.len() {
                let (t, d) = (r.t[k], (a[k] - b[k * every]).abs());
                let near = (0.0995..0.105).contains(&t) || (0.2495..0.275).contains(&t);
                if !near {
                    away = away.max(d);
                }
                sum += d;
                count += 1;
            }
        }
        (away, sum / count as f64)
    };
    let (coarse, middle, fine) = (difference(1e-4), difference(5e-5), difference(2.5e-5));
    for (a, b) in [(coarse, middle), (middle, fine)] {
        assert!(b.0 < 0.5 * a.0, "away from the instants: {a:?} {b:?}");
        assert!(b.1 < 0.5 * a.1, "mean: {a:?} {b:?}");
    }
    assert!(fine.0 < 2e-3 && fine.1 < 2e-4, "{fine:?}");
}

#[test]
fn a_converter_beside_a_switching_circuit_keeps_its_response() {
    // A circuit with a switch driven by a carrier of 2.3 kHz stands beside
    // the converter, without a connection. With the switching instants
    // within the steps, most steps are solved in parts. The source of the
    // converter stays on its straight lines and the controls on the grid
    // points, so the step response of the current is the one without the
    // switch. With switching at grid points it is the same to rounding;
    // with the instants within the steps the reactor is integrated over
    // the parts of a step instead of the whole, which changes the result
    // by less than the integration error itself (5e-6 at 50 us, see the
    // step response above): measured 7.3e-7.
    let chopper = json!([
        {"type": "voltage_source", "name": "E9", "nodes": ["c1", "gnd"], "waveform": {"type": "dc", "value": 100.0}},
        {"type": "controlled_switch", "name": "S9", "nodes": ["c1", "c2"], "gate": "pwm9"},
        {"type": "resistor", "name": "R9", "nodes": ["c2", "c3"], "resistance_ohm": 5.0},
        {"type": "inductor", "name": "L9", "nodes": ["c3", "gnd"], "inductance_h": 1e-3},
        {"type": "resistor", "name": "R8", "nodes": ["c2", "gnd"], "resistance_ohm": 1e3},
    ]);
    let pwm = json!([
        {"type": "step", "name": "id_cmd", "time_s": 0.01, "before": 0.2, "after": 0.8},
        {"type": "constant", "name": "duty", "value": 0.37},
        {"type": "triangle_wave", "name": "carrier9", "frequency_hz": 2300.0},
        {"type": "comparator", "name": "pwm9", "inputs": ["duty", "carrier9"]},
    ]);
    let case = |switching: &str, with_chopper: bool| {
        let mut model = on_infinite_bus(
            "2s-dirk",
            5e-5,
            0.03,
            "none",
            F_HZ,
            json!({"dc_voltage_kv": 60.0, "reference": {"type": "current", "d_pu": "id_cmd"}}),
            pwm.clone(),
            &["i_d", "i_q"],
        );
        model["simulation"]["switching"] = json!(switching);
        if with_chopper {
            let c = model["components"].as_array_mut().unwrap();
            c.extend(chopper.as_array().unwrap().iter().cloned());
        }
        run(&model)
    };
    let alone = case("grid", false);
    for (switching, bound) in [("grid", 1e-9), ("interpolated", 5e-6)] {
        let beside = case(switching, true);
        if switching == "interpolated" {
            assert!(
                beside.stats.split_steps > 100,
                "{}",
                beside.stats.split_steps
            );
        }
        for name in ["i_d", "i_q"] {
            let (a, b) = (alone.column(name), beside.column(name));
            let worst = (0..a.len())
                .map(|k| (a[k] - b[k]).abs())
                .fold(0.0, f64::max);
            assert!(worst < bound, "{switching} {name}: {worst:e}");
        }
    }
}

/// The control blocks of a converter named INV with `fields`, as the solver
/// builds them, with its measurements replaced by those of a bridge: the
/// voltages of the bus `bus` and the currents of the reactor `reactor`.
fn bridge_controls(h: f64, fields: &Value, bus: &str, reactor: &str) -> Vec<Value> {
    let mut spec = converter(fields.clone());
    spec["name"] = json!("INV");
    spec.as_object_mut().unwrap().remove("type");
    let spec: openkeito_model::GridFollowingConverter = serde_json::from_value(spec).unwrap();
    let mut blocks: Vec<Value> = spec
        .controls(h)
        .iter()
        .map(|c| serde_json::to_value(c).unwrap())
        .collect();
    for b in blocks.iter_mut() {
        let name = b["name"].as_str().unwrap().to_string();
        if let Some(phase) = name
            .strip_prefix("INV.v_")
            .filter(|p| ["a", "b", "c"].contains(p))
        {
            b["quantity"] = json!({"type": "node_voltage", "node": format!("{bus}.{phase}")});
        } else if name == "INV.i_abc" {
            b["quantity"] = json!({"type": "branch_current", "component": reactor});
        }
    }
    blocks
}

#[test]
fn the_averaged_model_gives_the_fundamental_of_a_switching_bridge() {
    // Two converters with the same control, each on its own infinite bus,
    // in one run. PV1 is the averaged model. INV is a two-level bridge of
    // six switches between +-6.5 kV, whose gates compare the voltage
    // references of the same control blocks (`bridge_controls`) with a
    // carrier of 5 kHz, behind the same reactor. The reference of the d
    // current steps from 0.3 to 0.8 at 20 ms.
    //
    // The current of the bridge is that of the averaged model plus the
    // ripple of the carrier. A leg switches between +-Vdc/2 = +-1.206 per
    // unit; against a reference near 1 per unit that leaves a triangle of
    // at most (Vdc/2) omega / x / (8 fc) = 0.063 per unit peak in a phase.
    // Averaged over one carrier period (20 rows), the ripple is gone and
    // the two models agree within a fifth of it: measured 8.1e-3 of the
    // rated current on the d axis. What is left is the ripple that the
    // controls of the bridge sample at their grid points, which the
    // averaged model does not have.
    let (h, fc) = (1e-5, 5000.0);
    let fields = json!({"dc_voltage_kv": 13.0, "reference": {"type": "current", "d_pu": "id_cmd"}});
    let (r_ohm, l_h) = {
        let z_base = V_KV * V_KV / S_MVA;
        (R_PU * z_base, X_PU * z_base / omega())
    };
    let mut components = vec![
        json!({"type": "three_phase_source", "name": "G1", "bus": "b1", "line_voltage_rms_v": V_KV * 1e3, "frequency_hz": F_HZ}),
        converter(fields.clone()),
        json!({"type": "three_phase_source", "name": "G2", "bus": "b2", "line_voltage_rms_v": V_KV * 1e3, "frequency_hz": F_HZ}),
        json!({"type": "voltage_source", "name": "EP", "nodes": ["dcp", "gnd"], "waveform": {"type": "dc", "value": 6500.0}}),
        json!({"type": "voltage_source", "name": "EN", "nodes": ["dcn", "gnd"], "waveform": {"type": "dc", "value": -6500.0}}),
        // The reactor of the averaged model, which carries no zero
        // sequence (1000 times the impedance).
        json!({"type": "coupled_rl", "name": "LF", "buses": ["sw", "b2"], "impedance":
            {"type": "sequence", "r1_ohm": r_ohm, "l1_h": l_h, "r0_ohm": 1e3 * r_ohm, "l0_h": 1e3 * l_h}}),
    ];
    let mut controls = vec![
        json!({"type": "step", "name": "id_cmd", "time_s": 0.02, "before": 0.3, "after": 0.8}),
        json!({"type": "triangle_wave", "name": "carrier", "frequency_hz": fc, "minimum": -1.0, "maximum": 1.0}),
    ];
    controls.extend(bridge_controls(h, &fields, "b2", "LF"));
    for ph in ["a", "b", "c"] {
        components.push(
            json!({"type": "controlled_switch", "name": format!("SP{ph}"),
            "nodes": ["dcp", format!("sw.{ph}")], "gate": format!("gp_{ph}")}),
        );
        components.push(
            json!({"type": "controlled_switch", "name": format!("SN{ph}"),
            "nodes": [format!("sw.{ph}"), "dcn"], "gate": format!("gn_{ph}")}),
        );
        controls.push(json!({"type": "gain", "name": format!("m_{ph}"), "input": format!("INV.e_abc.{ph}"), "gain": 1.0 / 6500.0}));
        controls.push(json!({"type": "comparator", "name": format!("gp_{ph}"), "inputs": [format!("m_{ph}"), "carrier"]}));
        controls.push(json!({"type": "comparator", "name": format!("gn_{ph}"), "inputs": ["carrier", format!("m_{ph}")]}));
    }
    let model = json!({"format": "openkeito-model", "version": "0.5",
        "simulation": {"time_step_s": h, "end_time_s": 0.06, "switching": "interpolated"},
        "components": components, "controls": controls,
        "outputs": [
            {"type": "signal", "signal": "PV1.i_d"}, {"type": "signal", "signal": "PV1.i_q"},
            {"type": "signal", "signal": "INV.i_d"}, {"type": "signal", "signal": "INV.i_q"}]});
    let r = run(&model);
    let column = |name: &str| {
        let k = r.columns.iter().position(|n| n == name).unwrap();
        r.rows.iter().map(|row| row[k]).collect::<Vec<f64>>()
    };
    let window = (1.0 / (fc * h)).round() as usize;
    let averaged = |x: &[f64]| -> Vec<f64> {
        (window..x.len())
            .map(|k| x[k - window..k].iter().sum::<f64>() / window as f64)
            .collect()
    };
    let ripple = 6500.0 / (V_KV * 1e3 * (2.0_f64 / 3.0).sqrt()) * omega() / X_PU / (8.0 * fc);
    for axis in ["i_d", "i_q"] {
        let (average, bridge) = (
            column(&format!("S(PV1.{axis})")),
            column(&format!("S(INV.{axis})")),
        );
        // Row by row: within the ripple (in the rotating frame the ripples
        // of the phases add up to at most 2/sqrt(3) of one).
        let raw = (0..bridge.len())
            .map(|k| (bridge[k] - average[k]).abs())
            .fold(0.0, f64::max);
        // Averaged over a carrier period.
        let (a, b) = (averaged(&average), averaged(&bridge));
        let mean = (0..a.len())
            .map(|k| (a[k] - b[k]).abs())
            .fold(0.0, f64::max);
        assert!(raw < 2.0 / 3.0_f64.sqrt() * ripple * 1.2, "{axis}: {raw:e}");
        assert!(raw > 0.2 * ripple, "{axis}: there is no ripple: {raw:e}");
        assert!(mean < 0.2 * ripple, "{axis}: {mean:e}");
    }
    // The bridge switched at its carrier: about 2 changes per period and
    // phase, each of two switches.
    let changes = r.stats.events.len() as f64;
    assert!(
        (changes / (0.06 * fc * 12.0) - 1.0).abs() < 0.05,
        "{changes}"
    );
}

#[test]
fn the_parameters_and_names_of_a_converter_are_checked() {
    let model = |fields: Value, controls: Value, initialization: &str, h: f64| {
        json!({"format": "openkeito-model", "version": "0.5",
            "simulation": {"time_step_s": h, "end_time_s": 0.01, "initialization": initialization},
            "components": [
                {"type": "three_phase_source", "name": "G1", "bus": "b1", "line_voltage_rms_v": 6600.0, "frequency_hz": 50.0},
                converter(fields),
            ],
            "controls": controls})
    };
    let power = json!({"reference": {"type": "power", "active_power_mw": 1.0}});
    // The defaults are a valid model, also from the steady state.
    assert!(codes(&model(power.clone(), json!([]), "steady_state", 5e-5)).is_empty());

    // Values out of range.
    for (field, value) in [
        ("rated_power_mva", json!(0.0)),
        ("rated_voltage_kv", json!(-6.6)),
        ("rated_frequency_hz", json!(0.0)),
        ("reactor_resistance_pu", json!(0.0)),
        ("reactor_reactance_pu", json!(-0.1)),
        ("current_limit_pu", json!(0.0)),
        ("current_time_constant_s", json!(0.0)),
        ("pll_natural_frequency_hz", json!(0.0)),
        ("pll_damping", json!(0.0)),
        ("voltage_feedforward_time_constant_s", json!(-1e-3)),
        ("dc_voltage_kv", json!(0.0)),
        ("modulation_limit", json!(0.0)),
        (
            "operating_point",
            json!({"active_power_mw": 1.0, "voltage_kv": 0.0}),
        ),
        (
            "reference",
            json!({"type": "power", "active_power_mw": 1.0, "minimum_voltage_pu": 0.0}),
        ),
        (
            "reference",
            json!({"type": "power", "active_power_mw": 1.0, "voltage_filter_time_constant_s": -1.0}),
        ),
    ] {
        let mut fields = power.clone();
        fields[field] = value;
        let d = diagnostics(&model(fields, json!([]), "none", 5e-5));
        assert_eq!(d.len(), 1, "{field}: {d:#?}");
        assert_eq!(d[0].code, "E130", "{field}");
        assert!(
            d[0].path.starts_with(&format!("components[1].{field}")),
            "{}",
            d[0].path
        );
    }
    // An unknown field does not parse.
    let mut fields = power.clone();
    fields["current_limit"] = json!(1.1);
    let d = diagnostics(&model(fields, json!([]), "none", 5e-5));
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].severity, openkeito_model::Severity::Error);

    // A reference names a signal that must exist; the signals of the
    // converter itself exist for the blocks of the model.
    let by_signal = json!({"reference": {"type": "power", "active_power_mw": "p_cmd"},
        "operating_point": {"active_power_mw": 1.0}});
    let d = diagnostics(&model(by_signal.clone(), json!([]), "none", 5e-5));
    assert_eq!(d[0].code, "E170");
    assert_eq!(d[0].path, "components[1].reference.active_power_mw");
    let controls = json!([
        {"type": "gain", "name": "p_cmd", "input": "PV1.v_d", "gain": 1.0},
        {"type": "sum", "name": "x", "inputs": ["PV1.pll.frequency_hz", "PV1.i_ref.q"], "signs": [1.0, 1.0]},
    ]);
    assert!(codes(&model(by_signal.clone(), controls, "none", 5e-5)).is_empty());
    let d = diagnostics(&model(
        by_signal.clone(),
        json!([{"type": "gain", "name": "p_cmd", "input": "PV1.pll", "gain": 1.0}]),
        "none",
        5e-5,
    ));
    assert_eq!(d[0].code, "E170");
    assert!(d[0].hint.contains("PV1.pll.theta"), "{}", d[0].hint);

    // A block of the model must not take a name of the blocks of the
    // converter.
    let d = diagnostics(&model(
        power.clone(),
        json!([{"type": "constant", "name": "PV1.x", "value": 1.0}]),
        "none",
        5e-5,
    ));
    assert_eq!(d[0].code, "E290");
    assert_eq!(d[0].path, "controls[0].name");

    // A loop without a state through the blocks of the converter is found:
    // the current reference from the limited current reference.
    let d = diagnostics(&model(
        json!({"reference": {"type": "current", "d_pu": "loop"}}),
        json!([{"type": "gain", "name": "loop", "input": "PV1.i_ref.d", "gain": 0.5}]),
        "none",
        5e-5,
    ));
    assert_eq!(d[0].code, "E171");
    assert!(d[0].message.contains("PV1.i_d_cmd"), "{}", d[0].message);

    // A time step too long for the current control.
    let d = diagnostics(&model(power.clone(), json!([]), "none", 5e-4));
    assert_eq!(d.iter().map(|d| d.code).collect::<Vec<_>>(), ["W290"]);
    assert_eq!(d[0].severity, openkeito_model::Severity::Warning);
    // No operating point for the steady state.
    let d = diagnostics(&model(
        json!({"reference": {"type": "power", "active_power_mw": "p_cmd"}}),
        json!([{"type": "constant", "name": "p_cmd", "value": 1.0}]),
        "steady_state",
        5e-5,
    ));
    let found: Vec<&str> = d.iter().map(|d| d.code).collect();
    assert!(found.contains(&"W291"), "{found:?}");
    // An operating point that is not the reference.
    let mut fields = power.clone();
    fields["operating_point"] = json!({"active_power_mw": 0.5});
    assert_eq!(
        codes(&model(fields, json!([]), "steady_state", 5e-5)),
        ["W292"]
    );
    // A dc voltage below the voltage of the bus.
    let mut fields = power.clone();
    fields["dc_voltage_kv"] = json!(9.0);
    assert_eq!(codes(&model(fields, json!([]), "none", 5e-5)), ["W293"]);
    // An older version does not have the converter.
    let mut old = model(power, json!([]), "none", 5e-5);
    old["version"] = json!("0.4");
    assert_eq!(codes(&old), ["E102"]);
}

#[test]
fn the_signals_of_a_converter_are_recorded_on_request_only() {
    // Without outputs: the node voltages and the currents of the
    // components, not the signals of the blocks of the converter.
    let mut model = on_infinite_bus(
        "2s-dirk",
        5e-5,
        0.001,
        "steady_state",
        F_HZ,
        json!({"reference": {"type": "power", "active_power_mw": 1.0}}),
        json!([]),
        &[],
    );
    model.as_object_mut().unwrap().remove("outputs");
    model.as_object_mut().unwrap().remove("controls");
    let r = run(&model);
    assert_eq!(
        r.columns,
        [
            "V(b1.a)", "V(b1.b)", "V(b1.c)", "I(G1.a)", "I(G1.b)", "I(G1.c)", "I(PV1.a)",
            "I(PV1.b)", "I(PV1.c)"
        ]
    );
    // The currents of the converter flow into the bus, those of the source
    // from the bus into the source: with nothing else at the bus they are
    // equal.
    for k in 0..3 {
        for row in &r.rows {
            assert!((row[3 + k] - row[6 + k]).abs() < 1e-9);
        }
    }
    // 1 MW at 6.6 kV: 87.5 A rms, in phase with the voltage. Grid points at
    // h = 50 us sit within <= h/2 of the sine peak (~4e-3 A sampling offset)
    // plus (omega h)^2 (~3e-5) method error: < 0.01 A.
    let peak = r.rows.iter().map(|row| row[6].abs()).fold(0.0, f64::max);
    let expected = 1e6 / (3.0_f64.sqrt() * 6600.0) * 2.0_f64.sqrt();
    assert!((peak - expected).abs() < 0.01, "{peak}");
    assert!(r.rows[0][0] * r.rows[0][6] > 0.0);
}
