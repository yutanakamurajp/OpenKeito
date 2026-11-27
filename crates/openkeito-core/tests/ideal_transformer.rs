//! The ideal three-phase transformer against its definition.
//!
//! Side 0 is driven by three single-phase sources with an unbalanced set of
//! phasors (positive, negative and zero sequence at once); side 1 feeds a
//! resistive star. Expected: V1 = (v1/v0) e^{+j alpha} V0 in the positive
//! sequence, e^{-j alpha} in the negative one, the plain ratio in the zero
//! sequence, and no power lost. The phasor solution has no time step and
//! agrees to rounding; the waveform departs by less than (w h)^2.

mod common;

use std::f64::consts::PI;

use common::phasors::{Complex64, F, W};
use common::run_model;
use serde_json::json;

const H: f64 = 2e-5;

fn sequences(v: [Complex64; 3]) -> [Complex64; 3] {
    let a = Complex64::from_polar(1.0, 2.0 * PI / 3.0);
    [
        (v[0] + v[1] + v[2]) / 3.0,
        (v[0] + a * v[1] + a * a * v[2]) / 3.0,
        (v[0] + a * a * v[1] + a * v[2]) / 3.0,
    ]
}

#[test]
fn the_sequences_turn_by_plus_and_minus_the_phase_shift() {
    let (v0, v1, alpha): (f64, f64, f64) = (400.0, 110.0, 2.15);
    // Phase-to-ground phasors (rms, cosine reference) of the sources.
    let set = [
        Complex64::from_polar(230e3, 0.3),
        Complex64::from_polar(215e3, 0.3 - 2.0),
        Complex64::from_polar(240e3, 0.3 + 2.2),
    ];
    let mut components = vec![
        json!({"type": "three_phase_ideal_transformer", "name": "T", "buses": ["p", "s"],
        "rated_voltage_kv": [v0, v1], "phase_shift_deg": alpha}),
    ];
    for (k, ph) in ["a", "b", "c"].iter().enumerate() {
        // A sin(wt + phi) has the phasor (A / sqrt 2) e^{j(phi - 90 deg)}.
        components.push(json!({"type": "voltage_source", "name": format!("E{ph}"), "nodes": [format!("p.{ph}"), "gnd"],
            "waveform": {"type": "sine", "amplitude": 2.0_f64.sqrt() * set[k].norm(), "frequency_hz": F,
                         "phase_deg": set[k].arg().to_degrees() + 90.0}}));
        components.push(json!({"type": "resistor", "name": format!("R{ph}"), "nodes": [format!("s.{ph}"), "gnd"],
            "resistance_ohm": 20.0 + 5.0 * k as f64}));
    }
    let m = json!({
        "format": "openkeito-model", "version": "0.4",
        "simulation": {"time_step_s": H, "end_time_s": 0.02, "initialization": "steady_state"},
        "components": components,
        "outputs": [{"type": "bus_voltage", "bus": "p"}, {"type": "bus_voltage", "bus": "s"},
                    {"type": "branch_current", "component": "T"}],
    });
    let r = run_model(&m);
    let three =
        |f: &dyn Fn(&str) -> String| [r.steady(&f("a")), r.steady(&f("b")), r.steady(&f("c"))];
    let (vp, vs) = (
        three(&|p| format!("V(p.{p})")),
        three(&|p| format!("V(s.{p})")),
    );
    let (ip, is) = (
        three(&|p| format!("I(T.0.{p})")),
        three(&|p| format!("I(T.1.{p})")),
    );
    for k in 0..3 {
        assert!((vp[k] - set[k]).norm() < 1e-9 * set[k].norm());
    }
    let (sp, ss) = (sequences(vp), sequences(vs));
    let rho = v1 / v0;
    let turn = Complex64::from_polar(1.0, alpha.to_radians());
    let expected = [rho * sp[0], rho * turn * sp[1], rho * turn.conj() * sp[2]];
    for (k, label) in ["zero", "positive", "negative"].iter().enumerate() {
        let e = (ss[k] - expected[k]).norm() / expected[k].norm();
        eprintln!("{label} sequence: relative error {e:.2e}");
        assert!(e < 1e-12, "{label}: {} against {}", ss[k], expected[k]);
    }
    // No power is stored or lost: what flows in at side 0 leaves at side 1.
    let s_in: Complex64 = (0..3)
        .map(|k| vp[k] * ip[k].conj() + vs[k] * is[k].conj())
        .sum();
    let s_load: f64 = (0..3)
        .map(|k| vs[k].norm_sqr() / (20.0 + 5.0 * k as f64))
        .sum();
    eprintln!("power balance: {:.2e} of {s_load:.3e} W", s_in.norm());
    assert!(s_in.norm() < 1e-12 * s_load);
    // The waveforms stay at the steady state.
    let init = r.stats.initialization.as_ref().unwrap();
    assert!(
        init.relative_voltage_deviation < (W * H).powi(2),
        "{}",
        init.relative_voltage_deviation
    );
}

#[test]
fn zero_sequence_inverted_turns_zero_sequence_by_180_degrees_as_yn_yn6() {
    let (v0, v1): (f64, f64) = (400.0, 110.0);
    let set = [
        Complex64::from_polar(230e3, 0.3),
        Complex64::from_polar(215e3, 0.3 - 2.0),
        Complex64::from_polar(240e3, 0.3 + 2.2),
    ];
    let mut components = vec![json!({
        "type": "three_phase_ideal_transformer", "name": "T", "buses": ["p", "s"],
        "rated_voltage_kv": [v0, v1], "phase_shift_deg": 180.0,
        "zero_sequence_inverted": true
    })];
    for (k, ph) in ["a", "b", "c"].iter().enumerate() {
        components.push(json!({
            "type": "voltage_source", "name": format!("E{ph}"), "nodes": [format!("p.{ph}"), "gnd"],
            "waveform": {"type": "sine", "amplitude": 2.0_f64.sqrt() * set[k].norm(), "frequency_hz": F,
                         "phase_deg": set[k].arg().to_degrees() + 90.0}
        }));
        components.push(json!({
            "type": "resistor", "name": format!("R{ph}"), "nodes": [format!("s.{ph}"), "gnd"],
            "resistance_ohm": 20.0 + 5.0 * k as f64
        }));
    }
    let m = json!({
        "format": "openkeito-model", "version": "0.6",
        "simulation": {"time_step_s": H, "end_time_s": 0.02, "initialization": "steady_state"},
        "components": components,
        "outputs": [{"type": "bus_voltage", "bus": "p"}, {"type": "bus_voltage", "bus": "s"},
                    {"type": "branch_current", "component": "T"}],
    });
    let r = run_model(&m);
    let three =
        |f: &dyn Fn(&str) -> String| [r.steady(&f("a")), r.steady(&f("b")), r.steady(&f("c"))];
    let (vp, vs) = (
        three(&|p| format!("V(p.{p})")),
        three(&|p| format!("V(s.{p})")),
    );
    let (ip, is) = (
        three(&|p| format!("I(T.0.{p})")),
        three(&|p| format!("I(T.1.{p})")),
    );
    let (sp, ss) = (sequences(vp), sequences(vs));
    let rho = v1 / v0;
    // With alpha = 180 deg and zero_sequence_inverted = true, every sequence is -rho * Vp.
    for (k, label) in ["zero", "positive", "negative"].iter().enumerate() {
        let exp = -rho * sp[k];
        let e = (ss[k] - exp).norm() / exp.norm();
        assert!(e < 1e-12, "{label}: {} against {exp}", ss[k]);
    }
    let s_in: Complex64 = (0..3)
        .map(|k| vp[k] * ip[k].conj() + vs[k] * is[k].conj())
        .sum();
    let s_load: f64 = (0..3)
        .map(|k| vs[k].norm_sqr() / (20.0 + 5.0 * k as f64))
        .sum();
    assert!(s_in.norm() < 1e-12 * s_load);
}

#[test]
fn ideal_transformer_tap_changes_step_ratio_and_phase_shift() {
    let m = json!({
        "format": "openkeito-model", "version": "0.6",
        "simulation": {"time_step_s": H, "end_time_s": 0.06, "initialization": "steady_state"},
        "components": [
            {"type": "three_phase_source", "name": "G", "bus": "p", "line_voltage_rms_v": 110e3, "frequency_hz": F},
            {"type": "three_phase_ideal_transformer", "name": "T", "buses": ["p", "s"],
             "rated_voltage_kv": [110.0, 22.0], "phase_shift_deg": 0.0,
             "tap_changes": [
                 {"time_s": 0.02, "rated_voltage_kv": [100.0, 22.0], "phase_shift_deg": 30.0},
                 {"time_s": 0.04, "rated_voltage_kv": [110.0, 33.0]}
             ]},
            {"type": "resistor", "name": "Ra", "nodes": ["s.a", "gnd"], "resistance_ohm": 100.0},
            {"type": "resistor", "name": "Rb", "nodes": ["s.b", "gnd"], "resistance_ohm": 100.0},
            {"type": "resistor", "name": "Rc", "nodes": ["s.c", "gnd"], "resistance_ohm": 100.0},
        ],
        "outputs": [{"type": "bus_voltage", "bus": "p"}, {"type": "bus_voltage", "bus": "s"}],
    });
    let r = run_model(&m);
    assert_eq!(r.stats.events.len(), 2);
    // In [0.02, 0.04): Vs = (22/100) * e^{+j 30 deg} * Vp.
    // Final window [0.04, 0.06) has Vs = (33/110) * e^{+j 0 deg} * Vp.
    let vp = r.phasor("V(p.a)", 1);
    let vs = r.phasor("V(s.a)", 1);
    let expected = (33.0 / 110.0) * vp;
    assert!((vs - expected).norm() / expected.norm() < 1e-10);
    // Check mid-window [0.02, 0.04) directly from samples at 0.02..0.04.
    let ia = r.col("V(p.a)");
    let ib = r.col("V(s.a)");
    let mid_err =
        r.t.iter()
            .zip(&r.rows)
            .filter(|(t, _)| **t >= 0.02 + H / 2.0 && **t < 0.04 - H / 2.0)
            .map(|(t, row)| {
                // Peak Vp.a = sqrt(2/3) * 110e3; Vs.a should be 0.22 * Vp shifted by +30 deg.
                let vp_ph = Complex64::from_polar(110e3 / 3.0_f64.sqrt(), W * t);
                let vs_ph = 0.22 * Complex64::from_polar(1.0, 30.0_f64.to_radians()) * vp_ph;
                let expected_s = 2.0_f64.sqrt() * vs_ph.re;
                let _ = row[ia];
                (row[ib] - expected_s).abs() / (2.0_f64.sqrt() * 0.22 * 110e3 / 3.0_f64.sqrt())
            })
            .fold(0.0_f64, f64::max);
    assert!(mid_err < 1e-5, "mid_err = {mid_err}");
}

// ---------------------------------------------------------------------------
// The ideal transformer built from dependent sources (vcvs and cccs) against
// the ideal three-phase transformer and the single-phase transformer.
//
// Per phase, with the ratio n = v0 / v1 from node p (side 0) to node s
// (side 1): the sense port of a cccs from p to an inner node x, a vcvs from
// x to gnd with V(x) = n V(s), so that V(p) = n V(s), and the output of the
// cccs from gnd into s with gain n, so that n times the current into side 0
// leaves side 1 (V(p) i_0 = V(s) n i_0: no power is stored or lost). The
// equations are those of the ideal transformer with other unknowns, so the
// runs agree to rounding, not to the bit.

/// Ratio and ratings of the transformers compared.
const V0_KV: f64 = 6.6;
const V1_KV: f64 = 0.4;

/// The dependent sources of one phase between `p` and `s` (names `<tag>`).
fn dependent_phase(tag: &str, p: &str, s: &str) -> Vec<serde_json::Value> {
    let n = V0_KV / V1_KV;
    let x = format!("x{tag}");
    vec![
        json!({"type": "cccs", "name": format!("F{tag}"), "sense": [p, x], "output": ["gnd", s], "gain_a_per_a": n}),
        json!({"type": "vcvs", "name": format!("E{tag}"), "control": [s, "gnd"], "output": [x, "gnd"], "gain_v_per_v": n}),
    ]
}

/// The largest differences of the columns `pairs` (each: name in `a`, name
/// in `b`, sign of b) between two runs on the same grid, relative to the
/// peak of the column in `a`: in the row at t = 0 and in the others. The
/// row at t = 0 is the solution at t = 0+, whose stage coefficient
/// 1e6 / time_step_s makes it the worst conditioned of the run.
fn largest_difference(
    a: &common::Run,
    b: &common::Run,
    pairs: &[(String, String, f64)],
) -> (f64, f64) {
    assert_eq!(a.t, b.t);
    let (mut first, mut rest) = (0.0_f64, 0.0_f64);
    for (x, y, sign) in pairs {
        let (i, j) = (a.col(x), b.col(y));
        let peak = a.rows.iter().map(|r| r[i].abs()).fold(0.0, f64::max);
        assert!(peak > 0.0, "{x} is 0");
        let d = |ra: &Vec<f64>, rb: &Vec<f64>| (ra[i] - sign * rb[j]).abs() / peak;
        first = first.max(d(&a.rows[0], &b.rows[0]));
        for (ra, rb) in a.rows.iter().zip(&b.rows).skip(1) {
            rest = rest.max(d(ra, rb));
        }
    }
    (first, rest)
}

/// Checks the differences of two runs: 1e-12 of the peak on the grid
/// after t = 0, the rounding bound of the solver (the condition estimate
/// times the machine epsilon) at t = 0+, and the instants of the events
/// within 1e-12 s. With diodes the instants are the criterion: an instant
/// placed 1e-13 s apart moves the steep edges of a commutation by that
/// much, which leaves up to 1e-10 of the peak on the grid (1e-9 allowed).
fn same_to_rounding(
    label: &str,
    a: &common::Run,
    b: &common::Run,
    pairs: &[(String, String, f64)],
    diodes: bool,
) {
    let (first, rest) = largest_difference(a, b, pairs);
    let e = event_difference(a, b);
    let bound = a
        .stats
        .max_condition_estimate
        .max(b.stats.max_condition_estimate)
        * f64::EPSILON;
    eprintln!(
        "{label}: largest difference {rest:.1e} of the peak after t = 0, {first:.1e} at t = 0+ \
         (bound {bound:.1e}), {} events apart by at most {e:.1e} s",
        a.stats.events.len()
    );
    assert!(
        rest < if diodes { 1e-9 } else { 1e-12 },
        "{label}: {rest:e}"
    );
    assert!(first < bound, "{label}: {first:e} at t = 0+");
    assert!(e < 1e-12, "{label}: {e:e} s");
}

/// The largest difference of the times of the events of two runs, which
/// must have the same events.
fn event_difference(a: &common::Run, b: &common::Run) -> f64 {
    let of = |r: &common::Run| -> Vec<(String, &'static str, f64)> {
        r.stats
            .events
            .iter()
            .map(|e| (e.component.clone(), e.action, e.time_s))
            .collect()
    };
    let (ea, eb) = (of(a), of(b));
    assert_eq!(ea.len(), eb.len(), "{ea:?}\n{eb:?}");
    ea.iter()
        .zip(&eb)
        .map(|(x, y)| {
            assert_eq!((&x.0, x.1), (&y.0, y.1));
            (x.2 - y.2).abs()
        })
        .fold(0.0, f64::max)
}

/// What is connected to side 1 in the cases compared.
#[derive(Clone, Copy, Debug)]
enum Load {
    Resistive,
    /// An RL load switched on at 3.3 ms.
    Energized,
    /// A resistive load, and a short circuit of side 1 from 5 ms that is
    /// cleared at the current zeros after 12 ms.
    Short,
    /// A diode bridge feeding R and C.
    Rectifier,
    /// A resistive and an RL load, from the steady state.
    SteadyState,
}

const LOADS: [Load; 5] = [
    Load::Resistive,
    Load::Energized,
    Load::Short,
    Load::Rectifier,
    Load::SteadyState,
];

/// The components at the side-1 nodes `s` (one per phase) for `load`, with
/// the names of the currents to compare.
fn side_1(load: Load, s: &[String], bus: Option<&str>) -> (Vec<serde_json::Value>, Vec<String>) {
    let mut c = Vec::new();
    let mut currents = Vec::new();
    let resistive = |c: &mut Vec<serde_json::Value>, currents: &mut Vec<String>, ohm: f64| {
        for (k, node) in s.iter().enumerate() {
            let name = format!("RL{k}");
            c.push(json!({"type": "resistor", "name": name, "nodes": [node, "gnd"], "resistance_ohm": ohm}));
            currents.push(name);
        }
    };
    match load {
        Load::Resistive => resistive(&mut c, &mut currents, 1.5),
        Load::Energized | Load::SteadyState => {
            if let Load::SteadyState = load {
                resistive(&mut c, &mut currents, 3.0);
            }
            for (k, node) in s.iter().enumerate() {
                let (l, m) = (format!("l{k}"), format!("m{k}"));
                if let Load::Energized = load {
                    c.push(json!({"type": "switch", "name": format!("S{k}"), "nodes": [node, l], "close_time_s": 3.3e-3}));
                } else {
                    c.push(json!({"type": "resistor", "name": format!("RS{k}"), "nodes": [node, l], "resistance_ohm": 1e-3}));
                }
                c.push(json!({"type": "resistor", "name": format!("RR{k}"), "nodes": [l, m], "resistance_ohm": 1.0}));
                c.push(json!({"type": "inductor", "name": format!("LL{k}"), "nodes": [m, "gnd"], "inductance_h": 5e-3}));
                currents.push(format!("LL{k}"));
            }
        }
        Load::Short => {
            resistive(&mut c, &mut currents, 1.5);
            match bus {
                Some(bus) => {
                    c.push(json!({"type": "fault", "name": "FLT", "bus": bus, "phases": ["a", "b", "c"], "ground": true,
                        "start_time_s": 5e-3, "clear_time_s": 12e-3}));
                    for ph in ["a", "b", "c"] {
                        currents.push(format!("FLT.{ph}"));
                    }
                }
                None => {
                    c.push(json!({"type": "switch", "name": "SC", "nodes": [s[0], "gnd"], "close_time_s": 5e-3,
                        "open_time_s": 12e-3, "open_at_current_zero": true}));
                    currents.push("SC".into());
                }
            }
        }
        Load::Rectifier => {
            for (k, node) in s.iter().enumerate() {
                c.push(json!({"type": "diode", "name": format!("DP{k}"), "nodes": [node, "pos"]}));
                c.push(json!({"type": "diode", "name": format!("DN{k}"), "nodes": ["neg", node]}));
                currents.push(format!("DP{k}"));
                currents.push(format!("DN{k}"));
            }
            if s.len() == 1 {
                // The other end of a single-phase bridge is gnd.
                c.push(json!({"type": "diode", "name": "DPG", "nodes": ["gnd", "pos"]}));
                c.push(json!({"type": "diode", "name": "DNG", "nodes": ["neg", "gnd"]}));
            }
            c.push(json!({"type": "resistor", "name": "RDC", "nodes": ["pos", "neg"], "resistance_ohm": 4.0}));
            c.push(json!({"type": "capacitor", "name": "CDC", "nodes": ["pos", "neg"], "capacitance_f": 2e-3}));
            // The DC side held by 100 ohm, not only by the blocking diodes:
            // the condition estimates of the two formulations, from which
            // the solver sets the shortest part of a split step and the
            // thresholds of the diode states, then stay small in both.
            // With 1e6 ohm the estimate of the dependent sources is 7.6
            // times that of the transformer (6.3e7 against 8.3e6): a
            // crossing 1.1e-8 s before a grid point is moved to it in one
            // run and not in the other (plan.md 6.14).
            c.push(json!({"type": "resistor", "name": "RG", "nodes": ["neg", "gnd"], "resistance_ohm": 100.0}));
            currents.push("RDC".into());
        }
    }
    (c, currents)
}

/// A model of version 0.10 with the switching instants within the steps.
fn case(load: Load, components: Vec<serde_json::Value>) -> serde_json::Value {
    let mut m = json!({
        "format": "openkeito-model", "version": "0.10",
        "simulation": {"time_step_s": H, "end_time_s": 0.04, "switching": "interpolated"},
        "components": components,
    });
    if let Load::SteadyState = load {
        m["simulation"]["initialization"] = json!("steady_state");
    }
    m
}

/// Sine sources behind R and L at the nodes `p` of side 0.
fn side_0(p: &[String]) -> Vec<serde_json::Value> {
    let amplitude = (2.0_f64 / 3.0).sqrt() * V0_KV * 1e3;
    let mut c = Vec::new();
    for (k, node) in p.iter().enumerate() {
        let (g, i) = (format!("g{k}"), format!("i{k}"));
        c.push(json!({"type": "voltage_source", "name": format!("E{k}"), "nodes": [g, "gnd"],
            "waveform": {"type": "sine", "amplitude": amplitude, "frequency_hz": F, "phase_deg": 20.0 - 120.0 * k as f64}}));
        c.push(json!({"type": "resistor", "name": format!("RG{k}"), "nodes": [g, i], "resistance_ohm": 0.4}));
        c.push(json!({"type": "inductor", "name": format!("LG{k}"), "nodes": [i, node], "inductance_h": 4e-3}));
    }
    c
}

#[test]
fn a_vcvs_and_a_cccs_per_phase_are_the_ideal_three_phase_transformer() {
    let phases = ["a", "b", "c"];
    let p: Vec<String> = phases.iter().map(|ph| format!("p.{ph}")).collect();
    let s: Vec<String> = phases.iter().map(|ph| format!("s.{ph}")).collect();
    for load in LOADS {
        let (loads, currents) = side_1(load, &s, Some("s"));
        let mut ideal = side_0(&p);
        ideal.push(
            json!({"type": "three_phase_ideal_transformer", "name": "T", "buses": ["p", "s"],
            "rated_voltage_kv": [V0_KV, V1_KV]}),
        );
        ideal.extend(loads.clone());
        let mut dependent = side_0(&p);
        for (k, ph) in phases.iter().enumerate() {
            dependent.extend(dependent_phase(ph, &p[k], &s[k]));
        }
        dependent.extend(loads);
        let (a, b) = (
            run_model(&case(load, ideal)),
            run_model(&case(load, dependent)),
        );
        let mut pairs: Vec<(String, String, f64)> = Vec::new();
        for node in p.iter().chain(&s) {
            pairs.push((format!("V({node})"), format!("V({node})"), 1.0));
        }
        let sources = ["LG0".to_string(), "LG1".into(), "LG2".into()];
        for name in currents.iter().chain(&sources) {
            pairs.push((format!("I({name})"), format!("I({name})"), 1.0));
        }
        for ph in phases {
            pairs.push((format!("I(T.0.{ph})"), format!("I(F{ph}.sense)"), 1.0));
            pairs.push((format!("I(T.1.{ph})"), format!("I(F{ph})"), -1.0));
        }
        let diodes = matches!(load, Load::Rectifier);
        same_to_rounding(&format!("three-phase, {load:?}"), &a, &b, &pairs, diodes);
    }
}

#[test]
fn a_vcvs_and_a_cccs_with_the_leakage_impedance_are_the_transformer() {
    // The transformer without magnetizing branch: the ideal transformer
    // with half the leakage impedance (R/2 + j X/2 per unit) on each side.
    let (s_va, z, r): (f64, f64, f64) = (0.1e6, 0.04, 0.01);
    let x = (z * z - r * r).sqrt();
    let w = 2.0 * PI * F;
    let base = |kv: f64| (kv * 1e3).powi(2) / s_va;
    let p = vec!["p".to_string()];
    let s = vec!["s".to_string()];
    for load in LOADS {
        let (loads, currents) = side_1(load, &s, None);
        let mut unit = side_0(&p);
        unit.push(json!({"type": "transformer", "name": "T", "windings": [["p", "gnd"], ["s", "gnd"]],
            "rated_voltage_kv": [V0_KV, V1_KV], "rated_power_mva": s_va / 1e6, "rated_frequency_hz": F,
            "impedance_percent": 100.0 * z, "resistance_percent": 100.0 * r}));
        unit.extend(loads.clone());
        let mut dependent = side_0(&p);
        for (side, node, inner, kv) in [(0, "p", "pm", V0_KV), (1, "s", "sm", V1_KV)] {
            let between = format!("{node}r");
            dependent.push(
                json!({"type": "resistor", "name": format!("ZR{side}"), "nodes": [node, between],
                "resistance_ohm": r / 2.0 * base(kv)}),
            );
            dependent.push(
                json!({"type": "inductor", "name": format!("ZL{side}"), "nodes": [between, inner],
                "inductance_h": x / 2.0 * base(kv) / w}),
            );
        }
        dependent.extend(dependent_phase("a", "pm", "sm"));
        dependent.extend(loads);
        let (a, b) = (
            run_model(&case(load, unit)),
            run_model(&case(load, dependent)),
        );
        let mut pairs: Vec<(String, String, f64)> = vec![
            ("V(p)".into(), "V(p)".into(), 1.0),
            ("V(s)".into(), "V(s)".into(), 1.0),
            ("I(T.0)".into(), "I(ZR0)".into(), 1.0),
            ("I(T.1)".into(), "I(ZR1)".into(), 1.0),
            ("I(LG0)".into(), "I(LG0)".into(), 1.0),
        ];
        for name in &currents {
            pairs.push((format!("I({name})"), format!("I({name})"), 1.0));
        }
        let diodes = matches!(load, Load::Rectifier);
        same_to_rounding(&format!("single-phase, {load:?}"), &a, &b, &pairs, diodes);
    }
}
