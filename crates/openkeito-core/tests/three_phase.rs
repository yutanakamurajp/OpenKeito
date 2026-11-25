//! Three-phase components against hand calculations: faults by symmetrical
//! components, transformers by their equivalent circuits, pi lines by the
//! ABCD parameters of lumped sections and of the distributed line.
//!
//! Phasors are rms with a cosine reference: x(t) = sqrt(2) |X| cos(wt + arg X).
//! A steady-state phasor is taken from the simulated waveform by a discrete
//! Fourier transform over the last whole periods of the run, after the
//! transients have decayed to below 1e-7 of their initial size.
//!
//! Tolerances: both methods are of second order, so the forced response at
//! angular frequency w has a relative error of about k (w h)^2. For the
//! trapezoidal rule k = 1/12 exactly: it evaluates every impedance at the
//! warped frequency (2/h) tan(w h/2); the fault currents below show
//! 3.2e-6 = (w h)^2/12 at h = 20 us. 2S-DIRK has k = 0.01 to 0.03 here
//! (4e-7 to 1.2e-6). The comparisons use (w h)^2, 12 times the error of the
//! trapezoidal rule; a modelling error such as a missing magnetizing branch
//! (1e-3) or a wrong phase shift fails them by orders of magnitude. The
//! convergence test checks the order itself.

mod common;

use common::phasors::*;
use common::run_model as run;
use serde_json::{json, Value};

/// The phasors of the hand calculations.
type C = Complex64;

fn model(method: &str, h: f64, end: f64, components: Value, outputs: Value) -> Value {
    model_of(method, h, end, None, components, outputs)
}

// ---------------------------------------------------------------- faults

/// 66 kV source with Z1 = 0.5 + j5 and Z0 = 1 + j15 ohm, a coupled line with
/// Z1 = 1 + j4 and Z0 = 3 + j12 ohm, and a fault at its end from t = 0.
const V_LL: f64 = 66_000.0;
const ANGLE: f64 = 20.0;

fn source_and_line() -> Vec<Value> {
    vec![
        json!({"type": "three_phase_source", "name": "G1", "bus": "s", "line_voltage_rms_v": V_LL,
               "frequency_hz": F, "angle_deg": ANGLE,
               "impedance": {"type": "sequence", "r1_ohm": 0.5, "l1_h": 5.0 / W, "r0_ohm": 1.0, "l0_h": 15.0 / W}}),
        json!({"type": "coupled_rl", "name": "LINE", "buses": ["s", "f"],
               "impedance": {"type": "sequence", "r1_ohm": 1.0, "l1_h": 4.0 / W, "r0_ohm": 3.0, "l0_h": 12.0 / W}}),
    ]
}

fn z1() -> C {
    c(1.5, 9.0)
}

fn z0() -> C {
    c(4.0, 27.0)
}

/// Phase-a voltage of the source: the pre-fault voltage of the unloaded bus.
fn e_a() -> C {
    polar(V_LL / 3.0_f64.sqrt(), ANGLE)
}

/// Fault currents [I_a, I_b, I_c] and the ground current from the sequence
/// networks (Z2 = Z1 for this static network) with fault resistance zf in
/// each phase and zg from the fault point to ground (Anderson, Analysis of
/// Faulted Power Systems, ch. 3; Grainger and Stevenson, ch. 12).
fn sequence_currents(kind: &str, zf: f64, zg: f64) -> ([C; 3], C) {
    let (z1, z2, z0, e) = (z1(), z1(), z0(), e_a());
    let zf = c(zf, 0.0);
    let zg = c(zg, 0.0);
    let zero = c(0.0, 0.0);
    let (i0, i1, i2) = match kind {
        "three_phase" => (zero, e / (z1 + zf), zero),
        "single_line_ground" => {
            let i = e / (z1 + z2 + z0 + 3.0 * (zf + zg));
            (i, i, i)
        }
        "line_line" => {
            let i = e / (z1 + z2 + 2.0 * zf);
            (zero, i, -i)
        }
        "double_line_ground" => {
            let zz0 = z0 + zf + 3.0 * zg;
            let zz2 = z2 + zf;
            let i1 = e / (z1 + zf + parallel(zz2, zz0));
            (-i1 * zz2 / (zz2 + zz0), i1, -i1 * zz0 / (zz2 + zz0))
        }
        other => panic!("{other}"),
    };
    let a = a_op();
    let a2 = a * a;
    (
        [i0 + i1 + i2, i0 + a2 * i1 + a * i2, i0 + a * i1 + a2 * i2],
        3.0 * i0,
    )
}

fn fault_case(method: &str, kind: &str, zf: f64, zg: f64) {
    let (phases, ground) = match kind {
        "three_phase" => (json!(["a", "b", "c"]), zg >= 0.0),
        "single_line_ground" => (json!(["a"]), true),
        "line_line" => (json!(["b", "c"]), false),
        "double_line_ground" => (json!(["b", "c"]), true),
        other => panic!("{other}"),
    };
    let mut comps = source_and_line();
    comps.push(json!({"type": "fault", "name": "F1", "bus": "f", "phases": phases, "ground": ground,
                      "fault_resistance_ohm": zf, "ground_resistance_ohm": zg.max(0.0), "start_time_s": 0.0}));
    let h = 2e-5;
    // The slowest mode, Z0 = 4 + j27 ohm, decays with L/R = 21.5 ms; 0.4 s
    // leaves e^-18.6 = 8e-9 of the initial offset.
    let r = run(&model(
        method,
        h,
        0.4,
        json!(comps),
        json!([{"type": "branch_current", "component": "F1"},
               {"type": "branch_current", "component": "G1"}]),
    ));
    let (expected, ground_current) = sequence_currents(kind, zf, zg.max(0.0));
    let tol = (W * h).powi(2);
    let names = ["a", "b", "c"];
    for (p, name) in names.iter().enumerate() {
        let col = format!("I(F1.{name})");
        if r.columns.contains(&col) {
            check(
                &format!("{method} {kind} zf={zf} zg={zg} {col}"),
                r.phasor(&col, 1),
                expected[p],
                tol,
            );
            // The source carries the fault current back: the only path.
            let source = r.phasor(&format!("I(G1.{name})"), 1);
            check(
                &format!("{method} {kind} I(G1.{name})"),
                -source,
                expected[p],
                tol,
            );
        } else {
            assert!(
                expected[p].abs() < 1e-9 * e_a().abs(),
                "{kind} phase {name}"
            );
        }
    }
    if ground {
        let g = r.phasor("I(F1.g)", 1);
        if ground_current.abs() > 1.0 {
            check(&format!("{method} {kind} I(F1.g)"), g, ground_current, tol);
        } else {
            // Balanced: no current to ground beyond the discretization error.
            assert!(
                g.abs() < tol * expected[0].abs(),
                "{kind}: I(F1.g) = {}",
                g.abs()
            );
        }
    }
}

#[test]
fn fault_currents_match_symmetrical_components() {
    for method in METHODS {
        // Bolted and through resistances. zg < 0 marks a three-phase fault
        // without ground.
        for (kind, zf, zg) in [
            ("three_phase", 0.0, 0.0),
            ("three_phase", 0.5, -1.0),
            ("single_line_ground", 0.0, 0.0),
            ("single_line_ground", 2.0, 1.0),
            ("line_line", 0.0, -1.0),
            ("line_line", 1.0, -1.0),
            ("double_line_ground", 0.0, 0.0),
            ("double_line_ground", 1.0, 3.0),
        ] {
            fault_case(method, kind, zf, zg);
        }
    }
}

/// Single line to ground fault from t = 0 at the end of the line: only
/// phase a carries current, through the self impedance (Z0 + 2 Z1)/3 of the
/// source and the line, so i(t) is that of one RL branch switched onto a
/// cosine source.
fn single_line_ground_from_rest(method: &str, h: f64) -> f64 {
    let mut comps = source_and_line();
    comps.push(json!({"type": "fault", "name": "F1", "bus": "f", "phases": ["a"], "ground": true, "start_time_s": 0.0}));
    let r = run(&model(
        method,
        h,
        0.06,
        json!(comps),
        json!([{"type": "branch_current", "component": "F1"}]),
    ));
    let z = (2.0 * z1() + z0()) * c(1.0 / 3.0, 0.0);
    let (rr, l) = (z.re, z.im / W);
    let amp = 2.0_f64.sqrt() * e_a().abs() / z.abs();
    let theta = (ANGLE.to_radians()) - z.im.atan2(z.re);
    let k = r.col("I(F1.a)");
    let mut err: f64 = 0.0;
    for (t, row) in r.t.iter().zip(&r.rows) {
        let exact = amp * ((W * t + theta).cos() - theta.cos() * (-rr / l * t).exp());
        err = err.max((row[k] - exact).abs());
    }
    err / amp
}

#[test]
fn fault_transient_converges_with_second_order() {
    for method in METHODS {
        let e1 = single_line_ground_from_rest(method, 2e-5);
        let e2 = single_line_ground_from_rest(method, 1e-5);
        let ratio = e1 / e2;
        eprintln!("{method}: SLG transient errors {e1:.3e} (h = 20 us), {e2:.3e} (10 us), ratio {ratio:.3}");
        // Second order: halving the step divides the error by 4.
        assert!(
            (3.6..4.4).contains(&ratio),
            "{method}: errors {e1:.3e}, {e2:.3e}, ratio {ratio:.2}"
        );
        // Size: below (w h)^2 of the peak (see the module comment).
        assert!(e1 < (W * 2e-5).powi(2), "{method}: {e1:.3e}");
    }
}

#[test]
fn each_phase_of_a_fault_clears_at_its_own_current_zero() {
    let mut comps = source_and_line();
    comps.push(
        json!({"type": "fault", "name": "F1", "bus": "f", "phases": ["a", "b", "c"],
                      "start_time_s": 0.0, "clear_time_s": 0.3}),
    );
    let h = 2e-5;
    let r = run(&model(
        "2s-dirk",
        h,
        0.34,
        json!(comps),
        json!([{"type": "branch_current", "component": "F1"}]),
    ));
    let events = &r.stats.events;
    assert_eq!(events.len(), 6, "{events:#?}");
    assert!(events[..3]
        .iter()
        .all(|e| e.action == "close" && e.time_s == 0.0));
    let opens = &events[3..];
    assert!(opens.iter().all(|e| e.action == "open" && e.time_s >= 0.3));
    // Without ground, the first phase interrupts alone; the other two then
    // carry one current between them and open together at its zero, a
    // quarter period later for a symmetrical fault.
    assert!(opens[0].time_s < opens[1].time_s);
    assert_eq!(opens[1].time_s, opens[2].time_s);
    let delay = opens[1].time_s - opens[0].time_s;
    assert!(
        (delay - 0.25 / F).abs() < 0.03 / F,
        "second and third poles {delay:.5} s after the first"
    );
    // Each opens within one step of its zero: the current interrupted is at
    // most |di/dt| h = w I_peak h.
    let peak = 2.0_f64.sqrt() * (e_a() / z1()).abs();
    for e in opens {
        let i = e.interrupted_current_a.unwrap().abs();
        assert!(i <= W * peak * h, "{} interrupts {i:.2} A", e.component);
    }
    // After the last opening no fault current flows.
    let last = opens[2].time_s;
    for name in ["I(F1.a)", "I(F1.b)", "I(F1.c)"] {
        let k = r.col(name);
        for (t, row) in r.t.iter().zip(&r.rows) {
            if *t > last + h / 2.0 {
                assert_eq!(row[k], 0.0, "{name} at {t}");
            }
        }
    }
}

// ----------------------------------------------------------- transformers

const S_MVA: f64 = 30.0;
const V_HV: f64 = 66.0;
const V_LV: f64 = 6.6;
const Z_PU: f64 = 0.10;
const R_PU: f64 = 0.01;

fn transformer(group: &str, i0: f64) -> Value {
    json!({"type": "three_phase_transformer", "name": "T1", "buses": ["h", "l"], "vector_group": group,
           "rated_voltage_kv": [V_HV, V_LV], "rated_power_mva": S_MVA, "rated_frequency_hz": F,
           "impedance_percent": Z_PU * 100.0, "resistance_percent": R_PU * 100.0,
           "no_load_current_percent": i0})
}

fn ideal_source(angle: f64) -> Value {
    json!({"type": "three_phase_source", "name": "G1", "bus": "h", "line_voltage_rms_v": V_HV * 1e3,
           "frequency_hz": F, "angle_deg": angle})
}

/// Leakage impedance of the transformer as a phase-to-neutral (star
/// equivalent) impedance on the side with rated line voltage v_kv.
fn leakage(v_kv: f64) -> C {
    let base = (v_kv * 1e3).powi(2) / (S_MVA * 1e6);
    c(R_PU * base, (Z_PU * Z_PU - R_PU * R_PU).sqrt() * base)
}

#[test]
fn vector_groups_give_the_voltage_ratio_and_phase_shift() {
    // A resistive star load of half the rating, grounded, on the low-voltage
    // side. Positive sequence: per phase, the source voltage divided by the
    // line voltage ratio and turned by -30 deg per clock hour drives the
    // leakage impedance and the load in series.
    let r_load = (V_LV * 1e3).powi(2) / (0.5 * S_MVA * 1e6);
    let h = 2e-5;
    for group in [
        "YNyn0", "YNyn6", "Yyn0", "YNy0", "YNd1", "YNd5", "YNd7", "YNd11", "Yd1", "Yd11", "Dyn1",
        "Dyn5", "Dyn7", "Dyn11", "Dy11", "Dd0", "Dd6",
    ] {
        let clock: f64 = group
            .trim_start_matches(|ch: char| ch.is_alphabetic())
            .parse()
            .unwrap();
        let mut comps = vec![ideal_source(10.0), transformer(group, 0.0)];
        for p in ["a", "b", "c"] {
            comps.push(json!({"type": "resistor", "name": format!("R{p}"), "nodes": [format!("l.{p}"), "gnd"], "resistance_ohm": r_load}));
        }
        // With the load (2.9 ohm) in series with the leakage (0.14 ohm), L/R
        // is 0.16 ms: the transient is gone long before the last period.
        let r = run(&model(
            "2s-dirk",
            h,
            0.1,
            json!(comps),
            json!([{"type": "bus_voltage", "bus": "l"}]),
        ));
        let e = polar(V_HV * 1e3 / 3.0_f64.sqrt(), 10.0);
        let open_circuit = (V_LV / V_HV) * e * polar(1.0, -30.0 * clock);
        let expected = open_circuit * c(r_load, 0.0) / (c(r_load, 0.0) + leakage(V_LV));
        let tol = (W * h).powi(2);
        for (p, name) in ["a", "b", "c"].iter().enumerate() {
            let shift = polar(1.0, -120.0 * p as f64);
            check(
                &format!("{group} V(l.{name})"),
                r.phasor(&format!("V(l.{name})"), 1),
                expected * shift,
                tol,
            );
        }
    }
}

#[test]
fn short_circuit_current_follows_from_the_impedance_percent() {
    // Rated voltage on the high-voltage side, a bolted three-phase fault on
    // the low-voltage side: I = S / (sqrt(3) V z) on each side.
    let h = 2e-5;
    for (group, i0) in [("YNd11", 0.0), ("Dyn11", 0.0), ("YNyn0", 2.0)] {
        let clock: f64 = group
            .trim_start_matches(|ch: char| ch.is_alphabetic())
            .parse()
            .unwrap();
        let comps = json!([
            ideal_source(0.0),
            transformer(group, i0),
            {"type": "fault", "name": "F1", "bus": "l", "phases": ["a", "b", "c"], "ground": true, "start_time_s": 0.0}
        ]);
        // Leakage L/R = sqrt(z^2 - r^2)/(w r) = 32 ms; 0.6 s leaves e^-19.
        let r = run(&model(
            "2s-dirk",
            h,
            0.6,
            comps,
            json!([{"type": "branch_current", "component": "F1"},
                   {"type": "branch_current", "component": "T1"}]),
        ));
        let e = polar(V_HV * 1e3 / 3.0_f64.sqrt(), 0.0);
        let ratio = V_HV / V_LV;
        let shift = polar(1.0, -30.0 * clock);
        let z_h = leakage(V_HV);
        // T equivalent referred to the high-voltage side: half the leakage,
        // then the magnetizing admittance in parallel with the other half.
        let (i_hv, i_lv) = if i0 == 0.0 {
            let i = e / z_h;
            (i, i * c(ratio, 0.0) * shift)
        } else {
            let base = (V_HV * 1e3).powi(2) / (S_MVA * 1e6);
            let z_m = c(0.0, base / (i0 / 100.0));
            let half = c(0.5, 0.0) * z_h;
            let i = e / (half + parallel(z_m, half));
            let to_lv = i * z_m / (z_m + half);
            (i, to_lv * c(ratio, 0.0) * shift)
        };
        let tol = (W * h).powi(2);
        if i0 == 0.0 {
            let rated = S_MVA * 1e6 / (3.0_f64.sqrt() * V_LV * 1e3);
            assert!(
                ((i_lv.abs() - rated / Z_PU) / (rated / Z_PU)).abs() < 1e-12,
                "hand calculation"
            );
        }
        check(
            &format!("{group} I(F1.a)"),
            r.phasor("I(F1.a)", 1),
            i_lv,
            tol,
        );
        check(
            &format!("{group} I(T1.0.a)"),
            r.phasor("I(T1.0.a)", 1),
            i_hv,
            tol,
        );
        // The transformer's low-voltage line current feeds the fault.
        check(
            &format!("{group} I(T1.1.a)"),
            -r.phasor("I(T1.1.a)", 1),
            i_lv,
            tol,
        );
    }
}

#[test]
fn magnetizing_current_follows_from_the_no_load_current() {
    // YNyn0 is three independent units: phase a alone is one RL branch
    // (half the leakage and the magnetizing inductance). Starting the source
    // at the angle where the forced current is zero leaves no transient in
    // that phase.
    let i0 = 2.0;
    let base = (V_HV * 1e3).powi(2) / (S_MVA * 1e6);
    let z = c(0.5, 0.0) * leakage(V_HV) + c(0.0, base / (i0 / 100.0));
    let angle = 90.0 + z.deg();
    let comps = json!([ideal_source(angle), transformer("YNyn0", i0)]);
    let h = 2e-5;
    let r = run(&model(
        "2s-dirk",
        h,
        0.04,
        comps,
        json!([{"type": "branch_current", "component": "T1"}]),
    ));
    let e = polar(V_HV * 1e3 / 3.0_f64.sqrt(), angle);
    let expected = e / z;
    check(
        "I(T1.0.a)",
        r.phasor("I(T1.0.a)", 1),
        expected,
        (W * h).powi(2),
    );
    // Against the no-load current itself: the half leakage in series changes
    // the magnitude by |z/2| / |z_m| = 0.10/2 * 0.02 = 1e-3.
    let rated = S_MVA * 1e6 / (3.0_f64.sqrt() * V_HV * 1e3);
    let measured = r.phasor("I(T1.0.a)", 1).abs() / rated * 100.0;
    assert!(
        ((measured - i0) / i0).abs() < 1.5e-3,
        "{measured:.5} percent"
    );
    // No current on the open low-voltage side.
    assert!(r.phasor("I(T1.1.a)", 1).abs() < 1e-6 * rated);
}

#[test]
fn a_ynd_transformer_is_a_zero_sequence_source_for_a_ground_fault() {
    // Source (Z1s, Z0s) on bus h with an YNd11 transformer whose delta side
    // is open (grounded through 1 Mohm). For a ground fault on h, the delta
    // closes the zero-sequence path through the leakage impedance, while the
    // positive and negative sequences see only the source.
    let h = 2e-5;
    let z1s = c(0.5, 5.0);
    let z0s = c(1.0, 15.0);
    let comps = json!([
        {"type": "three_phase_source", "name": "G1", "bus": "h", "line_voltage_rms_v": V_HV * 1e3,
         "frequency_hz": F, "impedance": {"type": "sequence", "r1_ohm": z1s.re, "l1_h": z1s.im / W, "r0_ohm": z0s.re, "l0_h": z0s.im / W}},
        transformer("YNd11", 0.0),
        {"type": "resistor", "name": "RG", "nodes": ["l.a", "gnd"], "resistance_ohm": 1e6},
        {"type": "fault", "name": "F1", "bus": "h", "phases": ["a"], "ground": true, "start_time_s": 0.0}
    ]);
    let r = run(&model(
        "2s-dirk",
        h,
        0.8,
        comps,
        json!([{"type": "branch_current", "component": "F1"},
               {"type": "branch_current", "component": "T1"}]),
    ));
    let e = polar(V_HV * 1e3 / 3.0_f64.sqrt(), 0.0);
    let zt = leakage(V_HV);
    let z0 = parallel(z0s, zt);
    let i0 = e / (2.0 * z1s + z0);
    // The 1 Mohm resistor shifts the delta's potential, not its currents:
    // its effect on the fault current is below 1e-8.
    let tol = (W * h).powi(2);
    check("I(F1.a)", r.phasor("I(F1.a)", 1), 3.0 * i0, tol);
    let transformer_share = i0 * z0s / (z0s + zt);
    for p in ["a", "b", "c"] {
        // Pure zero sequence in the transformer: the same in every phase,
        // out of the transformer into the fault (negative into the windings).
        check(
            &format!("I(T1.0.{p})"),
            -r.phasor(&format!("I(T1.0.{p})"), 1),
            transformer_share,
            tol,
        );
    }
}

#[test]
fn single_phase_transformer_ratio_and_polarity() {
    // 6.6 kV / 200 V, 100 kVA, z = 4 %, r = 1 %, resistive load of the rating.
    let h = 2e-5;
    for (secondary, sign) in [(json!(["s", "gnd"]), 1.0), (json!(["gnd", "s"]), -1.0)] {
        let r_load = 200.0_f64.powi(2) / 1e5;
        let comps = json!([
            {"type": "voltage_source", "name": "E1", "nodes": ["p", "gnd"],
             "waveform": {"type": "sine", "amplitude": 6600.0 * 2.0_f64.sqrt(), "frequency_hz": F, "phase_deg": 90.0}},
            {"type": "transformer", "name": "T1", "windings": [["p", "gnd"], secondary], "rated_voltage_kv": [6.6, 0.2],
             "rated_power_mva": 0.1, "rated_frequency_hz": F, "impedance_percent": 4.0, "resistance_percent": 1.0},
            {"type": "resistor", "name": "RL", "nodes": ["s", "gnd"], "resistance_ohm": r_load}
        ]);
        let r = run(&model(
            "trapezoidal",
            h,
            0.1,
            comps,
            json!([{"type": "node_voltage", "node": "s"},
                   {"type": "branch_current", "component": "T1"}]),
        ));
        let base = 200.0_f64.powi(2) / 1e5;
        let zl = c(
            0.01 * base,
            (0.04_f64.powi(2) - 0.01_f64.powi(2)).sqrt() * base,
        );
        let e = c(6600.0 / 33.0, 0.0);
        let u = sign * (e * c(r_load, 0.0) / (c(r_load, 0.0) + zl));
        let tol = (W * h).powi(2);
        check("V(s)", r.phasor("V(s)", 1), u, tol);
        // Power balance through the ideal transformer: the primary current is
        // the load current divided by the ratio 33.
        let i_load = u / c(r_load, 0.0);
        check(
            "I(T1.0)",
            r.phasor("I(T1.0)", 1),
            sign * i_load * c(1.0 / 33.0, 0.0),
            tol,
        );
        check("I(T1.1)", -r.phasor("I(T1.1)", 1), sign * i_load, tol);
    }
}

// ---------------------------------------------------------------- pi lines

/// Per km: r = 0.03 ohm, l = 0.9 mH, c = 12.7 nF (positive sequence), 400 km.
const LEN: f64 = 400.0;
const R1: f64 = 0.03;
const L1: f64 = 0.9e-3;
const C1: f64 = 12.7e-9;

fn line_end_voltage(sections: u32, h: f64) -> C {
    let comps = json!([
        ideal_source(0.0),
        {"type": "pi_line", "name": "LINE1", "buses": ["h", "r"], "length_km": LEN, "sections": sections,
         "per_km": {"type": "sequence", "r1_ohm_per_km": R1, "l1_h_per_km": L1, "c1_f_per_km": C1,
                    "r0_ohm_per_km": 0.2, "l0_h_per_km": 3e-3, "c0_f_per_km": 7e-9}}
    ]);
    // The lumped line rings at its natural frequencies, damped by the series
    // resistance with L/R = 2 l / r = 60 ms; 1.2 s leaves e^-20.
    let r = run(&model(
        "2s-dirk",
        h,
        1.2,
        comps,
        json!([{"type": "bus_voltage", "bus": "r"}]),
    ));
    r.phasor("V(r.a)", 1)
}

/// A over the line: product of the ABCD matrices of n pi sections; the
/// receiving end is open, so V_r = V_s / A.
fn pi_sections_a(n: u32) -> C {
    let d = LEN / n as f64;
    let z = c(R1 * d, W * L1 * d);
    let y = c(0.0, W * C1 * d);
    let half = c(1.0, 0.0) + z * y * c(0.5, 0.0);
    let section = [[half, z], [y * (c(1.0, 0.0) + z * y * c(0.25, 0.0)), half]];
    let mut m = [[c(1.0, 0.0), c(0.0, 0.0)], [c(0.0, 0.0), c(1.0, 0.0)]];
    for _ in 0..n {
        let mut next = [[c(0.0, 0.0); 2]; 2];
        for i in 0..2 {
            for j in 0..2 {
                next[i][j] = m[i][0] * section[0][j] + m[i][1] * section[1][j];
            }
        }
        m = next;
    }
    m[0][0]
}

/// cosh(gamma * length) of the distributed line.
fn distributed_a() -> C {
    let zy = c(R1, W * L1) * c(0.0, W * C1);
    // sqrt of a complex number.
    let r = zy.abs().sqrt();
    let gamma_l = c(LEN, 0.0) * polar(r, zy.deg() / 2.0);
    let ex = |x: C| polar(x.re.exp(), x.im.to_degrees());
    (ex(gamma_l) + ex(-gamma_l)) * c(0.5, 0.0)
}

#[test]
fn pi_line_shows_the_ferranti_rise_of_its_sections() {
    let h = 1e-5;
    let e = polar(V_HV * 1e3 / 3.0_f64.sqrt(), 0.0);
    for n in [1, 4] {
        let got = line_end_voltage(n, h);
        check(
            &format!("{n} sections"),
            got,
            e / pi_sections_a(n),
            (W * h).powi(2),
        );
        assert!(
            got.abs() > 1.05 * e.abs(),
            "Ferranti rise: {}",
            got.abs() / e.abs()
        );
    }
}

#[test]
fn more_pi_sections_approach_the_distributed_line() {
    // The error of n lumped sections against cosh(gamma l) falls as 1/n^2;
    // the discretization error of the run, below (w h)^2 = 1e-5, stays far
    // below the error of 4 sections (about 8e-5).
    let h = 1e-5;
    let e = polar(V_HV * 1e3 / 3.0_f64.sqrt(), 0.0);
    let exact = e / distributed_a();
    let errors: Vec<f64> = [1, 2, 4]
        .iter()
        .map(|&n| (line_end_voltage(n, h) - exact).abs() / exact.abs())
        .collect();
    eprintln!("pi sections 1, 2, 4 against the distributed line: {errors:?}");
    for w in errors.windows(2) {
        let ratio = w[0] / w[1];
        assert!((3.5..4.5).contains(&ratio), "errors {errors:?}");
    }
    assert!(errors[2] < 2e-4, "{errors:?}");
}

#[test]
fn three_phase_transformer_tap_changes_step_ratio_and_preserve_core_flux() {
    // Loaded YNd11 transformer changing both HV tap (changing core volts/turn)
    // and LV tap while magnetized; checks post-tap phasor against equivalent
    // circuit and verifies interpolated mode produces the same steady result.
    let h = 2e-5;
    let r_load = (V_LV * 1e3).powi(2) / (0.5 * S_MVA * 1e6);
    for switching in ["grid", "interpolated"] {
        let mut t = transformer("YNd11", 0.5);
        t["tap_changes"] = json!([
            {"time_s": 0.02, "rated_voltage_kv": [V_HV * 0.95, V_LV], "impedance_percent": 12.0, "resistance_percent": 1.5},
            {"time_s": 0.05, "rated_voltage_kv": [V_HV * 0.95, V_LV * 1.05], "impedance_percent": 11.0, "resistance_percent": 1.2}
        ]);
        let mut m = model_of(
            "2s-dirk",
            h,
            0.25,
            Some("steady_state"),
            json!([
                ideal_source(0.0),
                t,
                {"type": "resistor", "name": "Ra", "nodes": ["l.a", "gnd"], "resistance_ohm": r_load},
                {"type": "resistor", "name": "Rb", "nodes": ["l.b", "gnd"], "resistance_ohm": r_load},
                {"type": "resistor", "name": "Rc", "nodes": ["l.c", "gnd"], "resistance_ohm": r_load},
            ]),
            json!([{"type": "bus_voltage", "bus": "l"}]),
        );
        m["version"] = json!("0.6");
        m["simulation"]["switching"] = json!(switching);
        let r = run(&m);
        assert_eq!(r.stats.events.len(), 2, "{:?}", r.stats.events);
        assert!(r.stats.events.iter().all(|e| e.action == "step"));
        // Final state: HV = 0.95 * 66 kV, LV = 1.05 * 6.6 kV, Z = 11 %, R = 1.2 %, YNd11 (-330/+30 deg).
        // Phase-a HV = 66e3 / sqrt(3) < 0; ideal ratio k = (1.05*6.6)/(0.95*66) * e^{+j 30 deg}.
        let k_mag = (1.05 * V_LV) / (0.95 * V_HV);
        let e_sec = polar(V_HV * 1e3 / 3.0_f64.sqrt(), 0.0) * polar(k_mag, 30.0);
        let base_lv = (1.05 * V_LV * 1e3).powi(2) / (S_MVA * 1e6);
        let z_pu_f = 0.11;
        let r_pu_f = 0.012;
        let z_leak = c(
            r_pu_f * base_lv,
            (z_pu_f * z_pu_f - r_pu_f * r_pu_f).sqrt() * base_lv,
        );
        // Include magnetizing branch in parallel at primary (Ym = -j i0_pu / Zbase_hv scaled).
        // With i0 = 0.5%, half leakage on primary and half on secondary gives the exact 2-port divider:
        let v_expected = e_sec * c(r_load, 0.0) / (c(r_load, 0.0) + z_leak);
        let got = r.phasor("V(l.a)", 1);
        let rel = (got - v_expected).abs() / v_expected.abs();
        // Neglecting 0.5% magnetizing drop across half leakage introduces ~0.5% * 5.5% ~ 3e-4.
        assert!(rel < 5e-4, "{switching}: rel = {rel:.3e}");
    }
}

#[test]
fn coupled_rl_impedance_changes_switch_cleanly_at_scheduled_time() {
    let mut m = model_of(
        "2s-dirk",
        2e-5,
        0.16,
        Some("steady_state"),
        json!([
            {"type": "three_phase_source", "name": "G1", "bus": "s", "line_voltage_rms_v": 10e3, "frequency_hz": F},
            {"type": "coupled_rl", "name": "Z1", "buses": ["s", "l"],
             "impedance": {"type": "sequence", "r1_ohm": 2.0, "l1_h": 0.01, "r0_ohm": 4.0, "l0_h": 0.03},
             "impedance_changes": [
                 {"time_s": 0.03, "impedance": {"type": "sequence", "r1_ohm": 8.0, "l1_h": 0.02, "r0_ohm": 12.0, "l0_h": 0.05}}
             ]},
            {"type": "resistor", "name": "Ra", "nodes": ["l.a", "gnd"], "resistance_ohm": 10.0},
            {"type": "resistor", "name": "Rb", "nodes": ["l.b", "gnd"], "resistance_ohm": 10.0},
            {"type": "resistor", "name": "Rc", "nodes": ["l.c", "gnd"], "resistance_ohm": 10.0},
        ]),
        json!([{"type": "bus_voltage", "bus": "l"}]),
    );
    m["version"] = json!("0.6");
    let r = run(&m);
    assert_eq!(r.stats.events.len(), 1);
    assert_eq!(r.stats.events[0].action, "step");
    let e = polar(10e3 / 3.0_f64.sqrt(), 0.0);
    let z1_final = c(8.0, W * 0.02);
    let v_exact = e * c(10.0, 0.0) / (c(10.0, 0.0) + z1_final);
    check(
        "coupled_rl changed",
        r.phasor("V(l.a)", 1),
        v_exact,
        (W * 2e-5).powi(2),
    );
}
