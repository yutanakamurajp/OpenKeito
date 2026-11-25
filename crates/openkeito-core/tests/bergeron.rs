//! The travelling-wave line (`bergeron_line`) against closed-form solutions.
//!
//! Tolerances. A line between sources and resistors has no state that is
//! integrated: every value is a sum of products of earlier values. With a
//! travel time of a whole number of time steps nothing is interpolated, and
//! the run agrees with the lattice diagram to rounding (checked at 1e-12 of
//! the source voltage; the condition estimates are below 1e4). Phasor
//! solutions agree with the ABCD parameters of the same circuit to rounding
//! too (1e-12). A travel time between grid points is interpolated linearly:
//! the error of a sine is w (1 - w)/2 (omega h)^2 per reading for a place w
//! between two samples a step apart, and a front is spread over a step.
//! Where a tolerance is not one of these, the test derives it.

mod common;

use std::f64::consts::PI;

use common::phasors::{F, METHODS, W};
use common::{run_sim, Run};
use openkeito_core::{Complex64, FrequencyScanner, Simulator};
use openkeito_model::{line_modes, LineParameters};
use serde_json::{json, Value};

impl Run {
    fn series(&self, name: &str) -> Vec<f64> {
        let k = self.col(name);
        self.rows.iter().map(|row| row[k]).collect()
    }

    fn departure(&self) -> f64 {
        self.stats
            .initialization
            .as_ref()
            .expect("initialized run")
            .relative_voltage_deviation
    }
}

fn run(model: &Value) -> Run {
    let loaded = openkeito_model::load_str(&model.to_string())
        .unwrap_or_else(|e| panic!("model is valid: {e:#?}"));
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    let sim = Simulator::from_model(&loaded.model).expect("circuit builds");
    let out = run_sim(sim, loaded.model.simulation.time_step_s);
    assert!(out.stats.warnings.is_empty(), "{:?}", out.stats.warnings);
    out
}

fn model(method: &str, h: f64, end: f64, init: &str, components: Value, outputs: Value) -> Value {
    json!({
        "format": "openkeito-model",
        "version": "0.5",
        "simulation": {"time_step_s": h, "end_time_s": end, "method": method, "initialization": init},
        "components": components,
        "outputs": outputs,
    })
}

/// Zc = 400 ohm and 4 us/km in every mode: the three phases are three
/// separate lines.
const ZC: f64 = 400.0;
const S_PER_KM: f64 = 4e-6;

fn uniform(r_ohm_per_km: f64) -> Value {
    json!({"type": "sequence",
        "r1_ohm_per_km": r_ohm_per_km, "l1_h_per_km": 1.6e-3, "c1_f_per_km": 1e-8,
        "r0_ohm_per_km": r_ohm_per_km, "l0_h_per_km": 1.6e-3, "c0_f_per_km": 1e-8})
}

/// An overhead line by its sequence values: Z1 = 266.2 ohm with 3.381 us/km,
/// Z0 = 654.7 ohm with 4.583 us/km.
const R1: f64 = 0.03;
const L1: f64 = 0.9e-3;
const C1: f64 = 12.7e-9;
const R0: f64 = 0.2;
const L0: f64 = 3e-3;
const C0: f64 = 7e-9;

fn overhead(r1: f64, r0: f64) -> Value {
    json!({"type": "sequence", "r1_ohm_per_km": r1, "l1_h_per_km": L1, "c1_f_per_km": C1,
        "r0_ohm_per_km": r0, "l0_h_per_km": L0, "c0_f_per_km": C0})
}

fn line(name: &str, from: Value, to: Value, length_km: f64, per_km: Value) -> Value {
    json!({"type": "bergeron_line", "name": name, "buses": [from, to],
        "length_km": length_km, "per_km": per_km})
}

fn dc(name: &str, node: &str, value: f64) -> Value {
    json!({"type": "voltage_source", "name": name, "nodes": [node, "gnd"],
        "waveform": {"type": "dc", "value": value}})
}

fn resistor(name: &str, a: &str, b: &str, ohm: f64) -> Value {
    json!({"type": "resistor", "name": name, "nodes": [a, b], "resistance_ohm": ohm})
}

fn node(n: &str) -> Value {
    json!({"type": "node_voltage", "node": n})
}

fn current(c: &str) -> Value {
    json!({"type": "branch_current", "component": c})
}

/// Number of travel times `tau` that have passed at `t`, where a front that
/// arrives at a grid point is in its row.
fn passed(t: f64, tau: f64) -> i64 {
    (t / tau + 1e-9).floor() as i64
}

const E: f64 = 100.0;

/// Largest |got - expected(t)| over the rows.
fn worst(run: &Run, name: &str, expected: impl Fn(f64) -> f64) -> f64 {
    run.t
        .iter()
        .zip(run.series(name))
        .map(|(t, v)| (v - expected(*t)).abs())
        .fold(0.0, f64::max)
}

// ------------------------------------------------------ single-mode lines

#[test]
fn an_open_line_doubles_the_step_with_a_period_of_four_travel_times() {
    // A dc source of E switched onto a lossless line at t = 0 (25 km: tau =
    // 100 us = 10 steps), open at the far end. The source reflects with -1
    // and the open end with +1: the far end is 2 E from tau to 3 tau, 0 from
    // 3 tau to 5 tau, and so on; the current into the line at the source is
    // E/Zc for two travel times, -E/Zc for the next two.
    let tau = 25.0 * S_PER_KM;
    for method in METHODS {
        let r = run(&model(
            method,
            10e-6,
            12.0 * tau,
            "none",
            json!([
                dc("E1", "s.a", E),
                line("L1", json!("s"), json!("r"), 25.0, uniform(0.0))
            ]),
            json!([node("r.a"), node("r.b"), node("s.b"), current("L1")]),
        ));
        let far = |t: f64| {
            if ((passed(t, tau) + 1) / 2) % 2 == 1 {
                2.0 * E
            } else {
                0.0
            }
        };
        let into = |t: f64| {
            if passed(t, 2.0 * tau) % 2 == 0 {
                E / ZC
            } else {
                -E / ZC
            }
        };
        let errors = [
            worst(&r, "V(r.a)", far) / E,
            worst(&r, "I(L1.0.a)", into) * ZC / E,
            // Nothing flows into the open end, and the other phases stay at
            // rest: with equal modes the phases do not couple.
            worst(&r, "I(L1.1.a)", |_| 0.0) * ZC / E,
            worst(&r, "V(r.b)", |_| 0.0) / E,
            worst(&r, "V(s.b)", |_| 0.0) / E,
            worst(&r, "I(L1.0.b)", |_| 0.0) * ZC / E,
        ];
        eprintln!("{method}: open line, errors {errors:?}");
        assert!(errors.iter().all(|e| *e < 1e-12), "{method}: {errors:?}");
        // The first row is the solution at t = 0+: the wave has left.
        assert!((r.rows[0][r.col("I(L1.0.a)")] - E / ZC).abs() < 1e-15);
        // The front is in the row of tau, not before.
        assert!((r.series("V(r.a)")[10] - 2.0 * E).abs() < 1e-12 * E);
        assert_eq!(r.series("V(r.a)")[9], 0.0);
    }
}

#[test]
fn a_matched_line_only_delays() {
    // Source resistance 100 ohm, load Zc: the sending end is at
    // E Zc/(Rs + Zc) = 80 V from t = 0+ on, the load one travel time later,
    // and nothing comes back.
    let tau = 25.0 * S_PER_KM;
    for method in METHODS {
        let r = run(&model(
            method,
            10e-6,
            6.0 * tau,
            "none",
            json!([
                dc("E1", "e", E),
                resistor("Rs", "e", "s.a", 100.0),
                line("L1", json!("s"), json!("r"), 25.0, uniform(0.0)),
                resistor("Rl", "r.a", "gnd", ZC),
            ]),
            json!([node("s.a"), node("r.a"), current("Rl"), current("L1")]),
        ));
        let v0 = E * ZC / (100.0 + ZC);
        let load = |t: f64| if passed(t, tau) >= 1 { v0 } else { 0.0 };
        let errors = [
            worst(&r, "V(s.a)", |_| v0) / E,
            worst(&r, "V(r.a)", load) / E,
            worst(&r, "I(Rl)", |t| load(t) / ZC) * ZC / E,
            // The current into the line at the load end is the load current
            // with the other sign.
            worst(&r, "I(L1.1.a)", |t| -load(t) / ZC) * ZC / E,
            worst(&r, "I(L1.0.a)", |_| v0 / ZC) * ZC / E,
        ];
        eprintln!("{method}: matched line, errors {errors:?}");
        assert!(errors.iter().all(|e| *e < 1e-12), "{method}: {errors:?}");
    }
}

/// The lattice diagram of a lossless line between a source E behind `rs`
/// and a load `rl`: (sending-end voltage, receiving-end voltage) at `t`.
fn lattice(t: f64, tau: f64, rs: f64, rl: f64) -> (f64, f64) {
    let v0 = E * ZC / (rs + ZC);
    let (gs, gl) = ((rs - ZC) / (rs + ZC), (rl - ZC) / (rl + ZC));
    let n = passed(t, tau);
    // Arrivals at the load at tau, 3 tau, ...: the k-th adds
    // v0 (gs gl)^k (1 + gl). Arrivals at the source at 2 tau, 4 tau, ...:
    // the k-th adds v0 gl (gs gl)^(k-1) (1 + gs).
    let at_load = (0..(n + 1) / 2).map(|k| v0 * (gs * gl).powi(k as i32) * (1.0 + gl));
    let at_source = (1..=n / 2).map(|k| v0 * gl * (gs * gl).powi(k as i32 - 1) * (1.0 + gs));
    (v0 + at_source.sum::<f64>(), at_load.sum())
}

fn lattice_circuit(length_km: f64, per_km: Value, rs: f64, rl: f64) -> Value {
    json!([
        dc("E1", "e", E),
        resistor("Rs", "e", "s.a", rs),
        line("L1", json!("s"), json!("r"), length_km, per_km),
        resistor("Rl", "r.a", "gnd", rl),
    ])
}

#[test]
fn reflections_at_both_ends_follow_the_lattice_diagram() {
    // Rs = 100 ohm (reflection -0.6) and Rl = 1200 ohm (+0.5).
    let (rs, rl) = (100.0, 1200.0);
    // 25 km is 10 steps; 2.5 km is one step, the shortest line there is: the
    // end of a step then reads the grid point at its start.
    for (length, travel_times) in [(25.0, 14.0), (2.5, 40.0)] {
        let tau = length * S_PER_KM;
        for method in METHODS {
            let r = run(&model(
                method,
                10e-6,
                travel_times * tau,
                "none",
                lattice_circuit(length, uniform(0.0), rs, rl),
                json!([node("s.a"), node("r.a")]),
            ));
            let errors = [
                worst(&r, "V(s.a)", |t| lattice(t, tau, rs, rl).0) / E,
                worst(&r, "V(r.a)", |t| lattice(t, tau, rs, rl).1) / E,
            ];
            eprintln!("{method}: lattice with {length} km, errors {errors:?}");
            assert!(errors.iter().all(|e| *e < 1e-12), "{method}: {errors:?}");
        }
    }
    // The sum converges to the dc value E Rl/(Rs + Rl).
    let (_, last) = lattice(400.0 * 1e-4, 1e-4, rs, rl);
    assert!((last - E * rl / (rs + rl)).abs() < 1e-12 * E);
}

#[test]
fn a_lossy_line_is_two_lossless_halves_with_its_resistance_lumped() {
    // 25 km with 1.6 ohm/km: R = 40 ohm. The same circuit built from parts:
    // 10 ohm, a lossless line of 12.5 km, 20 ohm, another one, 10 ohm. The
    // history term of the lossy line eliminates the middle [Dommel 1969];
    // with half the travel time a whole number of steps (5) both runs
    // interpolate nothing and agree to rounding.
    let (rs, rl) = (100.0, 1200.0);
    let tau = 25.0 * S_PER_KM;
    for method in METHODS {
        let lossy = run(&model(
            method,
            10e-6,
            200.0 * tau,
            "none",
            lattice_circuit(25.0, uniform(1.6), rs, rl),
            json!([node("s.a"), node("r.a"), current("L1")]),
        ));
        let parts = run(&model(
            method,
            10e-6,
            200.0 * tau,
            "none",
            json!([
                dc("E1", "e", E),
                resistor("Rs", "e", "s.a", rs),
                resistor("Ra", "s.a", "k.a", 10.0),
                line("H1", json!("k"), json!("p"), 12.5, uniform(0.0)),
                resistor("Rm", "p.a", "q.a", 20.0),
                line("H2", json!("q"), json!("m"), 12.5, uniform(0.0)),
                resistor("Rb", "m.a", "r.a", 10.0),
                resistor("Rl", "r.a", "gnd", rl),
            ]),
            json!([node("s.a"), node("r.a"), current("Ra"), current("Rb")]),
        ));
        let difference = |a: &str, b: &str, sign: f64| {
            lossy
                .series(a)
                .iter()
                .zip(parts.series(b))
                .map(|(x, y)| (x - sign * y).abs())
                .fold(0.0, f64::max)
        };
        let errors = [
            difference("V(s.a)", "V(s.a)", 1.0) / E,
            difference("V(r.a)", "V(r.a)", 1.0) / E,
            difference("I(L1.0.a)", "I(Ra)", 1.0) * ZC / E,
            difference("I(L1.1.a)", "I(Rb)", -1.0) * ZC / E,
        ];
        eprintln!("{method}: lossy line against its parts, differences {errors:?}");
        assert!(errors.iter().all(|e| *e < 1e-12), "{method}: {errors:?}");
        // After 200 travel times the reflections are gone (each round trip
        // multiplies them by less than 0.3): the dc value with the whole
        // resistance in series.
        let last = *lossy.series("V(r.a)").last().unwrap();
        assert!(
            (last - E * rl / (rs + 40.0 + rl)).abs() < 1e-12 * E,
            "{last}"
        );
        // The first front at the load, from the equations of the element
        // with z = Zc + R/4 and rho = (Zc - R/4)/z: the source sends
        // a = (1 + rho) v with v = E z/(Rs + z), the load receives
        // (1 + rho)/2 of it and divides it between z and Rl.
        let z = ZC + 10.0;
        let rho = (ZC - 10.0) / z;
        let first = E * z / (rs + z) * (1.0 + rho).powi(2) / 2.0 * rl / (rl + z);
        assert!((lossy.series("V(r.a)")[10] - first).abs() < 1e-12 * E);
    }
}

// ------------------------------------------------------------ steady state

fn c(re: f64, im: f64) -> Complex64 {
    Complex64::new(re, im)
}

type Abcd = [[Complex64; 2]; 2];

fn cascade(a: &Abcd, b: &Abcd) -> Abcd {
    let mut m = [[c(0.0, 0.0); 2]; 2];
    for i in 0..2 {
        for j in 0..2 {
            m[i][j] = a[i][0] * b[0][j] + a[i][1] * b[1][j];
        }
    }
    m
}

fn series(z: Complex64) -> Abcd {
    [[c(1.0, 0.0), z], [c(0.0, 0.0), c(1.0, 0.0)]]
}

/// A lossless line of electrical length `theta`.
fn lossless(zc: f64, theta: f64) -> Abcd {
    [
        [c(theta.cos(), 0.0), c(0.0, zc * theta.sin())],
        [c(0.0, theta.sin() / zc), c(theta.cos(), 0.0)],
    ]
}

/// ABCD of the circuit of a mode: R/4, half the lossless line, R/2, the
/// other half, R/4.
fn lumped(zc: f64, theta: f64, r: f64) -> Abcd {
    let half = lossless(zc, 0.5 * theta);
    let quarter = series(c(0.25 * r, 0.0));
    let mut m = cascade(&quarter, &half);
    m = cascade(&m, &series(c(0.5 * r, 0.0)));
    m = cascade(&m, &half);
    cascade(&m, &quarter)
}

/// cosh(gamma l) of the distributed line with series impedance
/// r + j zc theta and shunt admittance j theta / zc over its length.
fn distributed_a(zc: f64, theta: f64, r: f64) -> Complex64 {
    (c(r, zc * theta) * c(0.0, theta / zc)).sqrt().cosh()
}

/// A three-phase source of 66 kV on bus s and the line to the open bus r.
fn open_line(per_km: Value, length_km: f64) -> Value {
    json!([
        {"type": "three_phase_source", "name": "G", "bus": "s", "line_voltage_rms_v": 66e3, "frequency_hz": F},
        line("L1", json!("s"), json!("r"), length_km, per_km),
    ])
}

fn open_line_outputs() -> Value {
    json!([{"type": "bus_voltage", "bus": "r"}, current("L1")])
}

#[test]
fn the_phasor_solution_has_the_abcd_parameters_of_the_same_circuit() {
    // Positive-sequence source: only the line modes carry anything. Open
    // end: V_r = V_s / A and I_s = C V_r.
    for (length, r1) in [(300.0, 0.0), (300.0, R1), (1000.0, R1)] {
        let theta = W * length * (L1 * C1).sqrt();
        let zc = (L1 / C1).sqrt();
        let r = r1 * length;
        let run = run(&model(
            "2s-dirk",
            50e-6,
            1e-3,
            "steady_state",
            open_line(overhead(r1, R0), length),
            open_line_outputs(),
        ));
        let vs = c(66e3 / 3.0_f64.sqrt(), 0.0);
        let ratio = run.steady("V(r.a)") / vs;
        let m = lumped(zc, theta, r);
        let error = (ratio - 1.0 / m[0][0]).norm() * m[0][0].norm();
        let is = run.steady("I(L1.0.a)");
        let error_i = (is - m[1][0] * run.steady("V(r.a)")).norm() / is.norm();
        let open = run.steady("I(L1.1.a)").norm() / is.norm();
        // Phase b lags by 120 degrees: a balanced set.
        let b = run.steady("V(r.b)") / run.steady("V(r.a)");
        let balanced = (b - Complex64::from_polar(1.0, -2.0 * PI / 3.0)).norm();
        eprintln!(
            "{length} km, {r1} ohm/km: |V_r/V_s| = {:.6}, errors {error:.1e} {error_i:.1e} {open:.1e} {balanced:.1e}",
            ratio.norm()
        );
        assert!(error < 1e-12 && error_i < 1e-12 && open < 1e-12 && balanced < 1e-12);
        if r1 == 0.0 {
            // Without losses: 1 / cos(beta l).
            assert!((ratio - 1.0 / theta.cos()).norm() < 1e-12);
            continue;
        }
        // Against the distributed line. Both are functions of u = Z Y =
        // -theta^2 + j rho theta (rho = R/Zc) that agree in u up to the
        // first order in rho; the second-order terms differ by
        // rho^2/8 (cos theta - sin theta/theta + sin^2(theta/2)), which is
        // rho^2 theta^2/96 for a short line. The next term is smaller by
        // about rho theta/10.
        let rho = r / zc;
        let estimate = rho * rho / 8.0
            * (theta.cos() - theta.sin() / theta + (0.5 * theta).sin().powi(2)).abs();
        let difference = (m[0][0] - distributed_a(zc, theta, r)).norm();
        eprintln!(
            "  A against cosh(gamma l): difference {difference:.3e}, estimate {estimate:.3e} (rho = {rho:.3}, theta = {theta:.3})"
        );
        assert!(difference < 1.05 * estimate && difference > 0.95 * estimate);
        let against_exact = (ratio - 1.0 / distributed_a(zc, theta, r)).norm();
        assert!(against_exact < 1.05 * estimate / m[0][0].norm_sqr());
    }
}

#[test]
fn a_run_from_the_steady_state_stays_on_it() {
    // 250 km of the uniform line: tau = 1 ms = 100 steps, theta = 0.314.
    // Nothing is integrated and nothing interpolated: before t = 0 the
    // history is the sine itself, afterwards its samples. What is left is
    // rounding, which the reflections at the ideal source and the open end
    // keep without loss (20 travel times here).
    for method in METHODS {
        let r = run(&model(
            method,
            10e-6,
            0.02,
            "steady_state",
            open_line(uniform(0.0), 250.0),
            open_line_outputs(),
        ));
        let ratio = r.steady("V(r.a)").norm() / (66e3 / 3.0_f64.sqrt());
        assert!((ratio - 1.0 / (W * 1e-3).cos()).abs() < 1e-12);
        // The rows against sqrt(2) Re(X e^{jwt}), relative to the largest
        // amplitude of their kind.
        let departure = |names: [&str; 2], first_row: usize| {
            let amplitude = |n: &str| 2.0_f64.sqrt() * r.steady(n).norm();
            let largest = names.iter().map(|n| amplitude(n)).fold(0.0, f64::max);
            let mut worst = 0.0_f64;
            for name in names {
                let x = r.steady(name);
                for (t, v) in r.t.iter().zip(r.series(name)).skip(first_row) {
                    let steady = 2.0_f64.sqrt() * (x * Complex64::from_polar(1.0, W * t)).re;
                    worst = worst.max((v - steady).abs());
                }
            }
            worst / largest
        };
        let after = [
            departure(["V(r.a)", "V(r.b)"], 1),
            departure(["I(L1.0.a)", "I(L1.0.c)"], 1),
        ];
        let init = r.stats.initialization.as_ref().unwrap();
        eprintln!(
            "{method}: whole number of steps, departure after the first row {after:?}, in the summary {:.1e} and {:.1e}",
            init.relative_voltage_deviation,
            init.relative_current_deviation.unwrap()
        );
        assert!(after.iter().all(|d| *d < 1e-12), "{after:?}");
        // The summary includes the first row, the solution at t = 0+, whose
        // sources are 1e-6 of a step late: W * 1e-6 * h = 3.1e-9 of the
        // amplitude for every circuit.
        assert!(init.relative_voltage_deviation < 1.01 * W * 1e-11);
        assert!(init.relative_current_deviation.unwrap() < 1.01 * W * 1e-11);
        assert_eq!(init.checked_until_s, 0.02);
    }
}

#[test]
fn interpolation_between_grid_points_is_of_second_order() {
    // tau = 100 1/3 steps of 10 us, and 200 2/3 steps of 5 us: under the
    // trapezoidal rule (samples at grid points only) both read at a place w
    // with w (1 - w) = 2/9. Reading a sine of angular frequency W there
    // multiplies it by 1 - eps, eps = (W h)^2/9: a small loss per travel
    // time. With D = e^{-j theta} (1 - eps), the source sends
    // a = 2 V_s/(1 + D^2) and the open end is at D a. Against the exact
    // line, a moves by eps/cos(theta) of the open-end voltage and the
    // open-end voltage by eps tan(theta), in quadrature. A run that starts
    // on the exact steady state has the exact a in its history; the
    // difference goes back and forth without loss, on top of the shift:
    // the departure is close to eps (1 + sin theta)/cos theta.
    let length = (100.0 + 1.0 / 3.0) * 10e-6 / S_PER_KM;
    let theta = W * length * S_PER_KM;
    let mut departures = Vec::new();
    for h in [10e-6, 5e-6] {
        let r = run(&model(
            "trapezoidal",
            h,
            0.04,
            "steady_state",
            open_line(uniform(0.0), length),
            open_line_outputs(),
        ));
        let predicted = (W * h).powi(2) / 9.0 * (1.0 + theta.sin()) / theta.cos();
        // The departure is relative to the largest node voltage, the open
        // end.
        let d = r.departure();
        eprintln!("h = {h:e}: departure {d:.3e}, predicted {predicted:.3e}");
        assert!(
            d > 0.9 * predicted && d < 1.05 * predicted,
            "{d} {predicted}"
        );
        departures.push(d);
    }
    let ratio = departures[0] / departures[1];
    eprintln!("halving the step divides the departure by {ratio:.2}");
    assert!((3.8..4.2).contains(&ratio));
    // 2S-DIRK also keeps the stages between the grid points, so the samples
    // are closer and the error smaller, by a factor that depends on the
    // place between them.
    let r = run(&model(
        "2s-dirk",
        10e-6,
        0.04,
        "steady_state",
        open_line(uniform(0.0), length),
        open_line_outputs(),
    ));
    eprintln!("2s-dirk: departure {:.3e}", r.departure());
    assert!(r.departure() < departures[0]);
}

#[test]
fn a_line_between_networks_with_states_stays_on_the_steady_state() {
    // Source behind an impedance, 120 km of overhead line (travel times of
    // 405.7 us and 550 us: nothing is a whole number of steps), and a load
    // with inductance. The departure is the error of the method in the
    // forced response and of the interpolation, both of the order
    // (W h)^2 = 9.9e-6 at h = 10 us (see steady_state.rs). It falls by about 4
    // when the step is halved; not exactly, because the place of a reading
    // between two samples, and with it the error w (1 - w)/2 of that reading,
    // is another one for each step.
    for method in METHODS {
        let mut departures = Vec::new();
        for h in [20e-6, 10e-6] {
            let r = run(&model(
                method,
                h,
                0.04,
                "steady_state",
                json!([
                    {"type": "three_phase_source", "name": "G", "bus": "s", "line_voltage_rms_v": 66e3, "frequency_hz": F,
                     "impedance": {"type": "sequence", "r1_ohm": 0.5, "l1_h": 0.016, "r0_ohm": 1.0, "l0_h": 0.048}},
                    line("L1", json!("s"), json!("r"), 120.0, overhead(R1, R0)),
                    {"type": "coupled_rl", "name": "LD", "buses": ["r", "gnd"],
                     "impedance": {"type": "sequence", "r1_ohm": 400.0, "l1_h": 0.4, "r0_ohm": 400.0, "l0_h": 0.4}},
                ]),
                json!([{"type": "bus_voltage", "bus": "r"}, current("L1"), current("LD")]),
            ));
            let init = r.stats.initialization.as_ref().unwrap();
            let d = init
                .relative_voltage_deviation
                .max(init.relative_current_deviation.unwrap());
            eprintln!(
                "{method}, h = {h:e}: departure {d:.2e}, (W h)^2 = {:.2e}",
                (W * h).powi(2)
            );
            assert!(d < (W * h).powi(2));
            departures.push(d);
        }
        let ratio = departures[0] / departures[1];
        eprintln!("{method}: halving the step divides the departure by {ratio:.2}");
        assert!((2.5..6.5).contains(&ratio));
    }
}

#[test]
fn pi_sections_approach_the_travelling_wave_line() {
    // The open-end voltage of the phasor solution. Without losses the
    // Bergeron line is the distributed line, and N pi sections differ from
    // it by a term in 1/N^2.
    let open_end = |line: Value| {
        let r = run(&model(
            "2s-dirk",
            50e-6,
            1e-3,
            "steady_state",
            json!([
                {"type": "three_phase_source", "name": "G", "bus": "s", "line_voltage_rms_v": 66e3, "frequency_hz": F},
                line,
            ]),
            json!([{"type": "bus_voltage", "bus": "r"}]),
        ));
        r.steady("V(r.a)")
    };
    let pi = |sections: u32, per_km: Value| {
        open_end(
            json!({"type": "pi_line", "name": "L1", "buses": ["s", "r"], "length_km": 300.0,
            "sections": sections, "per_km": per_km}),
        )
    };
    let reference = open_end(line(
        "L1",
        json!("s"),
        json!("r"),
        300.0,
        overhead(0.0, 0.0),
    ));
    let mut previous = f64::NAN;
    for sections in [2, 4, 8, 16, 32] {
        let d = (pi(sections, overhead(0.0, 0.0)) - reference).norm() / reference.norm();
        eprintln!(
            "lossless, {sections} sections: difference {d:.3e}, ratio {:.3}",
            previous / d
        );
        if sections >= 8 {
            assert!((3.95..4.05).contains(&(previous / d)));
        }
        previous = d;
    }
    // With losses the pi sections approach the distributed line, from which
    // the Bergeron line is rho^2/8 (cos theta - sin theta/theta +
    // sin^2(theta/2)) / |A| away in the open-end voltage (1.3e-6 here, see
    // above): the difference falls with 1/N^2 down to that.
    let reference = open_end(line("L1", json!("s"), json!("r"), 300.0, overhead(R1, R0)));
    let theta = W * 300.0 * (L1 * C1).sqrt();
    let rho = R1 * 300.0 / (L1 / C1).sqrt();
    let floor = rho * rho / 8.0
        * (theta.cos() - theta.sin() / theta + (0.5 * theta).sin().powi(2)).abs()
        / theta.cos();
    let mut differences = Vec::new();
    for sections in [2, 4, 8, 512] {
        let d = (pi(sections, overhead(R1, R0)) - reference).norm() / reference.norm();
        eprintln!("lossy, {sections} sections: difference {d:.3e} (floor {floor:.3e})");
        differences.push(d);
    }
    assert!((3.5..4.5).contains(&(differences[0] / differences[1])));
    assert!(differences[3] < 1.1 * floor && differences[3] > 0.9 * floor);
}

// ---------------------------------------------------------- three phases

/// Open-end value of a lossless mode whose sending end is held at 1 from
/// t = 0 on: 2 from tau to 3 tau, 0 from 3 tau to 5 tau, ...
fn square(t: f64, tau: f64) -> f64 {
    if ((passed(t, tau) + 1) / 2) % 2 == 1 {
        2.0
    } else {
        0.0
    }
}

#[test]
fn the_zero_sequence_and_the_line_modes_travel_at_their_own_speeds() {
    // 25 km with 4 us/km in the line modes (10 steps) and 6 us/km in the
    // zero sequence (15 steps). E on phase a at t = 0, phases b and c of the
    // sending end at gnd, the far end open. The sending voltages (E, 0, 0)
    // are E/3 of zero sequence plus the line-mode part (2E/3, -E/3, -E/3),
    // and each travels as on a single line with an open end:
    //   v_a = E/3 s0(t) + 2E/3 s1(t),   v_b = v_c = E/3 s0(t) - E/3 s1(t)
    // with s the square wave of each mode. The line modes arrive first:
    // 4E/3 on phase a and -2E/3 on the others, then 2E and 0.
    let (tau1, tau0) = (100e-6, 150e-6);
    let sequence = json!({"type": "sequence",
        "r1_ohm_per_km": 0.0, "l1_h_per_km": 1.6e-3, "c1_f_per_km": 1e-8,
        "r0_ohm_per_km": 0.0, "l0_h_per_km": 4.5e-3, "c0_f_per_km": 8e-9});
    // The same line by its matrices: self (x0 + 2 x1)/3, mutual (x0 - x1)/3.
    let balanced = |x1: f64, x0: f64| {
        let (s, m) = ((x0 + 2.0 * x1) / 3.0, (x0 - x1) / 3.0);
        json!([[s, m, m], [m, s, m], [m, m, s]])
    };
    let matrix = json!({"type": "matrix",
        "resistance_ohm_per_km": balanced(0.0, 0.0),
        "inductance_h_per_km": balanced(1.6e-3, 4.5e-3),
        "capacitance_f_per_km": balanced(1e-8, 8e-9)});
    for method in METHODS {
        for (form, per_km) in [("sequence", &sequence), ("matrix", &matrix)] {
            let r = run(&model(
                method,
                10e-6,
                1.5e-3,
                "none",
                json!([
                    dc("E1", "s.a", E),
                    line(
                        "L1",
                        json!(["s.a", "gnd", "gnd"]),
                        json!("r"),
                        25.0,
                        per_km.clone()
                    ),
                ]),
                json!([{"type": "bus_voltage", "bus": "r"}]),
            ));
            let a = |t: f64| E / 3.0 * square(t, tau0) + 2.0 * E / 3.0 * square(t, tau1);
            let other = |t: f64| E / 3.0 * square(t, tau0) - E / 3.0 * square(t, tau1);
            let errors = [
                worst(&r, "V(r.a)", a) / E,
                worst(&r, "V(r.b)", other) / E,
                worst(&r, "V(r.c)", other) / E,
            ];
            eprintln!("{method}, {form}: one phase energized, errors {errors:?}");
            assert!(errors.iter().all(|e| *e < 1e-12), "{method}: {errors:?}");
            // Between the two arrivals.
            let k = 12;
            assert!((r.series("V(r.a)")[k] - 4.0 * E / 3.0).abs() < 1e-12 * E);
            assert!((r.series("V(r.b)")[k] + 2.0 * E / 3.0).abs() < 1e-12 * E);
        }
    }
}

/// Inverse of a 3x3 matrix by cofactors.
fn inverse(m: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let cofactor = |i: usize, j: usize| {
        let (a, b) = ((i + 1) % 3, (i + 2) % 3);
        let (p, q) = ((j + 1) % 3, (j + 2) % 3);
        m[a][p] * m[b][q] - m[a][q] * m[b][p]
    };
    let det: f64 = (0..3).map(|j| m[0][j] * cofactor(0, j)).sum();
    let mut inv = [[0.0; 3]; 3];
    for (j, row) in inv.iter_mut().enumerate() {
        for (i, v) in row.iter_mut().enumerate() {
            *v = cofactor(i, j) / det;
        }
    }
    inv
}

#[test]
fn the_modes_of_an_untransposed_line_arrive_one_after_the_other() {
    // A line by its matrices (flat formation), 30 km, all three phases
    // driven by dc sources from t = 0, the far end open. The modes
    // (`line_modes`: they diagonalize L C, checked in the model crate) have
    // three travel times, none a whole number of steps. In modal quantities
    // every mode is a single line with an open end, so at the far end
    //   v(t) = W^-1 diag(s_k(t)) W v_s.
    // A front that arrives between two grid points is in the row of the
    // next one, and the values between fronts are exact: the history before
    // t = 0 and the solution at 0+ are separate, so the first reading
    // interpolates between equal values. Checked up to the first
    // reflection from the source (the fastest mode at 3 tau), after which
    // fronts have been read twice and are spread over a step.
    let l = [
        [1.6e-3, 0.6e-3, 0.4e-3],
        [0.6e-3, 1.6e-3, 0.6e-3],
        [0.4e-3, 0.6e-3, 1.6e-3],
    ];
    let cap = [
        [10.0e-9, -2.0e-9, -0.8e-9],
        [-2.0e-9, 10.5e-9, -2.0e-9],
        [-0.8e-9, -2.0e-9, 10.0e-9],
    ];
    let per_km = LineParameters::Matrix {
        resistance_ohm_per_km: [[0.0; 3]; 3],
        inductance_h_per_km: l,
        capacitance_f_per_km: cap,
    };
    let modes = line_modes(&per_km, 30.0).unwrap();
    let tau = modes.modes.map(|m| m.travel_time_s);
    eprintln!("travel times {tau:?}");
    let w = modes.transform;
    let w_inv = inverse(&w);
    let source = [100.0, -40.0, 25.0];
    let h = 5e-6;
    for method in METHODS {
        let r = run(&model(
            method,
            h,
            ((3.0 * tau[2] / h).floor() - 2.0) * h,
            "none",
            json!([
                dc("Ea", "s.a", source[0]),
                dc("Eb", "s.b", source[1]),
                dc("Ec", "s.c", source[2]),
                line(
                    "L1",
                    json!("s"),
                    json!("r"),
                    30.0,
                    serde_json::to_value(&per_km).unwrap()
                ),
            ]),
            json!([{"type": "bus_voltage", "bus": "r"}]),
        ));
        let expected = |t: f64, phase: usize| {
            (0..3)
                .map(|k| {
                    let mode: f64 = (0..3).map(|p| w[k][p] * source[p]).sum();
                    // The row of the first grid point at or after the
                    // arrival has the front.
                    let arrived = (t / h).round() >= (tau[k] / h).ceil();
                    w_inv[phase][k] * if arrived { 2.0 * mode } else { 0.0 }
                })
                .sum::<f64>()
        };
        let errors = [
            worst(&r, "V(r.a)", |t| expected(t, 0)) / E,
            worst(&r, "V(r.b)", |t| expected(t, 1)) / E,
            worst(&r, "V(r.c)", |t| expected(t, 2)) / E,
        ];
        eprintln!("{method}: untransposed line, errors {errors:?}");
        assert!(errors.iter().all(|e| *e < 1e-12), "{method}: {errors:?}");
        // After all three have arrived the open end is at twice the source.
        let last = r.rows.last().unwrap();
        for p in 0..3 {
            assert!((last[p] - 2.0 * source[p]).abs() < 1e-12 * E);
        }
    }
}

// ------------------------------------------- fronts between grid points

#[test]
fn a_front_between_grid_points_is_in_the_next_row_and_spreads_by_a_step_per_reading() {
    // tau = 10.5 steps (26.25 km), Rs = 100 ohm, Rl = 1200 ohm.
    let (rs, rl) = (100.0, 1200.0);
    let tau = 26.25 * S_PER_KM;
    let h = 10e-6;
    for method in METHODS {
        let r = run(&model(
            method,
            h,
            400.0 * tau,
            "none",
            lattice_circuit(26.25, uniform(0.0), rs, rl),
            json!([node("s.a"), node("r.a")]),
        ));
        let (vs, vr) = (r.series("V(s.a)"), r.series("V(r.a)"));
        // The first front left at t = 0+ and is read against the rest before
        // t = 0, which is kept apart: it is whole in the row of the first
        // grid point after tau (11 h).
        let first = lattice(1.5 * tau, tau, rs, rl).1;
        for (k, v) in vr.iter().enumerate().take(31) {
            let expected = if k >= 11 { first } else { 0.0 };
            assert!((v - expected).abs() < 1e-12 * E, "{method}: row {k}: {v}");
        }
        // Its reflection reaches the source at 2 tau = 21 h. The load sent
        // it between its rows 10 and 11, and the source reads half a step
        // from each: half of the front in row 21, all of it in row 22.
        let (before, after) = (
            lattice(tau, tau, rs, rl).0,
            lattice(2.5 * tau, tau, rs, rl).0,
        );
        assert!((vs[20] - before).abs() < 1e-12 * E);
        assert!((vs[22] - after).abs() < 1e-12 * E);
        assert!(vs[21] > before.min(after) && vs[21] < before.max(after));
        if method == "trapezoidal" {
            assert!((vs[21] - 0.5 * (before + after)).abs() < 1e-12 * E);
        }
        // Later fronts have been read more often and are spread further,
        // but nothing is lost: the dc value is reached.
        let last = *vr.last().unwrap();
        assert!((last - E * rl / (rs + rl)).abs() < 1e-12 * E);
        // Interpolated values lie between the samples: no value leaves the
        // range of the lattice diagram.
        let (low, high) = (0..40)
            .map(|k| lattice((k as f64 + 0.5) * tau, tau, rs, rl).1)
            .fold((0.0_f64, 0.0_f64), |(lo, hi), v| (lo.min(v), hi.max(v)));
        assert!(vr.iter().all(|v| *v >= low - 1e-9 && *v <= high + 1e-9));
    }
}

// ------------------------------------------------- switching and diodes

#[test]
fn a_switch_and_a_diode_at_the_ends_of_a_line() {
    // E behind Rs = Zc and a switch that closes at 50 us; 25 km of line
    // (10 steps); at the far end a diode in series with Rl = Zc. The switch
    // closes after the row of 50 us: the sending end is at E/2 from the row
    // of 60 us on. The line keeps the value before and the one after an
    // action at a grid point apart, and the far end, one travel time later,
    // reads the one after: the front is in the row of 150 us. The diode
    // still blocks in that row (the open end doubles the wave to E) and
    // conducts from the next step on: E/2 (1 + g) with the small reflection
    // g of its on-resistance. Its closing is an action at 150 us as well:
    // what the far end sends back from that grid point on is the matched
    // value, so the one row at E does not come back to the source. The steps
    // after the two actions use the trial solution for the diode state, and
    // the half steps under the trapezoidal rule.
    for method in METHODS {
        let model = model(
            method,
            10e-6,
            600e-6,
            "none",
            json!([
                dc("E1", "e", E),
                resistor("Rs", "e", "x", ZC),
                {"type": "switch", "name": "SW", "nodes": ["x", "s.a"], "close_time_s": 50e-6},
                line("L1", json!("s"), json!("r"), 25.0, uniform(0.0)),
                {"type": "diode", "name": "D1", "nodes": ["r.a", "d"]},
                resistor("Rl", "d", "gnd", ZC),
            ]),
            json!([node("s.a"), node("r.a")]),
        );
        let r = run(&model);
        let events: Vec<(i64, &str, &str)> = r
            .stats
            .events
            .iter()
            .map(|e| {
                (
                    (e.time_s * 1e6).round() as i64,
                    e.component.as_str(),
                    e.action,
                )
            })
            .collect();
        assert_eq!(
            events,
            [(50, "SW", "close"), (150, "D1", "close")],
            "{method}"
        );
        let g = 1e-3 / (2.0 * ZC + 1e-3);
        let (vs, vr) = (r.series("V(s.a)"), r.series("V(r.a)"));
        for k in 0..vs.len() {
            let sending = match k {
                0..=5 => 0.0,
                _ => 0.5 * E,
            };
            let far = match k {
                0..=14 => 0.0,
                15 => E,
                _ => 0.5 * E * (1.0 + g),
            };
            // The off-resistance of the diode (1e6 ohm) and g reflect parts
            // in 1e-4 and 1e-6 of the waves.
            assert!(
                (vs[k] - sending).abs() < 1e-3 * E,
                "{method}: V(s.a) row {k}: {}",
                vs[k]
            );
            assert!(
                (vr[k] - far).abs() < 1e-3 * E,
                "{method}: V(r.a) row {k}: {}",
                vr[k]
            );
        }
        assert!((vr[40] - 0.5 * E * (1.0 + g)).abs() < 1e-9 * E);
    }
}

// --------------------------------------------------------- frequency scan

#[test]
fn the_frequency_scan_uses_the_same_two_port() {
    // The sequence impedances seen from the far end of 100 km with the
    // sending end at an ideal source (a short circuit for the scan):
    // Z = B / D of the circuit of each mode, j Zc tan(theta) without
    // losses. 1479 Hz is close to the quarter-wave resonance of the line
    // modes.
    let length = 100.0;
    let frequencies = [50.0, 250.0, 1000.0, 1479.0, 3000.0];
    for (r1, r0) in [(0.0, 0.0), (R1, R0)] {
        let m = json!({
            "format": "openkeito-model",
            "version": "0.5",
            "simulation": {"time_step_s": 1e-5, "end_time_s": 1e-3},
            "components": open_line(overhead(r1, r0), length),
            "frequency_scan": {"frequencies_hz": frequencies, "buses": ["r"]},
        });
        let loaded = openkeito_model::load_str(&m.to_string()).unwrap();
        let scanner = FrequencyScanner::from_model(&loaded.model).expect("scan builds");
        let columns: Vec<String> = scanner.columns().iter().map(|c| c.name.clone()).collect();
        let mut rows = Vec::new();
        scanner
            .run(|f, v| {
                rows.push((f, v.to_vec()));
                Ok(())
            })
            .expect("scan completes");
        let value = |row: &[Option<f64>], name: &str| {
            let k = columns.iter().position(|c| c == name).unwrap();
            Complex64::from_polar(row[k].unwrap(), row[k + 1].unwrap().to_radians())
        };
        for (f, row) in &rows {
            let w = 2.0 * PI * f;
            let expected = |l: f64, cap: f64, r: f64| {
                let m = lumped((l / cap).sqrt(), w * length * (l * cap).sqrt(), r * length);
                m[0][1] / m[1][1]
            };
            let (z1, z0) = (expected(L1, C1, r1), expected(L0, C0, r0));
            let errors = [
                (value(row, "Z1mag(r)") - z1).norm() / z1.norm(),
                (value(row, "Z2mag(r)") - z1).norm() / z1.norm(),
                (value(row, "Z0mag(r)") - z0).norm() / z0.norm(),
            ];
            eprintln!(
                "{f} Hz, r1 = {r1}: |Z1| = {:.4e}, |Z0| = {:.4e}, errors {errors:?}",
                z1.norm(),
                z0.norm()
            );
            // Near the resonance the admittances cancel to 1e-3 of
            // themselves; rounding grows by as much.
            assert!(errors.iter().all(|e| *e < 1e-10), "{errors:?}");
            if r1 == 0.0 {
                let theta = w * length * (L1 * C1).sqrt();
                let tangent = c(0.0, (L1 / C1).sqrt() * theta.tan());
                assert!((z1 - tangent).norm() < 1e-9 * tangent.norm());
            }
        }
    }
}

#[test]
fn a_wave_that_rises_after_an_action_is_read_without_a_step_of_delay() {
    // E through a switch and 40 mH onto a line that ends in Zc: the sending
    // end follows E (1 - e^{-(t - tc) Zc / L}) (time constant 100 us, ten
    // steps) and the far end the same one travel time later. Nothing jumps:
    // the value just after the action is the one before it. A far end that
    // read the wave a step late would be off by E h Zc / L = 10 V at the
    // start; a value after the action that was simply that of the first
    // solution after it, by 0.29 of that in one row.
    //
    // What is left is the integration of the inductor current. 2S-DIRK:
    // 0.0404 (h Zc/L)^3 E per step at the start, summed over the decay
    // (about L/(Zc h) steps): 0.04 (h Zc/L)^2 E = 0.04 V. The trapezoidal
    // rule takes the step after the action as two backward-Euler half
    // steps, each with a local error of (h/2)^2/2 (Zc/L)^2 E: 0.25 V.
    // The value just after the action is on the straight line through the
    // first two solutions after it (at 0.29 h and h with 2S-DIRK), which
    // misses the curve by up to 0.29/2 (h Zc/L)^2 E = 0.15 V; the row one
    // travel time after an action at a grid point reads exactly that value
    // (0.08 V measured), every other row reads solutions.
    let (l, tau) = (40e-3, 25.0 * S_PER_KM);
    let h = 10e-6;
    for (switching, tc) in [("grid", 50e-6), ("interpolated", 53.7e-6)] {
        for (method, tolerance) in [("2s-dirk", 0.15), ("trapezoidal", 0.3)] {
            let mut m = model(
                method,
                h,
                600e-6,
                "none",
                json!([
                    dc("E1", "e", E),
                    {"type": "switch", "name": "SW", "nodes": ["e", "x"], "close_time_s": tc},
                    {"type": "inductor", "name": "Ls", "nodes": ["x", "s.a"], "inductance_h": l},
                    line("L1", json!("s"), json!("r"), 25.0, uniform(0.0)),
                    resistor("Rl", "r.a", "gnd", ZC),
                ]),
                json!([node("s.a"), node("r.a")]),
            );
            m["simulation"]["switching"] = switching.into();
            let r = run(&m);
            let wave = |t: f64| {
                if t > tc {
                    E * (1.0 - (-(t - tc) * ZC / l).exp())
                } else {
                    0.0
                }
            };
            let errors = [
                worst(&r, "V(s.a)", wave),
                worst(&r, "V(r.a)", |t| wave(t - tau)),
            ];
            eprintln!("{switching}, {method}: rising wave, errors {errors:?} V");
            assert!(errors.iter().all(|e| *e < tolerance), "{errors:?}");
        }
    }
}

// ------------------------------------- switching instants within steps

#[test]
fn split_steps_bring_the_waves_of_instants_within_steps_to_the_fine_grid() {
    // A dc source switched onto a lossless line and off again three times,
    // at instants between the grid points of h = 10 us and 5 us; the far end
    // has 1200 ohm (reflection +0.5), the sending end reflects with -1 while
    // a switch is closed and with +1 while all are open. Every instant
    // starts a front that goes back and forth until the load has taken it.
    //
    // The reference has the instants at its grid points: h = 0.1 us, where
    // the travel time is a whole number of steps and nothing is
    // interpolated.
    //
    // With `switching` "interpolated" the step is split at each instant, and
    // the line keeps the value before and the one after the action at that
    // place of the step. With a travel time of a whole number of steps the
    // rows read grid points, which lie on the same side of every front as in
    // the reference: no difference at all. (The same holds at grid points
    // here: the instant moves to the next grid point, which no row can
    // tell.) With a travel time between grid points every reading
    // interpolates, a front is spread over a step each time it is read, and
    // the rows next to a front differ: the mean difference falls with the
    // step, and with instants at grid points it is several times larger,
    // because every front is late by up to a step as well.
    let instants = [
        (123.7e-6, 301.3e-6),
        (455.9e-6, 688.1e-6),
        (801.3e-6, 1033.3e-6),
    ];
    let end = 1.5e-3;
    let far_end = |length_km: f64, h: f64, switching: &str, every: u64| {
        let mut components = vec![
            dc("E1", "e", E),
            line("L1", json!("s"), json!("r"), length_km, uniform(0.0)),
            resistor("Rl", "r.a", "gnd", 1200.0),
        ];
        for (k, (close, open)) in instants.iter().enumerate() {
            components.push(
                json!({"type": "switch", "name": format!("SW{k}"), "nodes": ["e", "s.a"],
                "close_time_s": close, "open_time_s": open, "open_at_current_zero": false}),
            );
        }
        let mut m = model(
            "2s-dirk",
            h,
            end,
            "none",
            json!(components),
            json!([node("r.a")]),
        );
        m["simulation"]["switching"] = switching.into();
        m["simulation"]["output_interval_steps"] = every.into();
        let r = run(&m);
        assert_eq!(r.stats.events.len(), 6, "{:?}", r.stats.events);
        if switching == "interpolated" {
            // The instants are those of the model, not grid points.
            let closing = r.stats.events[0].time_s;
            assert!((closing - instants[0].0).abs() < 1e-12, "{closing}");
        }
        r.series("V(r.a)")
    };
    // Mean |difference| at the rows of the coarse run, in volt.
    let distance = |a: &[f64], reference: &[f64], stride: usize| {
        let n = a.len();
        (0..n)
            .map(|k| (a[k] - reference[k * stride]).abs())
            .sum::<f64>()
            / n as f64
    };
    // 25 km: 10 and 20 steps of travel time. 25.625 km: 10.25 and 20.5 steps
    // (1025 of the reference).
    for length_km in [25.0, 25.625] {
        // The reference at every 5 us.
        let reference = far_end(length_km, 0.1e-6, "grid", 50);
        let mut split = Vec::new();
        let mut grid = Vec::new();
        for (h, stride) in [(10e-6, 2), (5e-6, 1)] {
            let with_parts = far_end(length_km, h, "interpolated", 1);
            split.push(distance(&with_parts, &reference, stride));
            let at_grid_points = far_end(length_km, h, "grid", 1);
            grid.push(distance(&at_grid_points, &reference, stride));
        }
        eprintln!(
            "{length_km} km, mean difference from the fine grid: split {split:?} V, at grid points {grid:?} V"
        );
        if length_km == 25.0 {
            assert!(split[0] < 1e-12 * E && split[1] < 1e-12 * E, "{split:?}");
            assert!(grid[0] < 1e-12 * E && grid[1] < 1e-12 * E, "{grid:?}");
        } else {
            assert!(split[0] < 0.5 * grid[0], "{split:?} {grid:?}");
            assert!(
                split[1] < 0.5 * grid[1] && split[1] < 0.75 * split[0],
                "{split:?} {grid:?}"
            );
        }
    }
}

#[test]
fn lossless_line_feeding_a_diode_rectifier_matches_the_lattice_diagram() {
    // Lossless line (Zc = 400 ohm) fed by a 2 kHz sine source behind Rs = 100
    // ohm, driving a diode + resistor (Rl = 400 ohm) at the receiving end
    // under "switching": "interpolated". With tau = 20 us (2 steps at
    // h = 10 us) and phase chosen so the downward zero crossing occurs at
    // t = 270 us + 1e-13 s (within min_part of step 27, where (27*h)/h < 27),
    // D1's turn-off is snapped to step 27 (`t_star = ta`). A second case with
    // tau = 25 us (2.5 steps at h = 10 us, 5 steps at h = 5 us) checks both
    // fractional-step interpolation and halving the time step, and with
    // Rs = 100 ohm != Zc the waves reflect repeatedly at both ends.
    let rs = 100.0;
    let rl = 400.0;
    let ron = 1e-3;
    let roff = 1e6;
    let f = 2000.0;
    let omega = 2.0 * PI * f;
    // Phase near 0 so E(0) ~ 0 (continuous start from rest) and the first
    // downward crossing of the forward wave at the load (at tau = 20 us)
    // lands just past t = 270 us (step 27 at h = 10 us).
    let phase_deg = 360.0 * f * 1e-13;
    let source = |t: f64| {
        if t < 0.0 {
            0.0
        } else {
            E * (omega * t + phase_deg.to_radians()).sin()
        }
    };
    fn lattice(
        t: f64,
        tau: f64,
        rs: f64,
        rl: f64,
        ron: f64,
        roff: f64,
        source: &impl Fn(f64) -> f64,
    ) -> (f64, f64) {
        fn a_s(
            t: f64,
            tau: f64,
            rs: f64,
            rl: f64,
            ron: f64,
            roff: f64,
            source: &impl Fn(f64) -> f64,
        ) -> f64 {
            if t < 0.0 {
                return 0.0;
            }
            let e_s = a_r(t - tau, tau, rs, rl, ron, roff, source).0;
            (2.0 * ZC * source(t) + (rs - ZC) * e_s) / (rs + ZC)
        }
        fn a_r(
            t: f64,
            tau: f64,
            rs: f64,
            rl: f64,
            ron: f64,
            roff: f64,
            source: &impl Fn(f64) -> f64,
        ) -> (f64, f64) {
            if t < 0.0 {
                return (0.0, 0.0);
            }
            let e_r = a_s(t - tau, tau, rs, rl, ron, roff, source);
            let rd = if e_r > 0.0 { ron } else { roff };
            let r_tot = rl + rd;
            let v_load = rl / (ZC + r_tot) * e_r;
            let ar = (r_tot - ZC) / (r_tot + ZC) * e_r;
            (ar, v_load)
        }
        let (_, v_load) = a_r(t, tau, rs, rl, ron, roff, source);
        let vs = if t < 0.0 {
            0.0
        } else {
            let es = a_r(t - tau, tau, rs, rl, ron, roff, source).0;
            (ZC * source(t) + rs * es) / (rs + ZC)
        };
        (vs, v_load)
    }

    for method in METHODS {
        // 1) Whole-step travel time (tau = 20 us): exact lattice-diagram values
        //    at grid points (and step 27 snaps D1's crossing to the grid point).
        // 2) Fractional travel time (tau = 25 us): h = 10 us interpolates at
        //    half-steps while h = 5 us hits whole steps, and halving h shrinks
        //    the error.
        let mut errors = Vec::new();
        for (h, length_km) in [(10e-6, 5.0), (5e-6, 5.0), (10e-6, 6.25), (5e-6, 6.25)] {
            let tau_case = length_km * S_PER_KM;
            let mut m = model(
                method,
                h,
                1.5e-3,
                "none",
                json!([
                    {"type": "voltage_source", "name": "E1", "nodes": ["e", "gnd"],
                     "waveform": {"type": "sine", "amplitude": E, "frequency_hz": f, "phase_deg": phase_deg}},
                    resistor("Rs", "e", "s.a", rs),
                    line("L1", json!("s"), json!("r"), length_km, uniform(0.0)),
                    {"type": "diode", "name": "D1", "nodes": ["r.a", "load"],
                     "on_resistance_ohm": ron, "off_resistance_ohm": roff},
                    resistor("Rl", "load", "gnd", rl),
                ]),
                json!([node("s.a"), node("load")]),
            );
            m["simulation"]["switching"] = "interpolated".into();
            let r = run(&m);
            let vs = r.series("V(s.a)");
            let vl = r.series("V(load)");
            let mut max_err = 0.0_f64;
            for (idx, &t) in r.t.iter().enumerate() {
                let (exp_s, exp_l) = lattice(t, tau_case, rs, rl, ron, roff, &source);
                max_err = max_err
                    .max((vs[idx] - exp_s).abs())
                    .max((vl[idx] - exp_l).abs());
            }
            eprintln!("{method}, h={h:e}, L={length_km} km: max error {max_err:.3e} V");
            errors.push(max_err);
        }
        // For tau = 20 us (whole steps at both h = 10 us and h = 5 us), the
        // error comes from linear interpolation/extrapolation of the sine wave
        // across one step at switching instants: O((omega h)^2) (~0.06-0.10 V
        // at h = 10 us and ~0.013-0.022 V at h = 5 us), shrinking by ~4x on
        // halving h. For tau = 25 us (fractional 2.5 steps at h = 10 us, whole
        // 5 steps at h = 5 us), h = 10 us adds chord interpolation (~0.4-0.7 V
        // for omega h = 0.126 rad).
        assert!(errors[0] < 0.15, "{method}: {errors:?}");
        assert!(errors[1] < errors[0] / 3.0, "{method}: {errors:?}");
        assert!(errors[2] < 1.0, "{method}: {errors:?}");
        assert!(errors[3] < errors[2] / 10.0, "{method}: {errors:?}");
    }
}
