//! Three-phase loads and current injections against their definitions.
//!
//! Phasors are rms with a cosine reference. The phasor solution has no time
//! step and agrees to rounding (1e-12); the waveform, taken by a Fourier
//! transform over the last period, departs from it by the second-order
//! error of the method, below (w h)^2 (see three_phase.rs).

mod common;

use common::phasors::{check as close, Complex64, F, W};
use common::{run_model, Run};
use serde_json::{json, Value};

const H: f64 = 2e-5;

fn run(components: Value, outputs: Value) -> Run {
    let m = json!({
        "format": "openkeito-model", "version": "0.4",
        "simulation": {"time_step_s": H, "end_time_s": 0.04, "initialization": "steady_state"},
        "components": components, "outputs": outputs,
    });
    run_model(&m)
}

#[test]
fn a_load_takes_its_powers_at_the_rated_voltage() {
    // Behind a source impedance the bus voltage V differs from the rated
    // one, and a constant impedance takes (P + jQ) (sqrt(3) |V| / V_rated)^2.
    // (Directly across an ideal source, a capacitive load would show the
    // first-order current of 2S-DIRK for capacitors that a source dictates:
    // docs/numerical-method.md.)
    for (p, q) in [(5.0, 1.5), (0.0, -3.0), (2.0, 0.0)] {
        let r = run(
            json!([
                {"type": "three_phase_source", "name": "G1", "bus": "b", "line_voltage_rms_v": 6600.0,
                 "frequency_hz": F, "angle_deg": 25.0,
                 "impedance": {"type": "sequence", "r1_ohm": 0.1, "l1_h": 1.0 / W, "r0_ohm": 0.1, "l0_h": 1.0 / W}},
                {"type": "three_phase_load", "name": "LD", "bus": "b", "rated_voltage_kv": 6.6,
                 "rated_frequency_hz": F, "active_power_mw": p, "reactive_power_mvar": q}
            ]),
            json!([{"type": "bus_voltage", "bus": "b"}, {"type": "branch_current", "component": "LD"}]),
        );
        let rated = Complex64::new(p * 1e6, q * 1e6);
        let at = |v: Complex64| rated * (3.0 * v.norm_sqr() / 6600.0_f64.powi(2));
        for ph in ["a", "b", "c"] {
            let v = r.steady(&format!("V(b.{ph})"));
            let i = r.steady(&format!("I(LD.{ph})"));
            close(
                &format!("phasor S, phase {ph}, P {p} Q {q}"),
                3.0 * v * i.conj(),
                at(v),
                1e-12,
            );
        }
        let v = r.steady("V(b.a)");
        assert!(
            (v.norm() * 3.0_f64.sqrt() / 6600.0 - 1.0).abs() > 1e-4,
            "the bus voltage moved"
        );
        let (vw, iw) = (r.phasor("V(b.a)", 1), r.phasor("I(LD.a)", 1));
        close(
            &format!("waveform S, P {p} Q {q}"),
            3.0 * vw * iw.conj(),
            at(vw),
            (W * H).powi(2),
        );
    }
}

#[test]
fn a_current_source_injects_its_current() {
    // Into a resistive star of 10 ohm per phase: V = 10 I at the angle of I.
    let (i_rms, angle): (f64, f64) = (120.0, -35.0);
    let r = run(
        json!([
            {"type": "three_phase_current_source", "name": "PV", "bus": "b", "current_rms_a": i_rms,
             "frequency_hz": F, "angle_deg": angle},
            {"type": "three_phase_load", "name": "R", "bus": "b", "rated_voltage_kv": 1.0,
             "rated_frequency_hz": F, "active_power_mw": 0.1, "reactive_power_mvar": 0.0}
        ]),
        json!([{"type": "bus_voltage", "bus": "b"}, {"type": "branch_current", "component": "PV"}]),
    );
    // R = V^2 / P = 1e6 / 1e5 = 10 ohm.
    for (k, ph) in ["a", "b", "c"].iter().enumerate() {
        let shift = [0.0, -120.0, 120.0][k];
        let i = Complex64::from_polar(i_rms, (angle + shift).to_radians());
        close(
            &format!("I(PV.{ph})"),
            r.steady(&format!("I(PV.{ph})")),
            i,
            1e-12,
        );
        close(
            &format!("V(b.{ph})"),
            r.steady(&format!("V(b.{ph})")),
            10.0 * i,
            1e-12,
        );
    }
    let i = Complex64::from_polar(i_rms, angle.to_radians());
    close(
        "waveform V(b.a)",
        r.phasor("V(b.a)", 1),
        10.0 * i,
        (W * H).powi(2),
    );
    close(
        "waveform I(PV.a)",
        r.phasor("I(PV.a)", 1),
        i,
        (W * H).powi(2),
    );
}
