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
