//! Steady-state initialization and frequency scans against hand
//! calculations.
//!
//! Phasors are rms with a cosine reference: x(t) = sqrt(2) |X| cos(wt + arg X).
//!
//! Tolerances: a run that starts from the exact (continuous) steady state
//! departs from it by the error of the integration method in the forced
//! response, about k (w h)^2 of the amplitude with k = 1/12 for the
//! trapezoidal rule and k = 0.01 to 0.03 for 2S-DIRK (see three_phase.rs).
//! The checks use (w h)^2. Phasor solutions and scans have no time step; they
//! agree with the hand calculations to rounding (checked at 1e-10).

mod common;

use std::f64::consts::PI;

use common::phasors::*;
use common::{run_model as run, Run};
use openkeito_core::FrequencyScanner;
use serde_json::{json, Value};

impl Run {
    /// Largest |x(t) - sqrt(2) Re(X e^{jwt})| of a column over t <= until.
    fn departure(&self, name: &str, x: Complex64, until: f64) -> f64 {
        let k = self.col(name);
        self.t
            .iter()
            .zip(&self.rows)
            .filter(|(t, _)| **t <= until + 1e-12)
            .map(|(t, row)| {
                let ss = 2.0_f64.sqrt() * (x * Complex64::from_polar(1.0, W * t)).re;
                (row[k] - ss).abs()
            })
            .fold(0.0, f64::max)
    }

    fn steady_phasor(&self, name: &str) -> Complex64 {
        let init = self.stats.initialization.as_ref().expect("initialized run");
        init.phasors
            .iter()
            .find(|p| p.name == name)
            .unwrap_or_else(|| panic!("no phasor {name}"))
            .value
    }
}

fn model(method: &str, h: f64, end: f64, init: &str, components: Value, outputs: Value) -> Value {
    model_of(method, h, end, Some(init), components, outputs)
}

// ------------------------------------------------------------ the network

/// 66 kV source behind Zs (Z1 = 0.5 + j5, Z0 = 1 + j15 ohm), a coupled line
/// (Z1 = 1 + j4, Z0 = 3 + j12 ohm) to bus f, a grounded star load at f
/// (Z1 = Z0 = 40 + j15 ohm) and a capacitor of 20 uF from each phase of f to
/// ground.
const V_LL: f64 = 66_000.0;
const ANGLE: f64 = -15.0;
const C_SHUNT: f64 = 20e-6;

fn zs(seq: usize, w: f64) -> Complex64 {
    [c(1.0, 15.0 * w / W), c(0.5, 5.0 * w / W)][seq]
}

fn zl(seq: usize, w: f64) -> Complex64 {
    [c(3.0, 12.0 * w / W), c(1.0, 4.0 * w / W)][seq]
}

fn zload(w: f64) -> Complex64 {
    parallel(c(40.0, 15.0 * w / W), 1.0 / c(0.0, w * C_SHUNT))
}

fn network() -> Vec<Value> {
    let mut v = vec![
        json!({"type": "three_phase_source", "name": "G1", "bus": "s", "line_voltage_rms_v": V_LL,
               "frequency_hz": F, "angle_deg": ANGLE,
               "impedance": {"type": "sequence", "r1_ohm": 0.5, "l1_h": 5.0 / W, "r0_ohm": 1.0, "l0_h": 15.0 / W}}),
        json!({"type": "coupled_rl", "name": "LINE", "buses": ["s", "f"],
               "impedance": {"type": "sequence", "r1_ohm": 1.0, "l1_h": 4.0 / W, "r0_ohm": 3.0, "l0_h": 12.0 / W}}),
        json!({"type": "coupled_rl", "name": "LOAD", "buses": ["f", "gnd"],
               "impedance": {"type": "sequence", "r1_ohm": 40.0, "l1_h": 15.0 / W, "r0_ohm": 40.0, "l0_h": 15.0 / W}}),
    ];
    for p in ["a", "b", "c"] {
        v.push(json!({"type": "capacitor", "name": format!("C{p}"), "nodes": [format!("f.{p}"), "gnd"], "capacitance_f": C_SHUNT}));
    }
    v
}

fn e_a() -> Complex64 {
    polar(V_LL / 3.0_f64.sqrt(), ANGLE)
}

/// Pre-fault (positive-sequence) voltage of bus f, phase a.
fn v_f() -> Complex64 {
    e_a() * zload(W) / (zs(1, W) + zl(1, W) + zload(W))
}

#[test]
fn the_steady_state_is_the_phasor_solution_and_has_no_transient() {
    let h = 2e-5;
    let tol = (W * h).powi(2);
    for method in METHODS {
        let r = run(&model(
            method,
            h,
            0.1,
            "steady_state",
            json!(network()),
            json!([{"type": "bus_voltage", "bus": "f"},
                   {"type": "branch_current", "component": "LINE"},
                   {"type": "branch_current", "component": "Ca"}]),
        ));
        // The phasor solution against the hand calculation.
        let a = polar(1.0, -120.0);
        for (p, shift) in [("a", c(1.0, 0.0)), ("b", a), ("c", a * a)] {
            let v = r.steady_phasor(&format!("V(f.{p})"));
            check(
                &format!("{method} phasor V(f.{p})"),
                v,
                v_f() * shift,
                1e-10,
            );
        }
        let i_line = (e_a() - v_f()) / (zs(1, W) + zl(1, W));
        check(
            "phasor I(LINE.a)",
            r.steady_phasor("I(LINE.a)"),
            i_line,
            1e-10,
        );
        check(
            "phasor I(Ca)",
            r.steady_phasor("I(Ca)"),
            v_f() * c(0.0, W * C_SHUNT),
            1e-10,
        );
        // The waveforms stay on the steady state for 5 periods.
        let init = r.stats.initialization.as_ref().unwrap();
        assert_eq!(init.checked_until_s, 0.1);
        eprintln!(
            "{method}: departure {:.2e} (voltage), {:.2e} (current)",
            init.relative_voltage_deviation,
            init.relative_current_deviation.unwrap()
        );
        assert!(init.relative_voltage_deviation < tol, "{init:?}");
        assert!(init.relative_current_deviation.unwrap() < tol, "{init:?}");
        assert!(r.stats.warnings.is_empty());
        // The report covers the waveform itself (its maximum is over every
        // node; relative values are on the largest amplitude, the source's).
        let mine = r.departure("V(f.a)", v_f(), 0.1);
        assert!(mine > 0.0 && mine <= init.max_voltage_deviation_v * (1.0 + 1e-9));

        // From rest the same circuit starts with offsets of the size of the
        // steady state itself.
        let rest = run(&model(
            method,
            h,
            0.1,
            "none",
            json!(network()),
            json!([{"type": "bus_voltage", "bus": "f"}, {"type": "branch_current", "component": "LINE"}]),
        ));
        assert!(rest.stats.initialization.is_none());
        let amplitude = 2.0_f64.sqrt() * i_line.norm();
        let departure = rest.departure("I(LINE.a)", i_line, 0.1) / amplitude;
        eprintln!("{method}: from rest the line current departs by {departure:.2}");
        assert!(departure > 0.3, "{departure}");
    }
}

#[test]
fn the_departure_from_the_steady_state_is_of_second_order() {
    for method in METHODS {
        let departure = |h: f64| {
            let r = run(&model(
                method,
                h,
                0.1,
                "steady_state",
                json!(network()),
                json!([{"type": "branch_current", "component": "LINE"}]),
            ));
            let init = r.stats.initialization.unwrap();
            init.relative_voltage_deviation
                .max(init.relative_current_deviation.unwrap())
        };
        let (d1, d2) = (departure(4e-5), departure(2e-5));
        let ratio = d1 / d2;
        eprintln!("{method}: {d1:.3e}, {d2:.3e}, ratio {ratio:.3}");
        assert!((3.5..4.5).contains(&ratio), "{method}: ratio {ratio}");
    }
}

/// A branch without a zero-sequence path, written as a zero-sequence
/// impedance many orders of magnitude above the positive-sequence one,
/// between two sources. The start is as steady as with a ratio of 3: the
/// history term of the branch, G L (alpha i + c f), keeps its digits where
/// the entries of G times those of L are that ratio above their sum. (As
/// G L, a ratio of 1e6 moved the voltages at t = 0+ by 1e-4 and one of 1e8
/// by 1e-2: the first part of the first step has a stage coefficient of
/// 1e6 steps, which makes of a rounding error of the history current a
/// voltage that much larger.)
#[test]
fn a_large_zero_sequence_ratio_leaves_the_start_steady() {
    let (r1, l1) = (0.05, 0.15 * 43.56 / W);
    let source = |name: &str, bus: &str, angle: f64| {
        json!({"type": "three_phase_source", "name": name, "bus": bus, "neutral": "gnd",
               "line_voltage_rms_v": 66e3, "frequency_hz": F, "angle_deg": angle,
               "impedance": {"type": "sequence", "r1_ohm": 0.5, "l1_h": 0.0159,
                             "r0_ohm": 1.0, "l0_h": 0.0477}})
    };
    for method in METHODS {
        for h in [50e-6, 5e-6] {
            let deviation = |ratio: f64| {
                let r = run(&model(
                    method,
                    h,
                    0.02,
                    "steady_state",
                    json!([
                        source("G1", "b1", 10.0),
                        source("G2", "b2", 0.0),
                        {"type": "coupled_rl", "name": "X", "buses": ["b1", "b2"],
                         "impedance": {"type": "sequence", "r1_ohm": r1, "l1_h": l1,
                                       "r0_ohm": ratio * r1, "l0_h": ratio * l1}},
                    ]),
                    json!([{"type": "branch_current", "component": "X"}]),
                ));
                let init = r.stats.initialization.unwrap();
                (
                    init.relative_voltage_deviation,
                    init.relative_current_deviation.unwrap(),
                )
            };
            let (v3, i3) = deviation(3.0);
            for ratio in [1e3, 1e6, 1e8] {
                let (v, i) = deviation(ratio);
                eprintln!(
                    "{method} h {h:e} ratio {ratio:e}: v {v:.2e} ({v3:.2e}), i {i:.2e} ({i3:.2e})"
                );
                assert!(v < 1e-6, "{method} {h} {ratio}: {v}");
                assert!(
                    (i / i3 - 1.0).abs() < 0.05,
                    "{method} {h} {ratio}: {i} {i3}"
                );
            }
        }
    }
}

#[test]
fn a_fault_from_the_steady_state_reaches_the_symmetrical_component_current() {
    // Single line to ground fault at f at 0.1 s. With the load, the
    // Thevenin voltage is the pre-fault voltage and the sequence impedances
    // are (Zs + Zl) in parallel with the load (the capacitors included).
    let h = 2e-5;
    for method in METHODS {
        let mut comps = network();
        comps.push(
            json!({"type": "fault", "name": "F1", "bus": "f", "phases": ["a"], "ground": true,
                          "fault_resistance_ohm": 1.0, "start_time_s": 0.1}),
        );
        // Slowest mode: L/R of the zero-sequence loop, below 50 ms; 0.6 s
        // after the fault leaves e^-12.
        let r = run(&model(
            method,
            h,
            0.7,
            "steady_state",
            json!(comps),
            json!([{"type": "branch_current", "component": "F1"}]),
        ));
        let init = r.stats.initialization.as_ref().unwrap();
        assert!((init.checked_until_s - 0.1).abs() < 1e-12);
        let z = |seq: usize| parallel(zs(seq, W) + zl(seq, W), zload(W));
        let i0 = v_f() / (z(1) + z(1) + z(0) + 3.0);
        check(
            &format!("{method} I(F1.a)"),
            r.phasor("I(F1.a)", 1),
            3.0 * i0,
            (W * h).powi(2),
        );
    }
}

#[test]
fn magnetizing_current_with_core_loss_from_the_steady_state() {
    // A YNyn0 transformer at no load from an ideal source: per phase, half
    // the leakage in series with the magnetizing branch, conductance
    // P_fe/S and susceptance sqrt(i0^2 - g^2) per unit. Started from rest,
    // the magnetizing current would carry an offset that decays with
    // L_m/(R/2) = 32 s; the steady state starts without it.
    let (s_mva, v_kv, i0, loss_kw): (f64, f64, f64, f64) = (30.0, 66.0, 2.0, 150.0);
    let comps = json!([
        {"type": "three_phase_source", "name": "G1", "bus": "h", "line_voltage_rms_v": v_kv * 1e3,
         "frequency_hz": F, "angle_deg": 30.0},
        {"type": "three_phase_transformer", "name": "T1", "buses": ["h", "l"], "vector_group": "YNyn0",
         "rated_voltage_kv": [v_kv, 6.6], "rated_power_mva": s_mva, "rated_frequency_hz": F,
         "impedance_percent": 10.0, "resistance_percent": 1.0,
         "no_load_current_percent": i0, "no_load_loss_kw": loss_kw}
    ]);
    let h = 2e-5;
    let r = run(&model(
        "2s-dirk",
        h,
        0.06,
        "steady_state",
        comps,
        json!([{"type": "branch_current", "component": "T1"}]),
    ));
    let base = (v_kv * 1e3).powi(2) / (s_mva * 1e6);
    let g = loss_kw * 1e3 / (s_mva * 1e6);
    let b = ((i0 / 100.0).powi(2) - g * g).sqrt();
    let z_m = base / c(g, -b);
    let half = c(0.01, (0.1_f64.powi(2) - 0.01_f64.powi(2)).sqrt()) * (base / 2.0);
    let e = polar(v_kv * 1e3 / 3.0_f64.sqrt(), 30.0);
    let expected = e / (half + z_m);
    check(
        "phasor I(T1.0.a)",
        r.steady_phasor("I(T1.0.a)"),
        expected,
        1e-10,
    );
    check(
        "waveform I(T1.0.a)",
        r.phasor("I(T1.0.a)", 1),
        expected,
        (W * h).powi(2),
    );
    let init = r.stats.initialization.unwrap();
    assert!(init.relative_current_deviation.unwrap() < (W * h).powi(2));
}

// ------------------------------------------------------------ frequency scans

struct Scan {
    columns: Vec<String>,
    rows: Vec<(f64, Vec<Option<f64>>)>,
    singular: Vec<f64>,
    warnings: Vec<&'static str>,
}

impl Scan {
    /// Z of a column pair (magnitude, angle) at row r.
    fn z(&self, r: usize, name: &str) -> Option<Complex64> {
        let k = self
            .columns
            .iter()
            .position(|c| *c == format!("Zmag{name}"))
            .unwrap_or_else(|| panic!("no column Zmag{name} in {:?}", self.columns));
        let row = &self.rows[r].1;
        Some(polar(row[k]?, row[k + 1]?))
    }

    fn seq(&self, r: usize, prefix: &str, bus: &str) -> Complex64 {
        let k = self
            .columns
            .iter()
            .position(|c| *c == format!("{prefix}mag({bus})"))
            .unwrap();
        let row = &self.rows[r].1;
        polar(row[k].unwrap(), row[k + 1].unwrap())
    }
}

fn scan(components: Value, frequency_scan: Value) -> Scan {
    let m = json!({
        "format": "openkeito-model",
        "version": "0.3",
        "simulation": {"time_step_s": 1e-5, "end_time_s": 1e-3},
        "components": components,
        "frequency_scan": frequency_scan,
    });
    let loaded = openkeito_model::load_str(&m.to_string())
        .unwrap_or_else(|e| panic!("model is valid: {e:#?}"));
    let scanner = FrequencyScanner::from_model(&loaded.model).expect("scan builds");
    let columns = scanner.columns().iter().map(|c| c.name.clone()).collect();
    let mut rows = Vec::new();
    let stats = scanner
        .run(|f, v| {
            rows.push((f, v.to_vec()));
            Ok(())
        })
        .expect("scan completes");
    Scan {
        columns,
        rows,
        singular: stats.singular_frequencies_hz,
        warnings: stats.warnings.iter().map(|d| d.code).collect(),
    }
}

#[test]
fn scan_gives_the_bus_impedances_of_the_hand_calculation() {
    // Sources set to zero: the source impedance remains. Per sequence, the
    // impedance at f is (Zs + Zl) in parallel with the load; the transfer
    // impedance from f to s is the voltage at s per ampere into f.
    let freqs = [10.0, 50.0, 300.0, 1234.5];
    let s = scan(
        json!(network()),
        json!({"frequencies_hz": freqs, "nodes": ["f.a", "s.a"], "buses": ["f"]}),
    );
    assert_eq!(s.rows.len(), 4);
    for (r, &f) in freqs.iter().enumerate() {
        assert_eq!(s.rows[r].0, f);
        let w = 2.0 * PI * f;
        let z_f = |seq: usize| parallel(zs(seq, w) + zl(seq, w), zload(w));
        let z_sf = |seq: usize| zs(seq, w) * zload(w) / (zs(seq, w) + zl(seq, w) + zload(w));
        let tol = 1e-10;
        check(
            &format!("Z1(f) at {f} Hz"),
            s.seq(r, "Z1", "f"),
            z_f(1),
            tol,
        );
        check(
            &format!("Z2(f) at {f} Hz"),
            s.seq(r, "Z2", "f"),
            z_f(1),
            tol,
        );
        check(
            &format!("Z0(f) at {f} Hz"),
            s.seq(r, "Z0", "f"),
            z_f(0),
            tol,
        );
        // Phase values: self (Z0 + 2 Z1)/3.
        let self_f = (z_f(0) + 2.0 * z_f(1)) / 3.0;
        check(
            &format!("Z(f.a) at {f} Hz"),
            s.z(r, "(f.a)").unwrap(),
            self_f,
            tol,
        );
        let mutual = (z_sf(0) + 2.0 * z_sf(1)) / 3.0;
        check(
            &format!("Z(f.a;s.a) at {f} Hz"),
            s.z(r, "(f.a;s.a)").unwrap(),
            mutual,
            tol,
        );
    }
}

#[test]
fn scan_finds_series_and_parallel_resonances() {
    // Series R-L-C from node "sr" to gnd and parallel R, L, C from "pr" to
    // gnd, both resonant at f0 = 1/(2 pi sqrt(L C)): there |Z| is R, at an
    // angle of 0. Elsewhere Z follows the textbook expressions.
    let (r, l, cap): (f64, f64, f64) = (2.0, 10e-3, 5e-6);
    let f0 = 1.0 / (2.0 * PI * (l * cap).sqrt());
    let comps = json!([
        {"type": "resistor", "name": "RS", "nodes": ["sr", "s1"], "resistance_ohm": r},
        {"type": "inductor", "name": "LS", "nodes": ["s1", "s2"], "inductance_h": l},
        {"type": "capacitor", "name": "CS", "nodes": ["s2", "gnd"], "capacitance_f": cap},
        {"type": "resistor", "name": "RP", "nodes": ["pr", "gnd"], "resistance_ohm": 500.0},
        {"type": "inductor", "name": "LP", "nodes": ["pr", "gnd"], "inductance_h": l},
        {"type": "capacitor", "name": "CP", "nodes": ["pr", "gnd"], "capacitance_f": cap}
    ]);
    let s = scan(
        comps.clone(),
        json!({"frequencies_hz": [f0 / 3.0, f0, 2.5 * f0], "nodes": ["sr", "pr"]}),
    );
    for (row, (f, _)) in s.rows.iter().enumerate() {
        let w = 2.0 * PI * f;
        let series = c(r, w * l - 1.0 / (w * cap));
        let par = 1.0 / c(1.0 / 500.0, w * cap - 1.0 / (w * l));
        check(
            &format!("series at {f:.2} Hz"),
            s.z(row, "(sr)").unwrap(),
            series,
            1e-10,
        );
        check(
            &format!("parallel at {f:.2} Hz"),
            s.z(row, "(pr)").unwrap(),
            par,
            1e-10,
        );
        // The two branches are not connected.
        assert!(s.z(row, "(sr;pr)").unwrap().norm() < 1e-9);
    }
    let at_f0 = s.z(1, "(sr)").unwrap();
    assert!((at_f0.norm() - r).abs() < 1e-9 * r && at_f0.arg().abs() < 1e-9);
    assert!((s.z(1, "(pr)").unwrap().norm() - 500.0).abs() < 1e-9 * 500.0);

    // Log spacing: equal ratios, both ends included.
    let s = scan(
        comps,
        json!({"start_hz": 10.0, "stop_hz": 1e4, "points": 4, "spacing": "log", "nodes": ["sr"]}),
    );
    let f: Vec<f64> = s.rows.iter().map(|(f, _)| *f).collect();
    for (got, want) in f.iter().zip([10.0, 100.0, 1000.0, 1e4]) {
        assert!((got - want).abs() < 1e-9 * want, "{f:?}");
    }
}

#[test]
fn a_lossless_resonance_has_a_very_large_impedance() {
    // At f0 the admittances of L and C cancel within one matrix entry, to
    // rounding: the scan reports a very large impedance there.
    let (l, cap): (f64, f64) = (10e-3, 5e-6);
    let f0 = 1.0 / (2.0 * PI * (l * cap).sqrt());
    let s = scan(
        json!([
            {"type": "inductor", "name": "L1", "nodes": ["n", "gnd"], "inductance_h": l},
            {"type": "capacitor", "name": "C1", "nodes": ["n", "gnd"], "capacitance_f": cap}
        ]),
        json!({"frequencies_hz": [f0 / 2.0, f0, 2.0 * f0], "nodes": ["n"]}),
    );
    let off = s.z(0, "(n)").unwrap().norm();
    match s.z(1, "(n)") {
        Some(z) => assert!(z.norm() > 1e9 * off, "{z}"),
        None => assert_eq!(s.singular, [f0]),
    }
}

#[test]
fn a_singular_network_leaves_empty_rows_and_a_warning() {
    // Two voltage sources in parallel are two short circuits in a scan:
    // the current between them is not determined at any frequency. The
    // scan takes the switches in their state before t = 0, so S1 is closed
    // in it. In a run S1 opens at t = 0; without it, validation rejects
    // the loop of the two sources (E142).
    let comps = json!([
        {"type": "voltage_source", "name": "E1", "nodes": ["n", "gnd"], "waveform": {"type": "dc", "value": 1.0}},
        {"type": "voltage_source", "name": "E2", "nodes": ["n2", "gnd"], "waveform": {"type": "dc", "value": 1.0}},
        {"type": "switch", "name": "S1", "nodes": ["n", "n2"], "initially_closed": true, "open_time_s": 0.0},
        {"type": "resistor", "name": "R1", "nodes": ["n", "m"], "resistance_ohm": 1.0},
        {"type": "resistor", "name": "R2", "nodes": ["m", "gnd"], "resistance_ohm": 1.0}
    ]);
    let s = scan(
        comps,
        json!({"frequencies_hz": [50.0, 60.0], "nodes": ["m"]}),
    );
    assert_eq!(s.singular, [50.0, 60.0]);
    assert!(s.rows.iter().all(|(_, v)| v.iter().all(Option::is_none)));
    assert_eq!(s.warnings, ["W193"]);
}

#[test]
fn scan_agrees_with_an_emt_run_that_injects_one_frequency() {
    // A 250 Hz current into bus f, phase a, of the network with its sources
    // at zero (the three-phase source keeps only its impedance). The EMT run
    // starts from the steady state at 250 Hz; V/I over the last period must
    // match the scan within the discretization error (w h)^2.
    let f = 250.0;
    let h = 2e-6;
    let mut comps: Vec<Value> = network();
    comps[0]["line_voltage_rms_v"] = json!(0.0);
    comps[0]["frequency_hz"] = json!(f);
    comps.push(json!({"type": "current_source", "name": "J1", "nodes": ["gnd", "f.a"],
                      "waveform": {"type": "sine", "amplitude": 100.0, "frequency_hz": f, "phase_deg": 90.0}}));
    let r = run(&model(
        "2s-dirk",
        h,
        3.0 / f,
        "steady_state",
        json!(comps),
        json!([{"type": "node_voltage", "node": "f.a"}, {"type": "node_voltage", "node": "s.b"}]),
    ));
    let i = polar(100.0 / 2.0_f64.sqrt(), 0.0);
    let z_emt = r.phasor_at("V(f.a)", f, 1) / i;
    let z_emt_mutual = r.phasor_at("V(s.b)", f, 1) / i;
    let s = scan(
        json!(network()),
        json!({"frequencies_hz": [f], "nodes": ["f.a", "s.b"]}),
    );
    let w = 2.0 * PI * f;
    let tol = (w * h).powi(2);
    check(
        "Z(f.a) EMT against scan",
        z_emt,
        s.z(0, "(f.a)").unwrap(),
        tol,
    );
    check(
        "Z(f.a;s.b) EMT against scan",
        z_emt_mutual,
        s.z(0, "(f.a;s.b)").unwrap(),
        tol,
    );
}

/// A capacitor directly across a sine voltage source, and one behind a
/// resistor: the voltage of the first is the source's, so its current is
/// C dv/dt of a prescribed voltage.
fn capacitor_across_a_source(method: &str, h: f64) -> (f64, f64, f64) {
    let (cap, r) = (1e-5, 10.0);
    let run = run(&model(
        method,
        h,
        0.04,
        "steady_state",
        json!([
            {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"],
             "waveform": {"type": "sine", "amplitude": 100.0, "frequency_hz": F}},
            {"type": "capacitor", "name": "C1", "nodes": ["n1", "gnd"], "capacitance_f": cap},
            {"type": "resistor", "name": "R1", "nodes": ["n1", "n2"], "resistance_ohm": r},
            {"type": "capacitor", "name": "C2", "nodes": ["n2", "gnd"], "capacitance_f": cap}
        ]),
        json!([{"type": "branch_current", "component": "C1"}, {"type": "branch_current", "component": "C2"}]),
    ));
    let v = polar(100.0 / 2.0_f64.sqrt(), -90.0);
    let i1 = c(0.0, W * cap) * v;
    let i2 = v / (c(r, 0.0) + 1.0 / c(0.0, W * cap));
    let amp = |x: Complex64| 2.0_f64.sqrt() * x.norm();
    // The row at t = 0 included: it carries C dv/dt of the source at 0+.
    let at0 = (run.rows[0][run.col("I(C1)")] - 2.0_f64.sqrt() * i1.re).abs() / amp(i1);
    (
        run.departure("I(C1)", i1, 0.04) / amp(i1),
        run.departure("I(C2)", i2, 0.04) / amp(i2),
        at0,
    )
}

#[test]
fn a_capacitor_across_a_voltage_source() {
    // Trapezoidal rule: second order for both currents, and no alternating
    // error (which a wrong derivative at t = 0+ would leave undamped).
    let (a1, b1, z1) = capacitor_across_a_source("trapezoidal", 2e-5);
    let (a2, b2, _) = capacitor_across_a_source("trapezoidal", 1e-5);
    eprintln!("trapezoidal: C1 {a1:.3e} -> {a2:.3e}, C2 {b1:.3e} -> {b2:.3e}, t = 0: {z1:.1e}");
    assert!((3.5..4.5).contains(&(a1 / a2)) && (3.5..4.5).contains(&(b1 / b2)));
    assert!(a1 < (W * 2e-5).powi(2) && z1 < 1e-5);
    // 2S-DIRK: the current behind the resistor is of second order. The
    // current that the source dictates is of first order: its last stage
    // derivative is (Y2 - y0 - h (1 - g) f1)/(g h), which differs from
    // y'(t + h) by h y'' (1 - g + g^2 - 2 g)/(2 g) = h y''/(2 sqrt 2), a
    // relative error of w h / (2 sqrt 2). Other programs that use 2S-DIRK show
    // the same error.
    let (a1, b1, z1) = capacitor_across_a_source("2s-dirk", 2e-5);
    let (a2, b2, _) = capacitor_across_a_source("2s-dirk", 1e-5);
    eprintln!("2s-dirk: C1 {a1:.3e} -> {a2:.3e}, C2 {b1:.3e} -> {b2:.3e}, t = 0: {z1:.1e}");
    assert!((3.5..4.5).contains(&(b1 / b2)), "{b1} {b2}");
    assert!((1.9..2.1).contains(&(a1 / a2)), "{a1} {a2}");
    let predicted = W * 2e-5 / (2.0 * 2.0_f64.sqrt());
    assert!(
        (a1 / predicted - 1.0).abs() < 0.01,
        "{a1:e} against {predicted:e}"
    );
    assert!(z1 < 1e-5);
}

#[test]
fn excluded_shunt_inductor_and_nonlinear_inductor_start_from_zero_without_warning() {
    let h = 5e-5;
    let r = run(&json!({
        "format": "openkeito-model",
        "version": "0.7",
        "simulation": {
            "time_step_s": h,
            "end_time_s": 5e-3,
            "method": "trapezoidal",
            "initialization": "steady_state"
        },
        "components": [
            {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"],
             "waveform": {"type": "sine", "amplitude": 100.0, "frequency_hz": F}},
            {"type": "resistor", "name": "R1", "nodes": ["n1", "n2"], "resistance_ohm": 10.0},
            {"type": "resistor", "name": "R2", "nodes": ["n2", "gnd"], "resistance_ohm": 10.0},
            {"type": "inductor", "name": "L_ex", "nodes": ["n2", "gnd"], "inductance_h": 0.05, "initialization": "exclude"},
            {"type": "nonlinear_inductor", "name": "NL_ex", "nodes": ["n2", "gnd"],
             "current_a": [0.5, 1.0],
             "flux_linkage_wb": [0.02, 0.025],
             "initialization": "exclude"}
        ],
        "outputs": [
            {"type": "node_voltage", "node": "n2"},
            {"type": "branch_current", "component": "L_ex"},
            {"type": "branch_current", "component": "NL_ex"}
        ]
    }));

    assert!(
        !r.stats.warnings.iter().any(|w| w.code == "W260"),
        "excluded nonlinear inductor must not emit W260: {:?}",
        r.stats.warnings
    );
    let v_expected = polar(50.0 / 2.0_f64.sqrt(), -90.0);
    check(
        "V(n2) matches no-shunt steady state",
        r.steady_phasor("V(n2)"),
        v_expected,
        1e-12,
    );

    let init = r.stats.initialization.as_ref().unwrap();
    let i_l_phasor = init
        .phasors
        .iter()
        .find(|p| p.name == "I(L_ex)")
        .unwrap()
        .value;
    let i_nl_phasor = init
        .phasors
        .iter()
        .find(|p| p.name == "I(NL_ex)")
        .unwrap()
        .value;
    assert_eq!(i_l_phasor, Complex64::ZERO);
    assert_eq!(i_nl_phasor, Complex64::ZERO);

    let i_l_col = r.col("I(L_ex)");
    let i_nl_col = r.col("I(NL_ex)");
    assert!(r.rows[0][i_l_col].abs() < 1e-12, "{}", r.rows[0][i_l_col]);
    assert!(r.rows[0][i_nl_col].abs() < 1e-12, "{}", r.rows[0][i_nl_col]);
    let last = r.t.len() - 1;
    assert!(
        r.rows[last][i_l_col].abs() > 0.1,
        "L_ex builds transient current: {}",
        r.rows[last][i_l_col]
    );
    assert!(
        r.rows[last][i_nl_col].abs() > 0.1,
        "NL_ex builds transient current: {}",
        r.rows[last][i_nl_col]
    );
}

#[test]
fn excluded_switching_branch_and_isolated_island_initialize_to_zero() {
    let r = run(&json!({
        "format": "openkeito-model",
        "version": "0.7",
        "simulation": {
            "time_step_s": 5e-5,
            "end_time_s": 5e-3,
            "method": "trapezoidal",
            "initialization": "steady_state"
        },
        "components": [
            {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"],
             "waveform": {"type": "sine", "amplitude": 100.0, "frequency_hz": F}},
            {"type": "resistor", "name": "R1", "nodes": ["n1", "gnd"], "resistance_ohm": 10.0},
            {"type": "diode", "name": "D_ex", "nodes": ["n1", "iso"], "initialization": "exclude"},
            {"type": "resistor", "name": "R_iso", "nodes": ["iso", "gnd"], "resistance_ohm": 10.0, "initialization": "exclude"},
            {"type": "capacitor", "name": "C_iso", "nodes": ["iso", "gnd"], "capacitance_f": 10e-6, "initialization": "exclude"}
        ],
        "outputs": [
            {"type": "node_voltage", "node": "iso"},
            {"type": "branch_current", "component": "D_ex"}
        ]
    }));

    assert_eq!(r.steady_phasor("V(iso)"), Complex64::ZERO);
    let init = r.stats.initialization.as_ref().unwrap();
    let i_d_phasor = init
        .phasors
        .iter()
        .find(|p| p.name == "I(D_ex)")
        .unwrap()
        .value;
    assert_eq!(i_d_phasor, Complex64::ZERO);

    let v_iso_col = r.col("V(iso)");
    let i_d_col = r.col("I(D_ex)");
    assert!(
        r.rows[0][v_iso_col].abs() < 1e-7,
        "{}",
        r.rows[0][v_iso_col]
    );
    assert!(r.rows[0][i_d_col].abs() < 2e-3, "{}", r.rows[0][i_d_col]);
    let last = r.t.len() - 1;
    assert!(
        r.rows[last][v_iso_col] > 1.0,
        "isolated island charges in transient"
    );
    assert!(
        r.rows[last][i_d_col] > 0.1,
        "excluded diode conducts in transient"
    );
}

#[test]
fn excluded_60hz_source_before_included_50hz_source_does_not_override_init_frequency() {
    let r = run(&json!({
        "format": "openkeito-model",
        "version": "0.7",
        "simulation": {
            "time_step_s": 5e-5,
            "end_time_s": 2e-2,
            "method": "2s-dirk",
            "initialization": "steady_state"
        },
        "components": [
            {"type": "voltage_source", "name": "E_ex", "nodes": ["n_ex", "gnd"],
             "waveform": {"type": "sine", "amplitude": 50.0, "frequency_hz": 60.0},
             "initialization": "exclude"},
            {"type": "resistor", "name": "R_ex", "nodes": ["n_ex", "gnd"],
             "resistance_ohm": 10.0, "initialization": "exclude"},
            {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"],
             "waveform": {"type": "sine", "amplitude": 100.0, "frequency_hz": F}},
            {"type": "resistor", "name": "R1", "nodes": ["n1", "n2"], "resistance_ohm": 10.0},
            {"type": "capacitor", "name": "C1", "nodes": ["n2", "gnd"], "capacitance_f": 100e-6}
        ],
        "outputs": [
            {"type": "node_voltage", "node": "n2"},
            {"type": "branch_current", "component": "C1"}
        ]
    }));

    let init = r.stats.initialization.as_ref().unwrap();
    assert_eq!(init.frequency_hz, F);
    let v_n2 = r.steady_phasor("V(n2)");
    assert!(
        v_n2.norm() > 50.0,
        "V(n2) phasor was not solved at {F} Hz: {v_n2}"
    );
    let i_c = r.steady_phasor("I(C1)");
    assert!(
        i_c.norm() > 1.0,
        "I(C1) phasor was not solved at {F} Hz: {i_c}"
    );
    let dv = r.departure("V(n2)", v_n2, 2e-2);
    assert!(dv < 0.05, "V(n2) departed from 50 Hz steady state: {dv}");
    let di = r.departure("I(C1)", i_c, 2e-2);
    assert!(di < 0.005, "I(C1) departed from 50 Hz steady state: {di}");
}
