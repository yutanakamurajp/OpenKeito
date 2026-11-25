//! Piecewise-linear elements compared with closed-form solutions: the
//! nonlinear resistor (surge arrester), the nonlinear inductor and the
//! saturation of transformers.
//!
//! The characteristic used for the reference is evaluated here, from the
//! points of the model, not by the solver. Every test prints the number of
//! additional solutions that the search for the segments took (run with
//! `--nocapture`); docs/numerical-method.md quotes them.

mod common;

use std::f64::consts::PI;

use common::phasors::METHODS;
use common::{run_model as run, Run};
use openkeito_core::Simulator;
use openkeito_model::Diagnostic;
use serde_json::{json, Value};

fn load(model: &Value) -> Result<openkeito_model::Loaded, Vec<Diagnostic>> {
    openkeito_model::load_str(&model.to_string())
}

fn model(method: &str, h: f64, end: f64, components: Value, outputs: Value) -> Value {
    json!({
        "format": "openkeito-model",
        "version": "0.5",
        "simulation": {"time_step_s": h, "end_time_s": end, "method": method},
        "components": components,
        "outputs": outputs,
    })
}

/// Prints the additional solutions of a run, for the documentation.
fn report(what: &str, r: &Run) {
    println!(
        "{what}: {} steps, {} solutions, operating-point iterations {} in all, at most {} in one \
         stage, {} factorizations",
        r.stats.steps,
        r.stats.solves,
        r.stats.operating_point_iterations,
        r.stats.max_operating_point_iterations,
        r.stats.factorizations
    );
}

/// A piecewise-linear function through points with increasing x, continued
/// with its outermost segments.
#[derive(Clone)]
struct Pwl(Vec<[f64; 2]>);

impl Pwl {
    /// The points and their mirror images; the origin is not a point.
    fn symmetric(x: &[f64], y: &[f64]) -> Pwl {
        let mut p: Vec<[f64; 2]> = x.iter().zip(y).rev().map(|(x, y)| [-x, -y]).collect();
        p.extend(x.iter().zip(y).map(|(x, y)| [*x, *y]));
        Pwl(p)
    }

    fn segments(&self) -> usize {
        self.0.len() - 1
    }

    /// Slope and intercept of segment k.
    fn line(&self, k: usize) -> (f64, f64) {
        let ([x0, y0], [x1, y1]) = (self.0[k], self.0[k + 1]);
        let g = (y1 - y0) / (x1 - x0);
        (g, y0 - g * x0)
    }

    /// The range of x of segment k.
    fn range(&self, k: usize) -> (f64, f64) {
        let lower = if k == 0 {
            f64::NEG_INFINITY
        } else {
            self.0[k][0]
        };
        let upper = if k + 1 == self.segments() {
            f64::INFINITY
        } else {
            self.0[k + 1][0]
        };
        (lower, upper)
    }

    fn segment_of(&self, x: f64) -> usize {
        (0..self.segments())
            .find(|&k| x <= self.range(k).1)
            .unwrap()
    }

    fn at(&self, x: f64) -> f64 {
        let (g, j) = self.line(self.segment_of(x));
        g * x + j
    }

    /// x of the solution of x + r y(x) = e: the voltage across an element
    /// i(v) fed from a source e behind a resistance r. One segment holds it,
    /// because the left side grows with x.
    fn behind_resistance(&self, e: f64, r: f64) -> f64 {
        for k in 0..self.segments() {
            let (g, j) = self.line(k);
            let x = (e - r * j) / (1.0 + r * g);
            let (lower, upper) = self.range(k);
            if x >= lower && x <= upper {
                return x;
            }
        }
        unreachable!("a monotonic characteristic has a solution")
    }
}

// ---------------------------------------------------------------------------
// Nonlinear resistor

/// E1 behind R1 = 50 ohm across the nonlinear resistor NR1.
fn resistor_circuit(method: &str, waveform: Value, characteristic: Value) -> Value {
    model(
        method,
        1e-4,
        0.04,
        json!([
            {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": waveform},
            {"type": "resistor", "name": "R1", "nodes": ["n1", "n2"], "resistance_ohm": 50.0},
            {"type": "nonlinear_resistor", "name": "NR1", "nodes": ["n2", "gnd"], "characteristic": characteristic},
        ]),
        json!([
            {"type": "node_voltage", "node": "n2"},
            {"type": "branch_current", "component": "NR1"},
            {"type": "branch_current", "component": "R1"},
        ]),
    )
}

#[test]
fn a_nonlinear_resistor_behind_a_resistor_solves_the_algebraic_equations() {
    // v + R i(v) = e(t) has one solution, in one segment, where it is a
    // linear equation. The circuit has no state, so every row must agree
    // with it to rounding: 1e-12 of the source amplitude (the solver accepts
    // a voltage within 1e-12 of the largest table voltage outside a segment,
    // which moves the current by at most that times a slope).
    let tables = [
        // Symmetric, three segments on each side.
        (
            json!({"type": "table", "voltage_v": [100.0, 200.0, 300.0], "current_a": [0.1, 1.0, 10.0], "symmetric": true}),
            Pwl::symmetric(&[100.0, 200.0, 300.0], &[0.1, 1.0, 10.0]),
        ),
        // A dead band: no current below 100 V.
        (
            json!({"type": "table", "voltage_v": [100.0, 200.0], "current_a": [0.0, 4.0], "symmetric": true}),
            Pwl::symmetric(&[100.0, 200.0], &[0.0, 4.0]),
        ),
        // Not symmetric: a forward knee at 1 V, a reverse one at -400 V.
        (
            json!({"type": "table", "voltage_v": [-500.0, -400.0, 0.0, 1.0, 2.0], "current_a": [-5.0, -0.004, 0.0, 0.01, 10.0]}),
            Pwl(vec![
                [-500.0, -5.0],
                [-400.0, -0.004],
                [0.0, 0.0],
                [1.0, 0.01],
                [2.0, 10.0],
            ]),
        ),
    ];
    let (amplitude, r) = (1000.0, 50.0);
    for (k, (characteristic, pwl)) in tables.iter().enumerate() {
        for method in METHODS {
            let sine = json!({"type": "sine", "amplitude": amplitude, "frequency_hz": 50.0});
            let run = run(&resistor_circuit(method, sine, characteristic.clone()));
            report(&format!("resistor table {k}, sine, {method}"), &run);
            let mut segments_seen = std::collections::BTreeSet::new();
            for (n, (t, row)) in run.t.iter().zip(&run.rows).enumerate() {
                // The row at t = 0 is solved with the sources at 1e-6 h.
                let t = if n == 0 { 1e-6 * 1e-4 } else { *t };
                let e = amplitude * (2.0 * PI * 50.0 * t).sin();
                let v = pwl.behind_resistance(e, r);
                segments_seen.insert(pwl.segment_of(v));
                let i = (e - v) / r;
                assert!(
                    (row[0] - v).abs() < 1e-12 * amplitude,
                    "t = {t}: {row:?} against {v}"
                );
                assert!(
                    (row[1] - i).abs() < 1e-12 * amplitude,
                    "t = {t}: {row:?} against {i}"
                );
                assert!(
                    (row[2] - i).abs() < 1e-12 * amplitude,
                    "t = {t}: {row:?} against {i}"
                );
            }
            assert_eq!(segments_seen.len(), pwl.segments(), "every segment is used");
            // A sine moves the voltage through neighbouring segments: the
            // segment of the first solution is the right one (1 iteration),
            // or the one next to it after that (2).
            assert!(run.stats.max_operating_point_iterations <= 2);
        }
        // Direct voltages in every segment, from the first solution on.
        for e in [-900.0, -450.0, -120.0, -30.0, 0.0, 0.5, 60.0, 330.0, 700.0] {
            let dc = json!({"type": "dc", "value": e});
            let run = run(&resistor_circuit("2s-dirk", dc, characteristic.clone()));
            let v = pwl.behind_resistance(e, r);
            for row in &run.rows {
                assert!(
                    (row[0] - v).abs() < 1e-12 * amplitude,
                    "{e} V: {row:?} against {v}"
                );
                assert!((row[1] - (e - v) / r).abs() < 1e-12 * amplitude);
            }
            // Found in the solution at t = 0+ and kept.
            assert!(run.stats.operating_point_iterations <= 3, "{e} V");
        }
    }
}

#[test]
fn two_nonlinear_resistors_in_one_circuit_settle_on_the_only_solution() {
    // E1 - R1 - n2, NR1 from n2 to gnd, R2 from n2 to n3, NR2 from n3 to
    // gnd: the segment of one changes the circuit the other sees. The
    // reference tries every pair of segments; exactly one pair holds its
    // own solution.
    let a = Pwl::symmetric(&[10.0, 20.0, 30.0], &[0.1, 2.0, 2.5]);
    let b = Pwl::symmetric(&[5.0, 8.0, 40.0], &[0.01, 1.0, 1.2]);
    let (r1, r2) = (4.0, 3.0);
    let reference = |e: f64| -> (f64, f64) {
        let mut found = Vec::new();
        for ka in 0..a.segments() {
            for kb in 0..b.segments() {
                let ((ga, ja), (gb, jb)) = (a.line(ka), b.line(kb));
                // (e - v2) / r1 = ga v2 + ja + (v2 - v3) / r2
                // (v2 - v3) / r2 = gb v3 + jb
                let m = [
                    [1.0 / r1 + ga + 1.0 / r2, -1.0 / r2],
                    [-1.0 / r2, 1.0 / r2 + gb],
                ];
                let rhs = [e / r1 - ja, -jb];
                let det = m[0][0] * m[1][1] - m[0][1] * m[1][0];
                let v2 = (rhs[0] * m[1][1] - m[0][1] * rhs[1]) / det;
                let v3 = (m[0][0] * rhs[1] - m[1][0] * rhs[0]) / det;
                let inside = |p: &Pwl, k: usize, v: f64| v > p.range(k).0 && v <= p.range(k).1;
                if inside(&a, ka, v2) && inside(&b, kb, v3) {
                    found.push((v2, v3));
                }
            }
        }
        assert_eq!(found.len(), 1, "{e} V: {found:?}");
        found[0]
    };
    let run = run(&model(
        "2s-dirk",
        1e-4,
        0.02,
        json!([
            {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"],
             "waveform": {"type": "sine", "amplitude": 60.0, "frequency_hz": 50.0}},
            {"type": "resistor", "name": "R1", "nodes": ["n1", "n2"], "resistance_ohm": r1},
            {"type": "nonlinear_resistor", "name": "NR1", "nodes": ["n2", "gnd"],
             "characteristic": {"type": "table", "voltage_v": [10.0, 20.0, 30.0], "current_a": [0.1, 2.0, 2.5], "symmetric": true}},
            {"type": "resistor", "name": "R2", "nodes": ["n2", "n3"], "resistance_ohm": r2},
            {"type": "nonlinear_resistor", "name": "NR2", "nodes": ["n3", "gnd"],
             "characteristic": {"type": "table", "voltage_v": [5.0, 8.0, 40.0], "current_a": [0.01, 1.0, 1.2], "symmetric": true}},
        ]),
        json!([{"type": "node_voltage", "node": "n2"}, {"type": "node_voltage", "node": "n3"}]),
    ));
    report("two coupled nonlinear resistors", &run);
    for (t, row) in run.t.iter().zip(&run.rows).skip(1) {
        let (v2, v3) = reference(60.0 * (2.0 * PI * 50.0 * t).sin());
        assert!(
            (row[0] - v2).abs() < 1e-11 && (row[1] - v3).abs() < 1e-11,
            "t = {t}"
        );
    }
    assert!(run.stats.max_operating_point_iterations <= 4);
}

// ---------------------------------------------------------------------------
// Surge arrester

/// i = p (v / V_ref)^q in two segments that meet at 1.1 V_ref.
const V_REF: f64 = 100e3;
const ARRESTER: [(f64, f64, f64); 2] = [(0.01, 12.0, 0.6), (0.0, 30.0, 1.1)];

fn arrester_segments() -> [(f64, f64, f64); 2] {
    let [(p0, q0, u0), (_, q1, u1)] = ARRESTER;
    // p1 from the meeting point: p0 u1^q0 = p1 u1^q1.
    [(p0, q0, u0), (p0 * u1.powf(q0 - q1), q1, u1)]
}

/// Current of the power law at the voltage v > 0.
fn arrester_current(v: f64) -> f64 {
    let u = v / V_REF;
    let s = arrester_segments();
    if u < s[0].2 {
        // The straight line through the origin below the first segment.
        s[0].0 * s[0].2.powf(s[0].1) * u / s[0].2
    } else if u < s[1].2 {
        s[0].0 * u.powf(s[0].1)
    } else {
        s[1].0 * u.powf(s[1].1)
    }
}

fn arrester_characteristic(tolerance: f64) -> Value {
    let s = arrester_segments();
    json!({"type": "power_law", "reference_voltage_v": V_REF, "max_current_a": 40e3,
    "relative_voltage_tolerance": tolerance,
    "segments": [
        {"coefficient_a": s[0].0, "exponent": s[0].1, "from_voltage_pu": s[0].2},
        {"coefficient_a": s[1].0, "exponent": s[1].1, "from_voltage_pu": s[1].2},
    ]})
}

/// The table that the model makes of the power law.
fn arrester_table(tolerance: f64) -> Pwl {
    let c: openkeito_model::Characteristic =
        serde_json::from_value(arrester_characteristic(tolerance)).unwrap();
    Pwl(c.points())
}

/// A surge current J1 into n1, which a line of surge impedance 400 ohm (R1)
/// and the arrester MOV1 hold to gnd.
fn surge(waveform: Value, tolerance: f64, h: f64, end: f64) -> Run {
    run(&model(
        "2s-dirk",
        h,
        end,
        json!([
            {"type": "current_source", "name": "J1", "nodes": ["gnd", "n1"], "waveform": waveform},
            {"type": "resistor", "name": "R1", "nodes": ["n1", "gnd"], "resistance_ohm": 400.0},
            {"type": "nonlinear_resistor", "name": "MOV1", "nodes": ["n1", "gnd"],
             "characteristic": arrester_characteristic(tolerance)},
        ]),
        json!([
            {"type": "node_voltage", "node": "n1"},
            {"type": "branch_current", "component": "MOV1"},
        ]),
    ))
}

/// The voltage at which the line and the power law together carry `i`:
/// v / 400 + i_arrester(v) = i, by bisection.
fn limited_voltage(i: f64) -> f64 {
    let (mut lo, mut hi) = (0.0, 400.0 * i);
    for _ in 0..200 {
        let mid = 0.5 * (lo + hi);
        if mid / 400.0 + arrester_current(mid) < i {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

#[test]
fn an_arrester_limits_a_surge_to_the_voltage_of_its_characteristic() {
    // A 10 kA step into 400 ohm would give 4 MV; the arrester holds the
    // voltage where its characteristic meets the line, v / 400 + i(v) = I.
    // Against the table the solver agrees to rounding. Against the power
    // law it differs by the error of the table, which is at most
    // relative_voltage_tolerance in voltage at equal current, and less at
    // the crossing with the line (whose current falls as the voltage
    // rises).
    let exact = limited_voltage(10e3);
    assert!((1.67e5..1.68e5).contains(&exact), "{exact}");
    let mut errors = Vec::new();
    for tolerance in [1e-2, 1e-3, 1e-4] {
        let table = arrester_table(tolerance);
        let step = json!({"type": "step", "time_s": 20e-6, "before": 0.0, "after": 10e3});
        let run = surge(step, tolerance, 1e-6, 60e-6);
        report(
            &format!("arrester step, {} segments", table.segments()),
            &run,
        );
        // r = 400 ohm in parallel is a source of 400 I behind 400 ohm.
        let v_table = table.behind_resistance(400.0 * 10e3, 400.0);
        for (t, row) in run.t.iter().zip(&run.rows) {
            if *t < 20.5e-6 {
                assert_eq!(row[0], 0.0, "before the step, t = {t}");
            } else {
                assert!(
                    (row[0] - v_table).abs() < 1e-9 * v_table,
                    "t = {t}: {row:?}"
                );
                assert!((row[1] - table.at(v_table)).abs() < 1e-9 * 10e3);
            }
        }
        let error = (v_table - exact) / exact;
        assert!(
            error <= 0.0 && error.abs() <= tolerance,
            "{tolerance}: {error}"
        );
        errors.push(error.abs());
        // From the segment of the origin to the one of 10 kA in one stage:
        // not more solutions than 2 + log2(segments) (the search halves the
        // range), whatever the number of segments.
        let limit = 2 + (table.segments() as f64).log2().ceil() as u64;
        assert!(
            run.stats.max_operating_point_iterations <= limit,
            "{} segments: {} iterations",
            table.segments(),
            run.stats.max_operating_point_iterations
        );
    }
    println!("arrester: error of the limited voltage for tolerances 1e-2, 1e-3, 1e-4: {errors:?}");
    assert!(errors[2] < errors[0]);
}

#[test]
fn an_arrester_follows_the_front_of_a_surge() {
    // A current that rises as a sine to 20 kA in 8 us (a quarter period at
    // 31.25 kHz) and falls again. The circuit has no state: every row lies
    // on the crossing of the table and the line for the current of its
    // time, to rounding, and within the tolerance of the table of the
    // power law.
    let tolerance = 1e-3;
    let table = arrester_table(tolerance);
    let (peak, f, h) = (20e3, 31.25e3, 0.1e-6);
    let sine = json!({"type": "sine", "amplitude": peak, "frequency_hz": f});
    let run = surge(sine, tolerance, h, 16e-6);
    report(
        &format!("arrester front, {} segments", table.segments()),
        &run,
    );
    let mut worst = 0.0_f64;
    for (n, (t, row)) in run.t.iter().zip(&run.rows).enumerate() {
        let t = if n == 0 { 1e-6 * h } else { *t };
        let i = peak * (2.0 * PI * f * t).sin();
        let v = table.behind_resistance(400.0 * i, 400.0);
        assert!(
            (row[0] - v).abs() < 1e-9 * 2e5,
            "t = {t}: {row:?} against {v}"
        );
        if i > 1.0 {
            worst = worst.max((v - limited_voltage(i)).abs() / limited_voltage(i));
        }
    }
    assert!(worst <= tolerance && worst > 0.0, "{worst}");
    // The crest: 20 kA less the current of the line.
    let crest = run.rows.iter().map(|r| r[0]).fold(0.0, f64::max);
    assert!((crest - limited_voltage(peak)).abs() <= tolerance * crest);
    // The first step takes the current from 0 to 393 A, the voltage from
    // the segment of the origin far up the table: 2 + log2(segments)
    // solutions at most. After that the voltage moves through neighbouring
    // segments, one more solution for each one passed.
    let limit = 2 + (table.segments() as f64).log2().ceil() as u64;
    assert!(run.stats.max_operating_point_iterations <= limit);
    assert!(run.stats.operating_point_iterations <= table.segments() as u64);
}

// ---------------------------------------------------------------------------
// Nonlinear inductor

/// Flux linkages and currents of the test reactor: 0.1 H up to 1 A, 25 mH up
/// to 3 A, 10 mH above.
const FLUX: [f64; 3] = [0.1, 0.15, 0.2];
const CURRENT: [f64; 3] = [1.0, 3.0, 8.0];

fn reactor(name: &str, nodes: [&str; 2]) -> Value {
    json!({"type": "nonlinear_inductor", "name": name, "nodes": nodes,
        "flux_linkage_wb": FLUX, "current_a": CURRENT})
}

/// The exact flux of d(flux)/dt = e - r i(flux) from 0: in each segment an
/// exponential, with the instants at which the flux reaches the next point.
struct StepResponse {
    /// (start time, start flux, final flux, rate) of every segment passed.
    pieces: Vec<(f64, f64, f64, f64)>,
    curve: Pwl,
}

impl StepResponse {
    fn new(e: f64, r: f64) -> StepResponse {
        let curve = Pwl::symmetric(&FLUX, &CURRENT);
        let mut pieces = Vec::new();
        let (mut t, mut flux) = (0.0, 0.0);
        let mut k = curve.segment_of(0.0);
        loop {
            let (g, j) = curve.line(k);
            // d(flux)/dt = e - r j - r g flux = rate (end - flux).
            let (rate, end) = (r * g, (e - r * j) / (r * g));
            pieces.push((t, flux, end, rate));
            let upper = curve.range(k).1;
            if end <= upper {
                break;
            }
            t += ((end - flux) / (end - upper)).ln() / rate;
            flux = upper;
            k += 1;
        }
        StepResponse { pieces, curve }
    }

    fn current(&self, t: f64) -> f64 {
        let (t0, flux0, end, rate) = *self.pieces.iter().rev().find(|p| p.0 <= t).unwrap();
        self.curve
            .at(end + (flux0 - end) * (-(t - t0) * rate).exp())
    }
}

fn step_response_error(method: &str, h: f64) -> (f64, Run) {
    let run = run(&model(
        method,
        h,
        8e-3,
        json!([
            {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {"type": "dc", "value": 100.0}},
            {"type": "resistor", "name": "R1", "nodes": ["n1", "n2"], "resistance_ohm": 10.0},
            reactor("L1", ["n2", "gnd"]),
        ]),
        json!([{"type": "branch_current", "component": "L1"}]),
    ));
    let exact = StepResponse::new(100.0, 10.0);
    let error = run
        .t
        .iter()
        .zip(&run.rows)
        .map(|(t, row)| (row[0] - exact.current(*t)).abs())
        .fold(0.0, f64::max);
    (error, run)
}

#[test]
fn a_nonlinear_inductor_charges_along_the_exponential_of_each_segment() {
    // 100 V on 10 ohm and the reactor: the flux rises through all three
    // segments (time constants 10 ms, 2.5 ms and 1 ms) to 10 A. The exact
    // solution passes the breakpoints at 1.0536 ms and 1.6819 ms, which are
    // not grid points of any of the steps used.
    let exact = StepResponse::new(100.0, 10.0);
    assert_eq!(exact.pieces.len(), 3);
    assert!((exact.pieces[1].0 - 1.0536e-3).abs() < 1e-7);
    assert!((exact.pieces[2].0 - 1.6819e-3).abs() < 1e-7);
    assert!((exact.current(1.0) - 10.0).abs() < 1e-12);

    for method in METHODS {
        // The error of a second-order method in a segment is about
        // (h / tau)^2 / 12 of the current that the segment adds, times a
        // factor near 1 that depends on the method: with tau = 1 ms and
        // h = 0.1 ms, 1e-3 of 7 A. A breakpoint passed within a step adds an
        // error of the same order (below).
        let steps = [2e-4, 1e-4, 5e-5, 2.5e-5, 1.25e-5];
        let errors: Vec<f64> = steps
            .iter()
            .map(|&h| {
                let (e, run) = step_response_error(method, h);
                report(&format!("reactor step response, {method}, h = {h:e}"), &run);
                // The flux moves by one segment at a time.
                assert!(run.stats.max_operating_point_iterations <= 1);
                // Two breakpoints, each passed in one stage (or in both
                // stages of one step with 2S-DIRK).
                assert!(run.stats.operating_point_iterations <= 4);
                e
            })
            .collect();
        let orders: Vec<f64> = errors.windows(2).map(|w| (w[0] / w[1]).log2()).collect();
        println!("reactor step response, {method}: errors {errors:?}, orders {orders:?}");
        assert!(errors[1] < 2e-2, "{method}: {errors:?}");
        // A step that passes a breakpoint is solved with a derivative
        // d(flux)/dt = e - r i(flux) whose slope changes within the step.
        // A second-order method then makes an error of O(h^2) in that one
        // step instead of O(h^3): for the trapezoidal rule, (change of the
        // second derivative) * a (h - a) / 2, where a is the time from the
        // start of the step to the breakpoint. Only two steps pass a
        // breakpoint, so the error at the end stays O(h^2), but with a part
        // that depends on where in the step the breakpoints fall. The order
        // observed between two step sizes therefore scatters around 2
        // (1.2 to 3.1 here), and over the factor of 16 in h it is 1.9 to
        // 2.2.
        let overall = (errors[0] / errors[4]).log2() / 4.0;
        assert!(
            (1.7..2.5).contains(&overall),
            "{method}: order {overall}, {errors:?}"
        );
        for o in &orders {
            assert!((0.8..3.6).contains(o), "{method}: orders {orders:?}");
        }
    }
}

#[test]
fn a_saturable_reactor_on_a_sine_voltage_carries_the_current_of_its_flux() {
    // v = V sin(wt) across the reactor: flux = (V / w)(1 - cos(wt)), which
    // reaches 2 V / w = 0.32 Wb, far into the last segment (10 mH, continued
    // beyond its last point 0.2 Wb): the peak current is
    // 8 A + (0.32 - 0.2) Wb / 10 mH = 20 A.
    let (f, peak_flux) = (50.0, 0.32);
    let w = 2.0 * PI * f;
    let amplitude = 0.5 * peak_flux * w;
    let curve = Pwl::symmetric(&FLUX, &CURRENT);
    assert!((curve.at(peak_flux) - 20.0).abs() < 1e-12);
    for method in METHODS {
        let error = |h: f64| -> (f64, f64, Run) {
            let run = run(&model(
                method,
                h,
                0.04,
                json!([
                    {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"],
                     "waveform": {"type": "sine", "amplitude": amplitude, "frequency_hz": f}},
                    reactor("L1", ["n1", "gnd"]),
                ]),
                json!([{"type": "branch_current", "component": "L1"}]),
            ));
            let exact = |t: f64| curve.at(amplitude / w * (1.0 - (w * t).cos()));
            let e = run
                .t
                .iter()
                .zip(&run.rows)
                .map(|(t, row)| (row[0] - exact(*t)).abs())
                .fold(0.0, f64::max);
            let peak = run.rows.iter().map(|r| r[0]).fold(0.0, f64::max);
            (e, peak, run)
        };
        let (coarse, peak, run) = error(1e-4);
        let (fine, _, _) = error(5e-5);
        report(
            &format!("saturable reactor on a sine voltage, {method}"),
            &run,
        );
        println!(
            "saturable reactor, {method}: peak {peak} A, error {coarse:e} A at 100 us, {fine:e} A at 50 us"
        );
        // The source dictates the voltage, so the flux is its integral by
        // the rule of the method, and the breakpoints change nothing in it.
        // The trapezoidal rule integrates a sine with the amplitude
        // (wh / 2) / tan(wh / 2) = 1 - (wh)^2 / 12: the flux
        // (V / w)(1 - cos(wt)) is short by (wh)^2 / 12 of itself, at its
        // peak 0.32 Wb by 2.6e-5 Wb at h = 100 us, which the last segment
        // (100 A/Wb) turns into 2.6e-3 A. 2S-DIRK is more accurate here.
        let bound = (w * 1e-4).powi(2) / 12.0 * peak_flux * 100.0;
        assert!(coarse <= 1.05 * bound, "{method}: {coarse} against {bound}");
        assert!((peak - 20.0).abs() <= 1.05 * bound, "{method}: peak {peak}");
        let ratio = coarse / fine;
        assert!((3.5..4.5).contains(&ratio), "{method}: ratio {ratio}");
        assert!(run.stats.max_operating_point_iterations <= 1);
        // Six breakpoints per period (three up, three down), two periods.
        assert!(run.stats.operating_point_iterations <= 2 * 12);
    }
}

// ---------------------------------------------------------------------------
// Saturation of transformers

/// 10 kV / 1 kV, 1 MVA, 50 Hz, u_k = 6 % without resistance, i_0 = 0.5 %.
/// Saturation: unsaturated (200 pu) up to 1.2 pu of flux, then 0.4 pu (the
/// air-core reactance).
const SATURATION_FLUX: [f64; 2] = [1.2, 1.5];
const SATURATION_CURRENT: [f64; 2] = [0.006, 0.756];
const RATED_V: f64 = 10e3;
const RATED_S: f64 = 1e6;

fn saturable_transformer(saturation: bool) -> Value {
    let mut t = json!({"type": "transformer", "name": "T1", "windings": [["p1", "gnd"], ["s1", "gnd"]],
        "rated_voltage_kv": [10.0, 1.0], "rated_power_mva": 1.0, "rated_frequency_hz": 50.0,
        "impedance_percent": 6.0, "no_load_current_percent": 0.5});
    if saturation {
        t["saturation"] = json!({"flux_pu": SATURATION_FLUX, "current_pu": SATURATION_CURRENT});
    }
    t
}

/// The peak of the inrush current in per unit when the total flux linkage
/// (leakage of winding 0 and core) reaches `flux_pu`: the current at which
/// x_leak i + flux_core(i) = flux_pu.
fn inrush_peak_pu(flux_pu: f64) -> f64 {
    // The inverse of the saturation characteristic, as a function of the
    // current, plus the leakage of winding 0 (half of 6 %).
    let total = Pwl::symmetric(
        &SATURATION_CURRENT,
        &[
            SATURATION_FLUX[0] + 0.03 * SATURATION_CURRENT[0],
            SATURATION_FLUX[1] + 0.03 * SATURATION_CURRENT[1],
        ],
    );
    // Solve total(i) = flux for i: the function grows, one segment holds it.
    for k in 0..total.segments() {
        let (g, j) = total.line(k);
        let i = (flux_pu - j) / g;
        let (lower, upper) = total.range(k);
        if i >= lower && i <= upper {
            return i;
        }
    }
    unreachable!()
}

#[test]
fn a_saturable_transformer_switched_in_at_a_voltage_zero_draws_the_inrush_of_its_characteristic() {
    // Rated voltage v = V sin(wt) from t = 0 on winding 0, winding 1 open.
    // Without resistance the total flux linkage of winding 0 is the
    // integral of the voltage, (V / w)(1 - cos(wt)): twice the rated flux
    // after half a period. It is shared by the leakage of winding 0
    // (0.03 pu) and the core, whose characteristic gives the current:
    // 2 = 0.03 i + 1.2 + 0.4 (i - 0.006), i = 1.866 pu = 263.9 A, with
    // 1.944 pu of flux in the core.
    let i_base = 2.0_f64.sqrt() * RATED_S / RATED_V;
    let flux_base = 2.0_f64.sqrt() * RATED_V / (2.0 * PI * 50.0);
    let peak_pu = inrush_peak_pu(2.0);
    assert!((peak_pu - 0.8024 / 0.43).abs() < 1e-12, "{peak_pu}");
    let core_flux_pu = 2.0 - 0.03 * peak_pu;
    assert!((core_flux_pu - 1.944).abs() < 1e-3);

    for method in METHODS {
        let inrush = |source_resistance: f64, h: f64| -> (f64, f64, Run) {
            let run = run(&model(
                method,
                h,
                0.02,
                json!([
                    {"type": "voltage_source", "name": "E1", "nodes": ["n0", "gnd"],
                     "waveform": {"type": "sine", "amplitude": 2.0_f64.sqrt() * RATED_V, "frequency_hz": 50.0}},
                    {"type": "resistor", "name": "RS", "nodes": ["n0", "p1"], "resistance_ohm": source_resistance},
                    saturable_transformer(true),
                ]),
                json!([
                    {"type": "branch_current", "component": "T1"},
                    {"type": "node_voltage", "node": "s1"},
                ]),
            ));
            // Columns: I(T1.0), I(T1.1), V(s1).
            let peak = run.rows.iter().map(|r| r[0]).fold(0.0, f64::max);
            // Winding 1 carries no current, so its voltage is that of the
            // core divided by the ratio 10: the flux of the core is its
            // integral (trapezoidal sums of the rows).
            let (mut flux, mut largest) = (0.0, 0.0_f64);
            for k in 1..run.rows.len() {
                flux += 0.5 * h * 10.0 * (run.rows[k][2] + run.rows[k - 1][2]);
                largest = largest.max(flux);
            }
            for row in &run.rows {
                assert!(
                    row[1].abs() < 1e-9 * i_base * 10.0,
                    "winding 1 is open: {row:?}"
                );
            }
            (peak / i_base, largest / flux_base, run)
        };

        // Without resistance only the integration error remains.
        let (peak, flux, run) = inrush(1e-6, 2e-5);
        report(&format!("transformer inrush, {method}"), &run);
        println!(
            "transformer inrush, {method}: peak {peak} pu ({} A), core flux {flux} pu",
            peak * i_base
        );
        // Flux: (wh)^2 / 12 of 1 pu = 3.3e-6 pu; current: that divided by
        // 0.43 pu. The bound leaves a factor of 4 for the breakpoint.
        let wh2 = (2.0 * PI * 50.0 * 2e-5_f64).powi(2) / 12.0;
        assert!(
            (peak - peak_pu).abs() < 4.0 * wh2 / 0.43,
            "{method}: {peak} against {peak_pu}"
        );
        assert!(
            (flux - core_flux_pu).abs() < 1e-4,
            "{method}: {flux} against {core_flux_pu}"
        );
        // The flux enters and leaves the saturated segment once.
        assert!(run.stats.max_operating_point_iterations <= 1);
        assert!(run.stats.operating_point_iterations <= 4);

        // With a source resistance of 0.5 ohm (0.005 pu) the flux at the
        // peak is lower by R times the integral of the current. The current
        // flows only while the core is saturated, from cos(wt) = -0.2 to the
        // peak, 4.36 ms, and is below its peak: the flux falls by less than
        // 0.005 pu * 1.866 pu * (w * 4.36 ms) = 0.0128 pu, and the peak
        // current by less than that divided by 0.43 pu (1.6 % of it).
        let (with_r, _, _) = inrush(0.5, 2e-5);
        let duration = ((-0.2_f64).acos() - PI).abs();
        let bound = 0.005 * peak_pu * duration / 0.43;
        assert!((0.029..0.030).contains(&bound), "{bound}");
        println!(
            "transformer inrush, {method}: with 0.5 ohm {with_r} pu, bound of the drop {bound} pu"
        );
        assert!(
            with_r < peak && peak - with_r < bound,
            "{method}: {with_r} against {peak}"
        );
        // The current is below its peak for most of that time: the drop is
        // about two thirds of the bound.
        assert!(
            peak - with_r > 0.25 * bound,
            "{method}: {with_r} against {peak}"
        );
    }
}

#[test]
fn a_transformer_that_does_not_saturate_is_the_linear_transformer() {
    // At rated voltage the flux stays below the first point (1.2 pu): the
    // run with the characteristic and the run without it differ by rounding
    // (the first slope is 1.2 / 0.006 = 200 pu, the reactance of i_0 =
    // 0.5 %, up to the rounding of that division).
    let circuit = |saturation: bool, voltage_pu: f64| {
        let mut m = model(
            "2s-dirk",
            5e-5,
            0.04,
            json!([
                {"type": "voltage_source", "name": "E1", "nodes": ["n0", "gnd"],
                 "waveform": {"type": "sine", "amplitude": voltage_pu * 2.0_f64.sqrt() * RATED_V, "frequency_hz": 50.0}},
                {"type": "resistor", "name": "RS", "nodes": ["n0", "p1"], "resistance_ohm": 0.5},
                saturable_transformer(saturation),
                {"type": "resistor", "name": "RL", "nodes": ["s1", "gnd"], "resistance_ohm": 2.0},
            ]),
            json!([{"type": "branch_current", "component": "T1"}, {"type": "node_voltage", "node": "s1"}]),
        );
        m["simulation"]["initialization"] = json!("steady_state");
        m
    };
    let (linear, saturable) = (run(&circuit(false, 1.0)), run(&circuit(true, 1.0)));
    assert_eq!(saturable.stats.operating_point_iterations, 0);
    assert!(
        saturable.stats.warnings.is_empty(),
        "{:?}",
        saturable.stats.warnings
    );
    let i_base = 2.0_f64.sqrt() * RATED_S / RATED_V;
    for (a, b) in linear.rows.iter().zip(&saturable.rows) {
        assert!((a[0] - b[0]).abs() < 1e-11 * i_base, "{a:?} against {b:?}");
        assert!((a[2] - b[2]).abs() < 1e-11 * 1e3, "{a:?} against {b:?}");
    }
    // The run stays on its steady state.
    let report = saturable.stats.initialization.as_ref().unwrap();
    assert!(report.relative_voltage_deviation < 1e-4);

    // At 1.3 times the rated voltage the steady state computed with the
    // unsaturated inductance has 1.3 pu of flux, above the first point:
    // warning W260 names the branch and what to do.
    let over = run(&circuit(true, 1.3));
    let w: Vec<&Diagnostic> = over
        .stats
        .warnings
        .iter()
        .filter(|w| w.code == "W260")
        .collect();
    assert_eq!(w.len(), 1, "{:?}", over.stats.warnings);
    assert_eq!(w[0].path, "T1.mag");
    assert!(w[0].message.contains("flux linkage"), "{}", w[0].message);
    assert!(
        w[0].hint.contains("\"initialization\": \"none\""),
        "{}",
        w[0].hint
    );
    assert!(over.stats.operating_point_iterations > 0);
}

/// A 10 MVA, 66 / 6.6 kV bank on an ideal source, low-voltage side open.
fn three_phase_inrush(group: &str, method: &str, h: f64) -> Run {
    let mut components = vec![
        // Phase a: v = V sin(wt), a voltage zero at t = 0.
        json!({"type": "three_phase_source", "name": "G1", "bus": "b1", "line_voltage_rms_v": 66e3,
            "frequency_hz": 50.0, "angle_deg": -90.0}),
        json!({"type": "three_phase_transformer", "name": "T1", "buses": ["b1", "b2"], "vector_group": group,
            "rated_voltage_kv": [66.0, 6.6], "rated_power_mva": 10.0, "rated_frequency_hz": 50.0,
            "impedance_percent": 6.0, "no_load_current_percent": 0.5,
            "saturation": {"flux_pu": SATURATION_FLUX, "current_pu": SATURATION_CURRENT}}),
    ];
    if group.ends_with("d1") {
        // A delta floats without a reference.
        components.push(json!({"type": "resistor", "name": "RG", "nodes": ["b2.a", "gnd"], "resistance_ohm": 1e6}));
    }
    run(&model(
        method,
        h,
        0.02,
        Value::Array(components),
        json!([{"type": "branch_current", "component": "T1"}]),
    ))
}

#[test]
fn the_units_of_a_three_phase_bank_saturate_each_with_its_own_flux() {
    // YNyn0 on an ideal source with the neutral grounded: three separate
    // units. With v_a = V sin(wt), the total flux of phase p is
    // (V / w)(cos(th) - cos(wt + th)), th = 0, -120, +120 degrees: phase a
    // reaches +2 pu, phases b and c reach -1.5 pu (and +0.5 pu). The peak
    // currents follow from the characteristic as for the single-phase unit:
    // 1.866 pu and -(1.5 - 1.2 + 0.0024) / 0.43 = -0.7033 pu, on the base
    // sqrt(2) (S / 3) / (V_LL / sqrt(3)).
    let i_base = 2.0_f64.sqrt() * (10e6 / 3.0) / (66e3 / 3.0_f64.sqrt());
    for method in METHODS {
        let h = 2e-5;
        let run = three_phase_inrush("YNyn0", method, h);
        report(&format!("three-phase inrush YNyn0, {method}"), &run);
        // Columns: I(T1.0.a), .b, .c, I(T1.1.a), .b, .c.
        let extreme = |col: usize, sign: f64| {
            run.rows.iter().map(|r| sign * r[col]).fold(0.0, f64::max) / i_base
        };
        let wh2 = (2.0 * PI * 50.0 * h).powi(2) / 12.0;
        let tol = 4.0 * wh2 / 0.43;
        let (a, b, c) = (extreme(0, 1.0), extreme(1, -1.0), extreme(2, -1.0));
        println!("three-phase inrush, {method}: peaks {a}, -{b}, -{c} pu");
        assert!((a - inrush_peak_pu(2.0)).abs() < tol, "{method}: a {a}");
        assert!((b - inrush_peak_pu(1.5)).abs() < tol, "{method}: b {b}");
        assert!((c - inrush_peak_pu(1.5)).abs() < tol, "{method}: c {c}");
        assert!((inrush_peak_pu(1.5) - 0.3024 / 0.43).abs() < 1e-12);
        // No unit saturates in the other direction: +0.5 pu of flux.
        assert!(extreme(1, 1.0) < 0.5 * 0.006 * 1.01);
        assert!(run.stats.max_operating_point_iterations <= 2);
    }
}

#[test]
fn units_that_share_an_isolated_star_point_settle_together() {
    // Yd1 with the star point isolated: the three magnetizing currents add
    // up to zero at the star point, so the segment of one unit changes the
    // voltages of the other two (and the delta carries the zero-sequence
    // flux change as a circulating current). There is no closed form; the
    // currents into the star side must add up to zero, the result must not
    // depend on the step beyond the error of the method, and the search
    // must settle in a few solutions although the units move together.
    let i_base = 2.0_f64.sqrt() * (10e6 / 3.0) / (66e3 / 3.0_f64.sqrt());
    for method in METHODS {
        let coarse = three_phase_inrush("Yd1", method, 4e-5);
        let fine = three_phase_inrush("Yd1", method, 2e-5);
        report(
            &format!("three-phase inrush Yd1 (isolated star), {method}"),
            &coarse,
        );
        let mut largest = 0.0_f64;
        let mut difference = 0.0_f64;
        for (k, row) in coarse.rows.iter().enumerate() {
            let sum = row[0] + row[1] + row[2];
            assert!(sum.abs() < 1e-9 * i_base, "t = {}: sum {sum}", coarse.t[k]);
            for p in 0..3 {
                largest = largest.max(row[p].abs());
                difference = difference.max((row[p] - fine.rows[2 * k][p]).abs());
            }
        }
        println!(
            "three-phase inrush Yd1, {method}: largest current {} pu, difference between h and h/2 {} pu",
            largest / i_base,
            difference / i_base
        );
        // The units saturate: more than the unsaturated magnetizing
        // current by far.
        assert!(largest > 0.2 * i_base, "{method}: {largest}");
        // (wh)^2 / 12 = 1.3e-5 of the flux at the coarse step, turned into
        // current by 1 / 0.43 pu, with a factor for the breakpoints.
        assert!(difference < 1e-3 * i_base, "{method}: {difference}");
        assert!(coarse.stats.max_operating_point_iterations <= 4, "{method}");
    }
}

// ---------------------------------------------------------------------------
// Together with diodes

/// The reference for the rectifier below: d(flux)/dt = v(t) - r i(flux),
/// where r is the load plus the resistance of the diode in its state, by
/// the exact solution of every segment over steps of `dt` with the source
/// taken at their middle. Returns the current at the times k * `every` * dt.
fn rectifier_reference(
    amplitude: f64,
    w: f64,
    load: f64,
    dt: f64,
    steps: usize,
    every: usize,
) -> Vec<f64> {
    let curve = Pwl::symmetric(&FLUX, &CURRENT);
    let (r_on, r_off) = (1e-3, 1e6);
    let mut flux = 0.0_f64;
    let mut out = vec![0.0];
    for n in 0..steps {
        let v = amplitude * (w * (n as f64 + 0.5) * dt).sin();
        let mut left = dt;
        // The diode conducts while the current is positive, or when the
        // source would drive a positive current through it.
        while left > 0.0 {
            let i = curve.at(flux);
            let on = i > 0.0 || (i == 0.0 && v > 0.0);
            let r = load + if on { r_on } else { r_off };
            let k = curve.segment_of(if v - r * i >= 0.0 {
                flux + 1e-15
            } else {
                flux - 1e-15
            });
            let (g, j) = curve.line(k);
            let (rate, end) = (r * g, (v - r * j) / (r * g));
            // Until the flux reaches an end of the segment or zero.
            let (lower, upper) = curve.range(k);
            let mut stops = vec![lower, upper];
            if flux != 0.0 {
                stops.push(0.0);
            }
            let mut time = left;
            let mut target = None;
            for s in stops {
                let reached = s.is_finite() && (end - s) * (flux - s) < 0.0;
                if reached {
                    let ts = ((end - flux) / (end - s)).ln() / rate;
                    if ts < time {
                        (time, target) = (ts, Some(s));
                    }
                }
            }
            flux = target.unwrap_or(end + (flux - end) * (-time * rate).exp());
            left -= time;
        }
        if (n + 1) % every == 0 {
            out.push(curve.at(flux));
        }
    }
    out
}

#[test]
fn a_rectifier_with_a_saturable_reactor_settles_diodes_and_segments_together() {
    // A sine source, the reactor, a diode and a 5 ohm load in series. The
    // diode conducts while the flux of the reactor is positive: the flux
    // rises through the breakpoints in the positive half period and comes
    // back in the negative one, and the diode blocks when it reaches zero,
    // in the segment of the origin. Segments and the diode state are decided
    // on the same solutions (the trial solution after every change of the
    // diode is iterated for the segments first).
    let (f, amplitude, load) = (50.0, 60.0, 5.0);
    let w = 2.0 * PI * f;
    let (h, periods) = (5e-5, 3.0);
    let steps = (periods / f / h).round() as usize;
    let reference = rectifier_reference(amplitude, w, load, h / 100.0, steps * 100, 100);
    for method in METHODS {
        let run = run(&model(
            method,
            h,
            periods / f,
            json!([
                {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"],
                 "waveform": {"type": "sine", "amplitude": amplitude, "frequency_hz": f}},
                reactor("L1", ["n1", "n2"]),
                {"type": "diode", "name": "D1", "nodes": ["n2", "n3"]},
                {"type": "resistor", "name": "R1", "nodes": ["n3", "gnd"], "resistance_ohm": load},
            ]),
            json!([{"type": "branch_current", "component": "L1"}]),
        ));
        report(
            &format!("rectifier with a saturable reactor, {method}"),
            &run,
        );
        let peak = reference.iter().fold(0.0_f64, |a, b| a.max(*b));
        assert!(peak > 8.0, "the reactor saturates: {peak} A");
        let error = run
            .rows
            .iter()
            .zip(&reference)
            .map(|(row, i)| (row[0] - i).abs())
            .fold(0.0, f64::max);
        println!(
            "rectifier with a saturable reactor, {method}: peak {peak} A, error {error:e} A, events {}",
            run.stats.events.len()
        );
        // The diode turns on at the grid point after the source becomes
        // positive and off one step after the current has crossed zero: an
        // instant moves by up to one step, in which the flux changes by up
        // to V h (at turn-off the voltage is far below its amplitude) and
        // the current by up to V h / 0.1 H in the first segment = 0.03 A.
        // The integration error is smaller (second order).
        assert!(error < amplitude * h / 0.1, "{method}: {error}");
        // One closing and one opening per period.
        let actions: Vec<&str> = run.stats.events.iter().map(|e| e.action).collect();
        assert_eq!(
            actions,
            ["close", "open", "close", "open", "close", "open"],
            "{method}"
        );
        assert!(run.stats.max_operating_point_iterations <= 2);
    }
}

#[test]
fn a_switch_onto_a_diode_and_an_arrester_settles_both_in_the_same_solution() {
    // 300 kV closes at 1 ms onto a diode in series with 100 ohm and the
    // arrester of the surge tests. In the trial solution that settles the
    // diode after the switch has closed, the diode is first taken as
    // blocking and the arrester stays near the origin; with the diode
    // conducting the arrester moves far up its characteristic. The result
    // is static: e = (100 ohm + R_on) i + v(i).
    let table = arrester_table(1e-3);
    let run = run(&model(
        "2s-dirk",
        1e-4,
        2e-3,
        json!([
            {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {"type": "dc", "value": 300e3}},
            {"type": "switch", "name": "S1", "nodes": ["n1", "n2"], "close_time_s": 1e-3},
            {"type": "diode", "name": "D1", "nodes": ["n2", "n3"]},
            {"type": "resistor", "name": "R1", "nodes": ["n3", "n4"], "resistance_ohm": 100.0},
            {"type": "nonlinear_resistor", "name": "MOV1", "nodes": ["n4", "gnd"], "characteristic": arrester_characteristic(1e-3)},
        ]),
        json!([{"type": "node_voltage", "node": "n4"}, {"type": "branch_current", "component": "MOV1"}]),
    ));
    report("switch onto a diode and an arrester", &run);
    let v = table.behind_resistance(300e3, 100.0 + 1e-3);
    assert!(v > 1.5e5 && v < 1.6e5, "{v}");
    for (t, row) in run.t.iter().zip(&run.rows) {
        if *t < 1.05e-3 {
            assert_eq!(row[0], 0.0, "t = {t}");
        } else {
            assert!(
                (row[0] - v).abs() < 1e-8 * v,
                "t = {t}: {row:?} against {v}"
            );
            assert!((row[1] - table.at(v)).abs() < 1e-8 * table.at(v));
        }
    }
    let events: Vec<(&str, &str)> = run
        .stats
        .events
        .iter()
        .map(|e| (e.component.as_str(), e.action))
        .collect();
    assert_eq!(events, [("S1", "close"), ("D1", "close")]);
    let limit = 2 + (table.segments() as f64).log2().ceil() as u64;
    assert!(run.stats.max_operating_point_iterations <= limit);
}

#[test]
fn the_flux_of_every_nonlinear_inductor_is_offered_as_a_signal() {
    let signals = |components: Value| -> Vec<String> {
        let m = model("2s-dirk", 1e-5, 1e-3, components, json!([]));
        let loaded = load(&m).unwrap_or_else(|d| panic!("model is not valid: {d:?}"));
        let sim = Simulator::from_model(&loaded.model).unwrap();
        let names = sim.element_signals().map(str::to_string).collect();
        names
    };
    let source = json!({"type": "voltage_source", "name": "E1", "nodes": ["p1", "gnd"],
        "waveform": {"type": "sine", "amplitude": 1e4, "frequency_hz": 50.0}});
    assert_eq!(
        signals(json!([source, reactor("L1", ["p1", "gnd"])])),
        ["L1.flux_wb"]
    );
    assert_eq!(
        signals(json!([source, saturable_transformer(true)])),
        ["T1.mag.flux_wb"]
    );
    // A linear magnetizing branch offers nothing.
    assert!(signals(json!([source, saturable_transformer(false)])).is_empty());
    let bank = json!({"type": "three_phase_transformer", "name": "T1", "buses": [["p1", "p2", "p3"], "b2"],
        "vector_group": "YNyn0", "rated_voltage_kv": [66.0, 6.6], "rated_power_mva": 10.0,
        "rated_frequency_hz": 50.0, "impedance_percent": 6.0, "no_load_current_percent": 0.5,
        "saturation": {"flux_pu": SATURATION_FLUX, "current_pu": SATURATION_CURRENT}});
    assert_eq!(
        signals(json!([
            source,
            bank,
            {"type": "resistor", "name": "R2", "nodes": ["p2", "gnd"], "resistance_ohm": 1.0},
            {"type": "resistor", "name": "R3", "nodes": ["p3", "gnd"], "resistance_ohm": 1.0},
        ])),
        ["T1.mag.a.flux_wb", "T1.mag.b.flux_wb", "T1.mag.c.flux_wb"]
    );
}

// ---------------------------------------------------------------------------
// Validation

fn codes(model: &Value) -> Vec<(&'static str, String)> {
    match load(model) {
        Ok(l) => l.warnings.into_iter().map(|d| (d.code, d.path)).collect(),
        Err(d) => d.into_iter().map(|d| (d.code, d.path)).collect(),
    }
}

fn with_component(component: Value) -> Value {
    model(
        "2s-dirk",
        1e-5,
        1e-3,
        json!([
            {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {"type": "dc", "value": 1.0}},
            {"type": "resistor", "name": "R1", "nodes": ["n1", "n2"], "resistance_ohm": 1.0},
            component,
        ]),
        json!([{"type": "node_voltage", "node": "n2"}]),
    )
}

#[test]
fn characteristics_that_cannot_be_solved_are_rejected_with_their_place() {
    let resistor = |characteristic: Value| {
        with_component(
            json!({"type": "nonlinear_resistor", "name": "NR1", "nodes": ["n2", "gnd"], "characteristic": characteristic}),
        )
    };
    let c = "components[2].characteristic";
    let table = |v: Value, i: Value, symmetric: bool| {
        resistor(json!({"type": "table", "voltage_v": v, "current_a": i, "symmetric": symmetric}))
    };
    assert_eq!(
        codes(&table(json!([1.0, 2.0]), json!([1.0]), true)),
        [("E260", c.to_string())]
    );
    assert_eq!(
        codes(&table(json!([1.0]), json!([1.0]), true)),
        [("E260", c.to_string())]
    );
    assert_eq!(
        codes(&table(json!([1.0, 1.0]), json!([1.0, 2.0]), true)),
        [("E261", format!("{c}.voltage_v[1]"))]
    );
    assert_eq!(
        codes(&table(json!([1.0, 2.0]), json!([1.0, 0.5]), true)),
        [("E261", format!("{c}.current_a[1]"))]
    );
    // With symmetric, the points are those of positive voltages.
    assert_eq!(
        codes(&table(json!([-1.0, 2.0]), json!([-1.0, 2.0]), true)),
        [("E261", format!("{c}.voltage_v[0]"))]
    );
    // Without it, the origin must be a point.
    assert_eq!(
        codes(&table(json!([-1.0, 2.0]), json!([-1.0, 2.0]), false)),
        [("E262", c.to_string())]
    );
    assert!(codes(&table(
        json!([-1.0, 0.0, 2.0]),
        json!([-1.0, 0.0, 3.0]),
        false
    ))
    .is_empty());
    // A dead band is an open circuit: it does not hold a node.
    let mut floating = table(json!([1.0, 2.0]), json!([0.0, 2.0]), true);
    floating["components"][1]["nodes"] = json!(["n1", "gnd"]);
    assert_eq!(codes(&floating), [("E141", "components".to_string())]);

    let power_law = |segments: Value, max: f64| {
        resistor(
            json!({"type": "power_law", "reference_voltage_v": 1e5, "segments": segments, "max_current_a": max}),
        )
    };
    let segment =
        |p: f64, q: f64, u: f64| json!({"coefficient_a": p, "exponent": q, "from_voltage_pu": u});
    assert!(codes(&power_law(json!([segment(1.0, 20.0, 0.5)]), 1e4)).is_empty());
    assert_eq!(
        codes(&power_law(json!([segment(1.0, 0.5, 0.5)]), 1e4)),
        [("E130", format!("{c}.segments[0].exponent"))]
    );
    assert_eq!(
        codes(&power_law(
            json!([segment(1.0, 20.0, 1.0), segment(1.0, 30.0, 1.0)]),
            1e4
        )),
        [("E263", format!("{c}.segments[1].from_voltage_pu"))]
    );
    assert_eq!(
        codes(&power_law(json!([segment(1.0, 20.0, 1.2)]), 1.0)),
        [("E263", format!("{c}.max_current_a"))]
    );
    // The second segment starts below what the first one has reached.
    assert_eq!(
        codes(&power_law(
            json!([segment(1.0, 20.0, 1.0), segment(1e-9, 30.0, 1.1)]),
            1e4
        )),
        [("E263", format!("{c}.segments[1].from_voltage_pu"))]
    );
    // Also when it starts above the start of the first one (1 A) but below
    // the last point that the table takes from it (4.6 A at 1.079 V_ref):
    // 0.25 * 1.1^30 = 4.4 A.
    assert_eq!(
        codes(&power_law(
            json!([segment(1.0, 20.0, 1.0), segment(0.25, 30.0, 1.1)]),
            1e4
        )),
        [("E263", format!("{c}.segments[1].from_voltage_pu"))]
    );
    // It starts above the first one: a warning, and the table still rises.
    assert_eq!(
        codes(&power_law(
            json!([segment(1.0, 20.0, 1.0), segment(0.5, 30.0, 1.1)]),
            1e4
        )),
        [("W261", format!("{c}.segments[1].from_voltage_pu"))]
    );

    let inductor = |flux: Value, current: Value| {
        with_component(
            json!({"type": "nonlinear_inductor", "name": "L1", "nodes": ["n2", "gnd"],
            "flux_linkage_wb": flux, "current_a": current}),
        )
    };
    assert!(codes(&inductor(json!([1.0, 1.2]), json!([1.0, 10.0]))).is_empty());
    // A flat part would be an infinite inductance, a vertical one none.
    assert_eq!(
        codes(&inductor(json!([1.0, 1.2]), json!([1.0, 1.0]))),
        [("E261", "components[2].current_a[1]".to_string())]
    );
    assert_eq!(
        codes(&inductor(json!([1.0, 1.0]), json!([1.0, 2.0]))),
        [("E261", "components[2].flux_linkage_wb[1]".to_string())]
    );
    assert_eq!(
        codes(&inductor(json!([1.0]), json!([1.0]))),
        [("E260", "components[2]".to_string())]
    );
}

#[test]
fn the_saturation_of_a_transformer_must_fit_its_no_load_current() {
    let transformer = |i0: f64, loss: f64, saturation: Value, version: &str| {
        let mut m = with_component(
            json!({"type": "transformer", "name": "T1", "windings": [["n2", "gnd"], ["s1", "gnd"]],
            "rated_voltage_kv": [10.0, 1.0], "rated_power_mva": 1.0, "rated_frequency_hz": 50.0,
            "impedance_percent": 6.0, "no_load_current_percent": i0, "no_load_loss_kw": loss,
            "saturation": saturation}),
        );
        m["version"] = json!(version);
        m
    };
    let good = json!({"flux_pu": [1.2, 1.5], "current_pu": [0.006, 0.756]});
    assert!(codes(&transformer(0.5, 0.0, good.clone(), "0.5")).is_empty());
    // The field belongs to format 0.5.
    let old = load(&transformer(0.5, 0.0, good.clone(), "0.4")).unwrap_err();
    assert_eq!(old[0].code, "E102");
    assert!(
        old[0].hint.contains("\"version\": \"0.5\""),
        "{}",
        old[0].hint
    );
    assert!(old[0].hint.contains("saturation"), "{}", old[0].hint);
    // Without the field, an older version is enough.
    let mut linear = transformer(0.5, 0.0, good.clone(), "0.3");
    linear["components"][2]
        .as_object_mut()
        .unwrap()
        .remove("saturation");
    assert!(codes(&linear).is_empty());

    // No magnetizing branch to saturate.
    let d = load(&transformer(0.0, 0.0, good.clone(), "0.5")).unwrap_err();
    assert_eq!(
        (d[0].code, d[0].path.as_str()),
        ("E264", "components[2].saturation")
    );
    // The first slope must be the reactance 1 / 0.005 = 200 pu; the hint
    // gives the current that fits.
    let d = load(&transformer(1.0, 0.0, good.clone(), "0.5")).unwrap_err();
    assert_eq!(
        (d[0].code, d[0].path.as_str()),
        ("E264", "components[2].saturation.current_pu[0]")
    );
    assert!(d[0].hint.contains("1.200000e-2"), "{}", d[0].hint);
    // With a core loss of 3 kW (0.3 %) the susceptance is 0.4 %: 250 pu.
    let with_loss = json!({"flux_pu": [1.2, 1.5], "current_pu": [0.0048, 0.756]});
    assert!(codes(&transformer(0.5, 3.0, with_loss, "0.5")).is_empty());
    assert_eq!(
        codes(&transformer(0.5, 3.0, good, "0.5")),
        [("E264", "components[2].saturation.current_pu[0]".to_string())]
    );
    // The points are checked like those of a nonlinear inductor.
    let bad = json!({"flux_pu": [1.2, 1.1], "current_pu": [0.006, 0.756]});
    assert_eq!(
        codes(&transformer(0.5, 0.0, bad, "0.5")),
        [("E261", "components[2].saturation.flux_pu[1]".to_string())]
    );
}

#[test]
fn a_steady_state_beyond_the_origin_segment_of_a_resistor_gives_w260() {
    // 150 V behind 50 ohm on a resistor that is 1000 ohm up to 100 V: the
    // phasor solution gives 142.9 V at its terminals, beyond the segment.
    let mut m = resistor_circuit(
        "2s-dirk",
        json!({"type": "sine", "amplitude": 150.0, "frequency_hz": 50.0}),
        json!({"type": "table", "voltage_v": [100.0, 200.0], "current_a": [0.1, 4.0], "symmetric": true}),
    );
    m["simulation"]["initialization"] = json!("steady_state");
    let over = run(&m);
    let w: Vec<&Diagnostic> = over
        .stats
        .warnings
        .iter()
        .filter(|w| w.code == "W260")
        .collect();
    assert_eq!(w.len(), 1, "{:?}", over.stats.warnings);
    assert_eq!(w[0].path, "NR1");
    assert!(w[0].message.contains("1.428571"), "{}", w[0].message);
    // Within the segment there is nothing to say, and the run stays on the
    // steady state.
    m["components"][0]["waveform"]["amplitude"] = json!(100.0);
    let within = run(&m);
    assert!(
        within.stats.warnings.is_empty(),
        "{:?}",
        within.stats.warnings
    );
    assert_eq!(within.stats.operating_point_iterations, 0);
}

#[test]
fn saturable_transformer_hv_tap_change_preserves_flux_per_turn_and_avoids_spurious_inrush() {
    // Steady-state saturable transformer magnetized at 1.0 pu peak flux
    // (knee at 1.2 pu). Stepped at peak flux zero-crossing vs off-zero: either
    // way, a +5% HV tap (10 kV -> 10.5 kV) keeps the core below 1.2 pu without
    // jumping above the knee.
    let mut m = model(
        "2s-dirk",
        2e-5,
        0.10,
        json!([
            {"type": "voltage_source", "name": "E1", "nodes": ["p1", "gnd"],
             "waveform": {"type": "sine", "amplitude": RATED_V * 2.0_f64.sqrt(), "frequency_hz": 50.0}},
            {
                "type": "transformer", "name": "T1", "windings": [["p1", "gnd"], ["s1", "gnd"]],
                "rated_voltage_kv": [10.0, 1.0], "rated_power_mva": 1.0, "rated_frequency_hz": 50.0,
                "impedance_percent": 6.0, "no_load_current_percent": 0.5,
                "saturation": {"flux_pu": SATURATION_FLUX, "current_pu": SATURATION_CURRENT},
                "tap_changes": [{"time_s": 0.02, "rated_voltage_kv": [10.5, 1.0]}]
            },
            {"type": "resistor", "name": "RL", "nodes": ["s1", "gnd"], "resistance_ohm": 1.0}
        ]),
        json!([
            {"type": "node_voltage", "node": "s1"},
            {"type": "branch_current", "component": "T1"},
        ]),
    );
    m["version"] = json!("0.6");
    m["simulation"]["initialization"] = json!("steady_state");
    let r = run(&m);
    assert_eq!(r.stats.events.len(), 1);
    assert_eq!(r.stats.events[0].action, "step");
    // Secondary voltage at the end matches (1.0 / 10.5) * 10e3 loaded through 6% leakage on 1 ohm base.
    let v1_col = r.col("V(s1)");
    let i0_col = r.col("I(T1.0)");
    let i_peak_after =
        r.t.iter()
            .zip(&r.rows)
            .filter(|(t, _)| **t >= 0.02)
            .map(|(_, row)| row[i0_col].abs())
            .fold(0.0_f64, f64::max);
    // Rated peak primary current is sqrt(2) * (1 MVA / 10 kV) = 141.4 A; saturated inrush would exceed 200 A.
    assert!(
        i_peak_after < 160.0,
        "unexpected inrush peak {i_peak_after} A"
    );
    let v_rms_end = {
        let samples: Vec<f64> =
            r.t.iter()
                .zip(&r.rows)
                .filter(|(t, _)| **t >= 0.08)
                .map(|(_, row)| row[v1_col])
                .collect();
        (samples.iter().map(|v| v * v).sum::<f64>() / samples.len() as f64).sqrt()
    };
    let v_ideal = 10e3 * (1.0 / 10.5);
    assert!(
        (v_rms_end - v_ideal).abs() / v_ideal < 0.02,
        "v_rms_end = {v_rms_end} vs {v_ideal}"
    );
}
