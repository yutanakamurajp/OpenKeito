//! Dependent sources (`vcvs`, `vccs`, `cccs`, `ccvs`) against closed-form
//! solutions.
//!
//! - Circuits of resistors are algebraic: the solution of every step is the
//!   one of the closed form, to rounding (relative 1e-12).
//! - The relation acts within the step: an inverting amplifier with a vcvs
//!   of gain 1e5 has the closed-form gain to 1e-9 at every time step,
//!   while the same amplifier built with a controlled_voltage_source that
//!   follows a gain block (one step late) diverges by the factor of the
//!   closed form per step.
//! - A vcvs charging an RC circuit converges with order 2 (trapezoidal
//!   rule and 2S-DIRK) and agrees with a voltage source of the same value.
//! - The phasor equations (steady state, frequency scan) use the same
//!   stamps.

mod common;

use std::f64::consts::PI;

use common::phasors::{c, check, polar};
use common::{run_model, try_run, Run};
use openkeito_core::{FrequencyScanner, SimError, Simulator};
use serde_json::{json, Value};

fn model(h: f64, end: f64, components: Value) -> Value {
    json!({
        "format": "openkeito-model",
        "version": "0.10",
        "simulation": {"time_step_s": h, "end_time_s": end},
        "components": components,
    })
}

fn dc(name: &str, nodes: [&str; 2], v: f64) -> Value {
    json!({"type": "voltage_source", "name": name, "nodes": nodes, "waveform": {"type": "dc", "value": v}})
}

fn r(name: &str, nodes: [&str; 2], ohm: f64) -> Value {
    json!({"type": "resistor", "name": name, "nodes": nodes, "resistance_ohm": ohm})
}

/// The largest relative difference of column `name` from `expected` over
/// the rows.
fn worst(run: &Run, name: &str, expected: f64) -> f64 {
    let k = run.col(name);
    run.rows
        .iter()
        .map(|row| (row[k] - expected).abs() / expected.abs().max(f64::MIN_POSITIVE))
        .fold(0.0, f64::max)
}

const REL: f64 = 1e-12;

#[test]
fn a_vcvs_drives_a_load_with_its_gain() {
    // Input divider V, R1, R2: v_c = V R2 / (R1 + R2) across R2. The vcvs
    // output behind R_o drives R_L: v_L = k v_c R_L / (R_o + R_L).
    let (v, r1, r2, ro, rl) = (3.0, 1e3, 2e3, 5.0, 50.0);
    for k in [7.5, -3.0, 1e3, 0.0] {
        let run = run_model(&model(
            1e-5,
            1e-4,
            json!([
                dc("V1", ["i", "gnd"], v),
                r("R1", ["i", "c"], r1),
                r("R2", ["c", "gnd"], r2),
                {"type": "vcvs", "name": "E1", "control": ["c", "gnd"], "output": ["e", "gnd"], "gain_v_per_v": k},
                r("RO", ["e", "l"], ro),
                r("RL", ["l", "gnd"], rl),
            ]),
        ));
        let vc = v * r2 / (r1 + r2);
        let vl = k * vc * rl / (ro + rl);
        if k == 0.0 {
            let col = run.col("V(l)");
            assert!(run.rows.iter().all(|row| row[col] == 0.0));
        } else {
            assert!(worst(&run, "V(l)", vl) < REL, "k = {k}");
            // The output current flows from output[0] through the source:
            // a source that delivers power has a negative current.
            assert!(worst(&run, "I(E1)", -vl / rl) < REL, "k = {k}");
        }
        // The control port draws no current: the divider is unloaded.
        assert!(worst(&run, "V(c)", vc) < REL);
    }
}

#[test]
fn a_cccs_mirrors_the_current_of_its_sense_port() {
    // V drives R_in in series with the sense port: i_in = V / R_in. The
    // output pushes beta i_in into R_L (from gnd through the source into
    // "o"): v_o = beta i_in R_L.
    let (v, rin, rl) = (2.0, 400.0, 30.0);
    for beta in [2.5, -0.75, 1e4] {
        let run = run_model(&model(
            1e-5,
            1e-4,
            json!([
                dc("V1", ["i", "gnd"], v),
                r("RIN", ["i", "s"], rin),
                {"type": "cccs", "name": "F1", "sense": ["s", "gnd"], "output": ["gnd", "o"], "gain_a_per_a": beta},
                r("RL", ["o", "gnd"], rl),
            ]),
        ));
        let iin = v / rin;
        assert!(worst(&run, "V(o)", beta * iin * rl) < REL, "beta = {beta}");
        assert!(worst(&run, "I(F1.sense)", iin) < REL);
        assert!(worst(&run, "I(F1)", beta * iin) < REL);
        // The sense port is 0 V.
        let s = run.col("V(s)");
        assert!(run.rows.iter().all(|row| row[s].abs() < 1e-15 * v));
    }
}

#[test]
fn a_vccs_and_a_ccvs_follow_their_gains() {
    let (v, r1, r2, rl) = (5.0, 1e3, 3e3, 20.0);
    let g = 0.02;
    let run = run_model(&model(
        1e-5,
        1e-4,
        json!([
            dc("V1", ["i", "gnd"], v),
            r("R1", ["i", "c"], r1),
            r("R2", ["c", "gnd"], r2),
            {"type": "vccs", "name": "G1", "control": ["c", "gnd"], "output": ["gnd", "o"], "transconductance_s": g},
            r("RL", ["o", "gnd"], rl),
        ]),
    ));
    let vc = v * r2 / (r1 + r2);
    assert!(worst(&run, "V(o)", g * vc * rl) < REL);
    assert!(worst(&run, "I(G1)", g * vc) < REL);

    // The ccvs measures i_s = V / (R1 + R2) through its sense port between
    // the two resistors and gives r i_s behind R_o into R_L.
    let (rt, ro) = (75.0, 2.0);
    let run = run_model(&model(
        1e-5,
        1e-4,
        json!([
            dc("V1", ["i", "gnd"], v),
            r("R1", ["i", "s1"], r1),
            {"type": "ccvs", "name": "H1", "sense": ["s1", "s2"], "output": ["e", "gnd"], "transresistance_ohm": rt},
            r("R2", ["s2", "gnd"], r2),
            r("RO", ["e", "l"], ro),
            r("RL", ["l", "gnd"], rl),
        ]),
    ));
    let is = v / (r1 + r2);
    let vl = rt * is * rl / (ro + rl);
    assert!(worst(&run, "V(l)", vl) < REL);
    assert!(worst(&run, "I(H1.sense)", is) < REL);
    assert!(worst(&run, "I(H1)", -vl / rl) < REL);
}

/// An inverting amplifier: R_in from the input to the inverting node m,
/// R_f from m to the output, an operational amplifier of gain A from the
/// non-inverting input (gnd) and m to the output, and a load.
fn inverting_amplifier(h: f64, end: f64, a: f64, delayed: bool) -> Value {
    let amplifier = if delayed {
        json!({"type": "controlled_voltage_source", "name": "A1", "nodes": ["out", "gnd"], "signal": "drive"})
    } else {
        json!({"type": "vcvs", "name": "A1", "control": ["gnd", "m"], "output": ["out", "gnd"], "gain_v_per_v": a})
    };
    let mut m = model(
        h,
        end,
        json!([
            {"type": "voltage_source", "name": "VIN", "nodes": ["in", "gnd"],
                "waveform": {"type": "sine", "amplitude": 0.1, "frequency_hz": 1e3, "phase_deg": 30.0, "offset": 0.0}},
            r("RIN", ["in", "m"], 1e3),
            r("RF", ["m", "out"], 1e4),
            amplifier,
            r("RL", ["out", "gnd"], 2e3),
        ]),
    );
    if delayed {
        m["controls"] = json!([
            {"type": "measurement", "name": "vm", "quantity": {"type": "node_voltage", "node": "m"}},
            {"type": "gain", "name": "drive", "input": "vm", "gain": -a},
        ]);
    }
    m
}

#[test]
fn an_inverting_amplifier_has_the_gain_of_the_closed_form_at_every_step() {
    // v_out / v_in = -(R_f / R_in) / (1 + (1 + R_f / R_in) / A).
    for a in [1e5, 1e6] {
        amplifier_at_every_step(a);
    }
}

fn amplifier_at_every_step(a: f64) {
    let ratio = 10.0;
    let gain = -ratio / (1.0 + (1.0 + ratio) / a);
    let mut first: Option<Vec<f64>> = None;
    for h in [1e-5, 2.5e-6, 1e-6] {
        let run = run_model(&inverting_amplifier(h, 2e-3, a, false));
        let (i, o) = (run.col("V(in)"), run.col("V(out)"));
        let mut largest = 0.0_f64;
        for row in &run.rows {
            if row[i].abs() > 1e-3 {
                largest = largest.max((row[o] / row[i] / gain - 1.0).abs());
            }
        }
        assert!(largest < 1e-9, "h = {h}: {largest:e}");
        eprintln!(
            "inverting amplifier, A = {a:e}, h = {h:e}: largest relative error of the gain \
             {largest:.1e}, condition estimate {:.1e}",
            run.stats.max_condition_estimate
        );
        // The rows at the same instants agree for every time step: the
        // circuit is algebraic. (The row at t = 0 is the solution at
        // t = 0+, a small fraction of the time step later.)
        let stride = (1e-5 / h).round() as usize;
        let on_coarse_grid: Vec<f64> = run
            .rows
            .iter()
            .step_by(stride)
            .skip(1)
            .map(|row| row[o])
            .collect();
        match &first {
            None => first = Some(on_coarse_grid),
            Some(f) => {
                assert_eq!(f.len(), on_coarse_grid.len());
                for (x, y) in f.iter().zip(&on_coarse_grid) {
                    assert!((x - y).abs() <= 1e-12 * x.abs().max(1e-3), "{x} {y}");
                }
            }
        }
    }
}

#[test]
fn the_same_amplifier_one_step_late_grows_by_the_loop_gain_per_step() {
    // With the amplifier as a controlled voltage source that follows
    // -A V(m) one step late, v_out(n + 1) = -A V(m)(n) and
    // V(m) = (v_in R_f + v_out R_in) / (R_in + R_f): the error grows by
    // -A R_in / (R_in + R_f) = -A / 11 per step. Recorded for
    // docs/model.md, not a property of the dependent sources.
    let a = 1e5;
    let factor = -a / 11.0;
    let m = inverting_amplifier(1e-5, 2e-3, a, true);
    match try_run(&m) {
        Ok(run) => panic!("the run completes: {:?}", run.rows.last()),
        Err(SimError::SignalNotFinite { time_s, .. }) => {
            eprintln!("one step late, A = 1e5: the signal is not finite at t = {time_s:e} s");
            assert!(time_s < 1e-3);
        }
        Err(e) => panic!("{:?}", e.to_diagnostic()),
    }
    let short = {
        let mut m = m.clone();
        m["simulation"]["end_time_s"] = json!(1e-4);
        run_model(&m)
    };
    let o = short.col("V(out)");
    let steps: Vec<f64> = short
        .rows
        .windows(2)
        .skip(3)
        .map(|w| w[1][o] / w[0][o])
        .collect();
    for s in &steps {
        assert!((s / factor - 1.0).abs() < 1e-6, "{s} against {factor}");
    }
    eprintln!(
        "one step late, A = 1e5: v_out grows by {:.6e} per step (closed form {factor:.6e})",
        steps[0]
    );
}

/// A vcvs of gain k copies V to the RC circuit R, C: v_C = k V (1 - e^(-t/RC)).
fn rc(h: f64, method: &str, vcvs: bool) -> Value {
    let (v, k) = (1.5, 2.0);
    let source = if vcvs {
        json!({"type": "vcvs", "name": "E1", "control": ["i", "gnd"], "output": ["e", "gnd"], "gain_v_per_v": k})
    } else {
        dc("E1", ["e", "gnd"], k * v)
    };
    let mut m = model(
        h,
        5e-3,
        json!([
            dc("V1", ["i", "gnd"], v),
            r("RI", ["i", "gnd"], 1e3),
            source,
            r("R1", ["e", "c"], 10.0),
            {"type": "capacitor", "name": "C1", "nodes": ["c", "gnd"], "capacitance_f": 1e-4},
        ]),
    );
    m["simulation"]["method"] = json!(method);
    m
}

#[test]
fn a_vcvs_charges_an_rc_circuit_with_second_order_and_as_a_voltage_source() {
    let (v, k, tau) = (1.5, 2.0, 1e-3);
    for method in ["trapezoidal", "2s-dirk"] {
        let error = |h: f64| {
            let run = run_model(&rc(h, method, true));
            let col = run.col("V(c)");
            run.t
                .iter()
                .zip(&run.rows)
                .map(|(t, row)| (row[col] - k * v * (1.0 - (-t / tau).exp())).abs())
                .fold(0.0, f64::max)
        };
        let (coarse, fine) = (error(1e-4), error(5e-5));
        let order = (coarse / fine).log2();
        eprintln!("{method}: error {coarse:.3e} at 1e-4 s, {fine:.3e} at 5e-5 s, order {order:.3}");
        assert!((order - 2.0).abs() < 0.1, "{method}: order {order}");

        // Against a voltage source of the value k V.
        let with_vcvs = run_model(&rc(5e-5, method, true));
        let with_source = run_model(&rc(5e-5, method, false));
        let (a, b) = (with_vcvs.col("V(c)"), with_source.col("V(c)"));
        let largest = with_vcvs
            .rows
            .iter()
            .zip(&with_source.rows)
            .map(|(x, y)| (x[a] - y[b]).abs() / (k * v))
            .fold(0.0, f64::max);
        eprintln!("{method}: vcvs against voltage source {largest:.1e} of k V");
        assert!(largest <= 1e-15, "{method}: {largest:e}");
    }
}

#[test]
fn the_steady_state_and_the_scan_use_the_same_stamps() {
    // A sine source drives R1 into node c with C to gnd; a vcvs of gain k
    // copies V(c) to a load. The steady state: V(c) = V / (1 + j w R1 C),
    // V(l) = k V(c) R_L / (R_o + R_L).
    let (amplitude, f, r1, cap, k, ro, rl) = (10.0, 50.0, 100.0, 20e-6, -4.0, 1.0, 9.0);
    let w = 2.0 * PI * f;
    let mut m = model(
        1e-5,
        0.04,
        json!([
            {"type": "voltage_source", "name": "V1", "nodes": ["i", "gnd"],
                "waveform": {"type": "sine", "amplitude": amplitude, "frequency_hz": f, "phase_deg": 90.0, "offset": 0.0}},
            r("R1", ["i", "c"], r1),
            {"type": "capacitor", "name": "C1", "nodes": ["c", "gnd"], "capacitance_f": cap},
            {"type": "vcvs", "name": "E1", "control": ["c", "gnd"], "output": ["e", "gnd"], "gain_v_per_v": k},
            r("RO", ["e", "l"], ro),
            r("RL", ["l", "gnd"], rl),
        ]),
    );
    m["simulation"]["initialization"] = json!("steady_state");
    let run = run_model(&m);
    let vin = polar(amplitude / 2f64.sqrt(), 0.0);
    let vc = vin / (c(1.0, w * r1 * cap));
    let vl = vc * (k * rl / (ro + rl));
    check("V(c)", run.steady("V(c)"), vc, 1e-12);
    check("V(l)", run.steady("V(l)"), vl, 1e-12);
    // The run stays in the steady state from t = 0.
    let col = run.col("V(l)");
    let at = |t: f64| 2f64.sqrt() * (vl * c((w * t).cos(), (w * t).sin())).re;
    let largest = run
        .t
        .iter()
        .zip(&run.rows)
        .map(|(t, row)| (row[col] - at(*t)).abs())
        .fold(0.0, f64::max);
    assert!(largest < 1e-6 * vl.norm(), "{largest:e}");

    // A vccs from node n to gnd controlled by V(n) is a conductance g: the
    // impedance at n is 1 / (g + 1 / R + j w C).
    let (g, rn, cn) = (0.05, 40.0, 1e-5);
    let mut m = model(
        1e-5,
        1e-3,
        json!([
            r("RN", ["n", "gnd"], rn),
            {"type": "capacitor", "name": "CN", "nodes": ["n", "gnd"], "capacitance_f": cn},
            {"type": "vccs", "name": "G1", "control": ["n", "gnd"], "output": ["n", "gnd"], "transconductance_s": g},
        ]),
    );
    let freqs = [10.0, 400.0, 2e3];
    m["frequency_scan"] = json!({"frequencies_hz": freqs, "nodes": ["n"]});
    let loaded = openkeito_model::load_str(&m.to_string()).unwrap();
    let scanner = FrequencyScanner::from_model(&loaded.model).unwrap();
    let columns: Vec<String> = scanner.columns().iter().map(|c| c.name.clone()).collect();
    let k = columns.iter().position(|c| c == "Zmag(n)").unwrap();
    let mut rows = Vec::new();
    scanner
        .run(|f, v| {
            rows.push((f, v.to_vec()));
            Ok(())
        })
        .unwrap();
    for (f, row) in rows {
        let wf = 2.0 * PI * f;
        let z = c(1.0, 0.0) / c(g + 1.0 / rn, wf * cn);
        check(
            &format!("Z(n) at {f} Hz"),
            polar(row[k].unwrap(), row[k + 1].unwrap()),
            z,
            1e-12,
        );
    }
}

#[test]
fn a_dependent_source_between_the_ends_of_a_line_keeps_them_in_one_part() {
    // Two parts joined by a Bergeron line, and a vcvs that copies the
    // voltage of one end to a load at the other: one part, and the same
    // rows with and without the division.
    let per_km = json!({"type": "sequence", "r1_ohm_per_km": 0.03, "l1_h_per_km": 0.9e-3,
        "c1_f_per_km": 12.7e-9, "r0_ohm_per_km": 0.2, "l0_h_per_km": 3e-3, "c0_f_per_km": 7e-9});
    let build = |partitioning: &str, with_vcvs: bool| {
        let mut components = vec![
            json!({"type": "three_phase_source", "name": "G", "bus": "a", "line_voltage_rms_v": 66e3, "frequency_hz": 50.0,
                "impedance": {"type": "sequence", "r1_ohm": 0.5, "l1_h": 0.01, "r0_ohm": 0.5, "l0_h": 0.01}}),
            json!({"type": "bergeron_line", "name": "L", "buses": ["a", "b"], "length_km": 30.0, "per_km": per_km}),
            json!({"type": "coupled_rl", "name": "LD", "buses": ["b", "gnd"],
                "impedance": {"type": "sequence", "r1_ohm": 400.0, "l1_h": 0.1, "r0_ohm": 400.0, "l0_h": 0.1}}),
        ];
        if with_vcvs {
            components.push(json!({"type": "vcvs", "name": "E1", "control": ["a.a", "gnd"], "output": ["x", "gnd"], "gain_v_per_v": 1e-3}));
            components.push(json!({"type": "resistor", "name": "RX", "nodes": ["x", "b.a"], "resistance_ohm": 1e3}));
        }
        let mut m = model(2e-6, 2e-3, json!(components));
        m["simulation"]["partitioning"] = json!(partitioning);
        let loaded = openkeito_model::load_str(&m.to_string()).unwrap();
        Simulator::from_model(&loaded.model).unwrap()
    };
    // Without the vcvs the line divides the circuit.
    assert_eq!(build("every_line", false).parts().len(), 2);
    let divided = build("every_line", true);
    assert_eq!(divided.parts().len(), 1);
    let run = |sim: Simulator| common::run_sim(sim, 2e-6);
    let (a, b) = (run(divided), run(build("off", true)));
    assert_eq!(a.stats.parts, 1);
    let bits = |r: &Run| -> Vec<Vec<u64>> {
        r.rows
            .iter()
            .map(|row| row.iter().map(|x| x.to_bits()).collect())
            .collect()
    };
    assert_eq!(bits(&a), bits(&b));
}

#[test]
fn a_singular_loop_of_gains_names_the_dependent_sources() {
    // A vcvs of gain 1 that measures its own output is caught by the
    // validation (E144).
    let m = model(
        1e-5,
        1e-4,
        json!([
            dc("V1", ["i", "gnd"], 1.0),
            r("R1", ["i", "o"], 10.0),
            {"type": "vcvs", "name": "E1", "control": ["o", "gnd"], "output": ["o", "gnd"], "gain_v_per_v": 1.0},
        ]),
    );
    let d = common::diagnostics(&m);
    assert_eq!(d.iter().map(|d| d.code).collect::<Vec<_>>(), ["E144"]);
    assert!(d[0].hint.contains("Change a gain"), "{}", d[0].hint);

    // The validation checks the gains in the state at t = 0. A switch that
    // closes later and makes the output of a vcvs of gain 1 its own control
    // stops the run there (E200), and the hint names the source.
    let m = model(
        1e-5,
        1e-4,
        json!([
            dc("V1", ["i", "gnd"], 1.0),
            r("R1", ["i", "o"], 10.0),
            {"type": "vcvs", "name": "E1", "control": ["o", "gnd"], "output": ["e", "gnd"], "gain_v_per_v": 1.0},
            r("RE", ["e", "gnd"], 10.0),
            {"type": "switch", "name": "S1", "nodes": ["e", "o"], "close_time_s": 5e-5},
        ]),
    );
    assert!(common::diagnostics(&m).is_empty());
    match try_run(&m) {
        Err(e @ SimError::Singular { .. }) => {
            let d = e.to_diagnostic();
            assert_eq!(d.code, "E200");
            assert!(d.message.contains("t = 5e-5 s"), "{}", d.message);
            assert!(d.hint.contains("dependent sources \"E1\""), "{}", d.hint);
        }
        Err(e) => panic!("{:?}", e.to_diagnostic()),
        Ok(_) => panic!("the run completes"),
    }
}

#[test]
fn a_loop_of_a_vcvs_output_and_a_sense_port_closed_by_the_output_of_the_cccs() {
    // V drives R1 into node c, R2 from c to gnd. The vcvs E1 (gain k)
    // measures c and its output is across the sense port of the cccs F1,
    // whose output (gain beta) feeds c. The sense port holds V(s) = 0, so
    // V(c) = 0 and R2 carries nothing: beta i_s = -V / R1 at c. Validation
    // warns (W144) and the run completes with the closed form.
    let (v, r1, r2) = (3.0, 40.0, 25.0);
    for (k, beta) in [(2.0, 2.0), (-0.5, -4.0), (1e3, 1.0)] {
        let m = model(
            1e-5,
            1e-4,
            json!([
                dc("V1", ["i", "gnd"], v),
                r("R1", ["i", "c"], r1),
                r("R2", ["c", "gnd"], r2),
                {"type": "vcvs", "name": "E1", "control": ["c", "gnd"], "output": ["s", "gnd"], "gain_v_per_v": k},
                {"type": "cccs", "name": "F1", "sense": ["s", "gnd"], "output": ["gnd", "c"], "gain_a_per_a": beta},
            ]),
        );
        let codes: Vec<&str> = common::diagnostics(&m).iter().map(|d| d.code).collect();
        assert_eq!(codes, ["W144"], "k = {k}, beta = {beta}");
        let run = run_model(&m);
        let is = -v / (r1 * beta);
        assert!(
            worst(&run, "I(F1.sense)", is) < REL,
            "k = {k}, beta = {beta}"
        );
        assert!(worst(&run, "I(F1)", beta * is) < REL);
        assert!(worst(&run, "I(R1)", v / r1) < REL);
        for zero in ["V(c)", "V(s)", "I(R2)"] {
            let col = run.col(zero);
            assert!(
                run.rows.iter().all(|row| row[col].abs() < 1e-12 * v / r1),
                "{zero}"
            );
        }
    }
}

#[test]
fn a_loop_in_a_group_too_large_for_the_check_of_the_gains_stops_the_run() {
    // The output of a vcvs across the sense port of a cccs that feeds only
    // RL: singular whatever the gains. With a ladder of 600 resistors on
    // node c the group has more than 600 unknowns, which validation does
    // not solve: it warns (W144, whose hint says so), and the run stops
    // (E200) with a hint that names the two sources.
    let mut components = vec![
        dc("V1", ["i", "gnd"], 1.0),
        r("R1", ["i", "c"], 10.0),
        r("R2", ["c", "gnd"], 10.0),
        json!({"type": "vcvs", "name": "E1", "control": ["c", "gnd"], "output": ["s", "gnd"], "gain_v_per_v": 2.0}),
        json!({"type": "cccs", "name": "F1", "sense": ["s", "gnd"], "output": ["gnd", "o"], "gain_a_per_a": 2.0}),
        r("RL", ["o", "gnd"], 10.0),
    ];
    for j in 0..600 {
        let from = if j == 0 {
            "c".to_string()
        } else {
            format!("l{j}")
        };
        let to = format!("l{}", j + 1);
        components.push(r(&format!("RS{j}"), [&from, &to], 10.0));
        components.push(r(&format!("RP{j}"), [&to, "gnd"], 10.0));
    }
    let m = model(1e-5, 1e-4, Value::Array(components));
    let d = common::diagnostics(&m);
    assert_eq!(d.iter().map(|d| d.code).collect::<Vec<_>>(), ["W144"]);
    assert!(
        d[0].hint.contains("more than 600 unknowns"),
        "{}",
        d[0].hint
    );
    match try_run(&m) {
        Err(e @ SimError::Singular { .. }) => {
            let d = e.to_diagnostic();
            assert_eq!(d.code, "E200");
            assert!(d.hint.contains("\"E1\""), "{}", d.hint);
            assert!(d.hint.contains("\"F1\""), "{}", d.hint);
        }
        Err(e) => panic!("{:?}", e.to_diagnostic()),
        Ok(_) => panic!("the run completes"),
    }
}

#[test]
fn a_precision_rectifier_switches_at_the_zeros_of_its_input() {
    // Inverting half-wave precision rectifier: an operational amplifier of
    // gain A (vcvs from gnd and m to p), R_in from the input to m, D1 from m
    // to p, D2 from p to the output and R_f from the output to m. For
    // v_in > 0, D1 conducts and the output is 0; for v_in < 0, D2 conducts
    // and v_out = -(R_f / R_in) v_in. Both diodes change state where v_in
    // crosses 0, to within what the finite gain and the resistances of the
    // diodes leave. With "switching": "interpolated" the instants lie
    // within the time steps.
    let (f, phase_deg, h) = (50.0, 17.0_f64, 1e-5);
    let w = 2.0 * PI * f;
    for a in [1e5, 1e6] {
        let mut m = model(
            h,
            0.04,
            json!([
                {"type": "voltage_source", "name": "VIN", "nodes": ["in", "gnd"],
                    "waveform": {"type": "sine", "amplitude": 1.0, "frequency_hz": f, "phase_deg": phase_deg, "offset": 0.0}},
                r("RIN", ["in", "m"], 1e3),
                {"type": "vcvs", "name": "A1", "control": ["gnd", "m"], "output": ["p", "gnd"], "gain_v_per_v": a},
                {"type": "diode", "name": "D1", "nodes": ["m", "p"]},
                {"type": "diode", "name": "D2", "nodes": ["p", "out"]},
                r("RF", ["out", "m"], 2e3),
                r("RL", ["out", "gnd"], 1e4),
            ]),
        );
        m["simulation"]["switching"] = json!("interpolated");
        let run = run_model(&m);
        let zero = |t: f64| {
            let k = ((w * t + phase_deg.to_radians()) / PI).round();
            (k * PI - phase_deg.to_radians()) / w
        };
        let events: Vec<_> = run.stats.events.iter().filter(|e| e.time_s > 0.0).collect();
        assert!(events.len() >= 6, "{events:?}");
        let largest = events
            .iter()
            .map(|e| (e.time_s - zero(e.time_s)).abs())
            .fold(0.0, f64::max);
        // The circuit has no states: at every grid point the output is the
        // solution of the resistive circuit with the diodes in the states
        // that the sign of v_in gives.
        let (i, o) = (run.col("V(in)"), run.col("V(out)"));
        let output = run
            .rows
            .iter()
            .skip(1)
            .map(|row| (row[o] - rectifier_output(row[i], a)).abs())
            .fold(0.0, f64::max);
        eprintln!(
            "precision rectifier, A = {a:e}: {} switching instants, largest distance from the \
             zero of the input {largest:.2e} s, largest output error {output:.2e} V, condition \
             estimate {:.1e}",
            events.len(),
            run.stats.max_condition_estimate
        );
        assert!(largest < 1e-12, "A = {a:e}: {largest:e} s");
        assert!(output < 1e-12, "A = {a:e}: {output:e} V");
    }
}

/// The output of the precision rectifier for the input `v_in`: with
/// p = -A V(m) and the conductances g1 of D1 and g2 of D2 (on 1e-3 ohm,
/// off 1e6 ohm), the node equations of m and of the output.
fn rectifier_output(v_in: f64, a: f64) -> f64 {
    let (rin, rf, rl) = (1e3, 2e3, 1e4);
    let (on, off) = (1.0 / 1e-3, 1.0 / 1e6);
    let (g1, g2) = if v_in > 0.0 { (on, off) } else { (off, on) };
    // [a11 a12; a21 a22] [V(m); V(out)] = [v_in / R_in; 0].
    let a11 = 1.0 / rin + 1.0 / rf + g1 * (1.0 + a);
    let a12 = -1.0 / rf;
    let a21 = -1.0 / rf + a * g2;
    let a22 = 1.0 / rf + 1.0 / rl + g2;
    let b1 = v_in / rin;
    -a21 * b1 / (a11 * a22 - a12 * a21)
}

#[test]
fn ports_away_from_gnd_follow_the_closed_forms() {
    // Every node of both ports away from gnd. The input: V across R1, R2,
    // R3 in series, the controlling voltage across R2 (or the controlling
    // current through the sense port between R1 and R2). The output from o1
    // to o2, with R_a from o1 and R_b from o2 to gnd.
    let (v, r1, r2, r3, ra, rb) = (6.0, 100.0, 200.0, 300.0, 7.0, 11.0);
    let i_in = v / (r1 + r2 + r3);
    let vc = i_in * r2;
    let input = |sense: bool| {
        let mut c = vec![dc("V1", ["i", "gnd"], v), r("R1", ["i", "c1"], r1)];
        if sense {
            c.push(r("R2", ["c2", "c3"], r2));
        } else {
            c.push(r("R2", ["c1", "c2"], r2));
        }
        c.push(r("R3", [if sense { "c3" } else { "c2" }, "gnd"], r3));
        c.push(r("RA", ["o1", "gnd"], ra));
        c.push(r("RB", ["o2", "gnd"], rb));
        c
    };
    let run = |sense: bool, source: Value| {
        let mut c = input(sense);
        c.push(source);
        run_model(&model(1e-5, 1e-4, json!(c)))
    };
    // A current i from o1 through the source to o2: V(o1) = -i R_a,
    // V(o2) = i R_b.
    let current = |run: &Run, i: f64| {
        assert!(worst(run, "V(o1)", -i * ra) < REL);
        assert!(worst(run, "V(o2)", i * rb) < REL);
    };
    let g = 0.03;
    current(
        &run(
            false,
            json!({"type": "vccs", "name": "G1", "control": ["c1", "c2"], "output": ["o1", "o2"], "transconductance_s": g}),
        ),
        g * vc,
    );
    let beta = 4.0;
    let f = run(
        true,
        json!({"type": "cccs", "name": "F1", "sense": ["c1", "c2"], "output": ["o1", "o2"], "gain_a_per_a": beta}),
    );
    current(&f, beta * i_in);
    assert!(worst(&f, "I(F1.sense)", i_in) < REL);
    // A voltage u = V(o1) - V(o2) across R_a + R_b in series through gnd:
    // the current u / (R_a + R_b) flows out of o1 into R_a.
    let voltage = |run: &Run, u: f64| {
        let i = u / (ra + rb);
        assert!(worst(run, "V(o1)", i * ra) < REL);
        assert!(worst(run, "V(o2)", -i * rb) < REL);
    };
    let k = -2.5;
    voltage(
        &run(
            false,
            json!({"type": "vcvs", "name": "E1", "control": ["c1", "c2"], "output": ["o1", "o2"], "gain_v_per_v": k}),
        ),
        k * vc,
    );
    let rt = 90.0;
    voltage(
        &run(
            true,
            json!({"type": "ccvs", "name": "H1", "sense": ["c1", "c2"], "output": ["o1", "o2"], "transresistance_ohm": rt}),
        ),
        rt * i_in,
    );
}
