//! The thyristor against the textbook six-pulse phase-controlled converter
//! (Mohan, Undeland, Robbins, "Power Electronics", 3rd ed., 2003, ch. 6,
//! "Line-frequency phase-controlled rectifiers and inverters"): the average
//! DC voltage with commutation overlap,
//!
//!   U_d = (3 sqrt 2/pi) V_LL cos(alpha) - (3/pi) w L_s I_d,
//!
//! the overlap angle u from cos(alpha) - cos(alpha + u) = 2 w L_s I_d/(sqrt 2 V_LL),
//! the firing instants, the diode rectifier at alpha = 0 and the inverter at
//! alpha = 150 deg.

mod common;

use std::f64::consts::PI;

use common::*;
use serde_json::{json, Value};

const F: f64 = 50.0;
const V_LL: f64 = 400.0;
const LS: f64 = 1e-3;
const ID: f64 = 50.0;
const LD: f64 = 1.0;

fn w() -> f64 {
    2.0 * PI * F
}

/// How the six thyristors are fired.
#[derive(Clone, Copy)]
enum Gates {
    /// Pulses of 120 deg that start alpha after the natural commutation
    /// (the instant at which the voltage of the thyristor becomes positive).
    Pulses { alpha_deg: f64 },
    /// A gate signal that always fires: the thyristor is a diode.
    Always,
    /// Six diodes instead of the thyristors.
    Diodes,
}

/// The converter: a three-phase source (phase a is sqrt(2/3) V_LL sin(w t))
/// with the inductance `LS` per phase, six thyristors T1 to T6 numbered in
/// the order of firing, and on the DC side `rd`, the smoothing inductor `LD`
/// (starting at `ID`) and the source `e`, so that U(p) - U(n) =
/// rd I + LD dI/dt + e. Output: the DC current.
fn converter(h: f64, end: f64, rd: f64, e: f64, gates: Gates, switching: &str) -> Value {
    // Thyristor k (1..6) has its natural commutation at w t = 30 + 60 (k - 1)
    // deg: T1 (a to p) at 30, T2 (n to c) at 90, T3 (b to p) at 150, T4 (n
    // to a) at 210, T5 (c to p) at 270, T6 (n to b) at 330.
    let ends = [
        ("ac.a", "p"),
        ("n", "ac.c"),
        ("ac.b", "p"),
        ("n", "ac.a"),
        ("ac.c", "p"),
        ("n", "ac.b"),
    ];
    let mut components = vec![
        json!({"type": "three_phase_source", "name": "G1", "bus": "ac", "line_voltage_rms_v": V_LL,
        "frequency_hz": F, "angle_deg": -90.0,
        "impedance": {"type": "sequence", "r1_ohm": 0.0, "l1_h": LS, "r0_ohm": 0.0, "l0_h": LS}}),
    ];
    let mut controls = vec![json!({"type": "constant", "name": "on", "value": 1.0})];
    for (k, (a, c)) in ends.iter().enumerate() {
        let name = format!("T{}", k + 1);
        let natural = 30.0 + 60.0 * k as f64;
        // sin(w t + phase) > threshold for 2 acos(threshold) around the peak.
        let (start, width) = match gates {
            Gates::Pulses { alpha_deg } => (natural + alpha_deg, 120.0),
            Gates::Always => {
                components.push(
                    json!({"type": "thyristor", "name": name, "nodes": [a, c], "gate": "on"}),
                );
                continue;
            }
            Gates::Diodes => {
                components.push(json!({"type": "diode", "name": name, "nodes": [a, c]}));
                continue;
            }
        };
        let threshold = (width / 2.0_f64).to_radians().cos();
        let gate = format!("g{}", k + 1);
        let sine = format!("s{}", k + 1);
        let level = format!("l{}", k + 1);
        controls.push(
            json!({"type": "sine_wave", "name": sine, "amplitude": 1.0, "frequency_hz": F,
            "phase_deg": 90.0 - width / 2.0 - start}),
        );
        controls.push(json!({"type": "constant", "name": level, "value": threshold}));
        controls.push(json!({"type": "comparator", "name": gate, "inputs": [sine, level]}));
        components.push(json!({"type": "thyristor", "name": name, "nodes": [a, c], "gate": gate}));
    }
    components
        .push(json!({"type": "resistor", "name": "RD", "nodes": ["p", "m"], "resistance_ohm": rd}));
    components.push(json!({"type": "inductor", "name": "LD", "nodes": ["m", "q"], "inductance_h": LD, "initial_current_a": ID}));
    components.push(json!({"type": "voltage_source", "name": "E", "nodes": ["q", "n"], "waveform": {"type": "dc", "value": e}}));
    let mut model = json!({
        "format": "openkeito-model",
        "version": "0.9",
        "simulation": {"time_step_s": h, "end_time_s": end, "switching": switching},
        "components": components,
        "outputs": [{"type": "branch_current", "component": "LD"}],
    });
    match gates {
        Gates::Diodes => {}
        Gates::Always => model["controls"] = Value::Array(controls[..1].to_vec()),
        Gates::Pulses { .. } => model["controls"] = Value::Array(controls[1..].to_vec()),
    }
    model
}

/// The average DC voltage of the textbook at the load current `i`, with
/// the drop on two conducting thyristors of 1 mohm.
fn formula(alpha_deg: f64, i: f64) -> f64 {
    3.0 * 2f64.sqrt() / PI * V_LL * alpha_deg.to_radians().cos()
        - 3.0 / PI * w() * LS * i
        - 2.0 * RON * i
}

/// The overlap time of the textbook at the load current `i`.
fn overlap(alpha_deg: f64, i: f64) -> f64 {
    let a = alpha_deg.to_radians();
    ((a.cos() - 2.0 * w() * LS * i / (2f64.sqrt() * V_LL)).acos() - a) / w()
}

const T0: f64 = 0.04;
const T1: f64 = 0.06;

/// The average DC voltage over [T0, T1] from the current alone (see the
/// six-pulse diode rectifier in `switching.rs`): rd mean(i) + LD (i(T1) -
/// i(T0))/(T1 - T0) + e, with the mean current.
fn average(out: &Run, rd: f64, e: f64) -> (f64, f64) {
    let i = mean(out, 0, T0, T1);
    let k0 = (T0 / out.h).round() as usize;
    let k1 = (T1 / out.h).round() as usize;
    let di = out.rows[k1][0] - out.rows[k0][0];
    (rd * i + LD * di / (T1 - T0) + e, i)
}

/// The DC side for the firing angle `alpha_deg` that holds `ID`: a
/// resistance alone for a rectifier, 1 ohm and a source for an inverter.
fn load(alpha_deg: f64) -> (f64, f64) {
    let u = formula(alpha_deg, ID);
    if u > 0.0 {
        (u / ID, 0.0)
    } else {
        (1.0, u - ID)
    }
}

fn run(h: f64, alpha_deg: f64, switching: &str) -> (Run, f64, f64) {
    let (rd, e) = load(alpha_deg);
    let gates = Gates::Pulses { alpha_deg };
    let out = run_model(&converter(h, T1, rd, e, gates, switching));
    let (u, i) = average(&out, rd, e);
    (out, u, i)
}

/// The overlaps of the upper thyristors after T0: from the turn-on of one
/// to the turn-off of the one it takes over from.
fn overlaps(r: &Run) -> Vec<f64> {
    let upper = ["T1", "T3", "T5"];
    let events: Vec<_> = r
        .stats
        .events
        .iter()
        .filter(|e| e.time_s > T0 && upper.contains(&e.component.as_str()))
        .collect();
    events
        .windows(2)
        .filter(|p| p[0].action == "close" && p[1].action == "open")
        .map(|p| p[1].time_s - p[0].time_s)
        .collect()
}

#[test]
fn phase_control_has_the_textbook_average_overlap_and_firing_instants() {
    // The load holds the current: 1 H against the ripple of the DC voltage
    // (sixth harmonic; at alpha = 60 deg about 0.3 of 540 V) leaves
    // 160/(6 w LD) = 0.09 A peak to peak; the formula is taken at the mean
    // current, and the ripple changes the commutation voltage by its square
    // (below 1e-4 of U_d). The drop on a third thyristor during the overlap
    // is below 0.03 V; the leakage of the blocking ones (1e6 ohm) 0.5 mA.
    let h = 2e-5;
    for alpha in [15.0, 30.0, 60.0, 150.0] {
        let (out, u, i) = run(h, alpha, "interpolated");
        let f = formula(alpha, i);
        let o = overlaps(&out);
        let ou = overlap(alpha, i);
        let worst = o.iter().map(|x| (x - ou).abs()).fold(0.0, f64::max);
        // Firing: T1 turns on at (30 + alpha)/360 of a period after the
        // start of each period (the source has it forward-biased then).
        let period = 1.0 / F;
        let fire = (30.0 + alpha) / 360.0 * period;
        let firing: Vec<f64> = out
            .stats
            .events
            .iter()
            .filter(|e| e.component == "T1" && e.action == "close" && e.time_s > 0.5 * period)
            .map(|e| {
                let k = ((e.time_s - fire) / period).round();
                e.time_s - fire - k * period
            })
            .collect();
        let firing_error = firing.iter().map(|x| x.abs()).fold(0.0, f64::max);
        println!(
            "alpha {alpha:5.1}: U_d {u:9.4} V at {i:.4} A, formula {f:9.4} V ({:.2e} relative); overlap {ou:.4e} s, \
             largest deviation {worst:.2e} s over {}; firing of T1 off by {firing_error:.1e} s over {}; \
             {} instants within steps, {} split steps",
            (u - f).abs() / f.abs(),
            o.len(),
            firing.len(),
            out.stats.instants_within_steps,
            out.stats.split_steps,
        );
        assert!((u - f).abs() < 1e-3 * f.abs(), "alpha {alpha}: {u} vs {f}");
        assert_eq!(o.len(), 3, "alpha {alpha}: {o:?}");
        assert!(worst < 5e-7, "alpha {alpha}: {o:?} vs {ou}");
        assert!(!firing.is_empty());
        assert!(firing_error < 1e-9, "alpha {alpha}: {firing:?}");
        assert_eq!(out.stats.cycle_fallbacks, 0);
        assert_eq!(out.stats.instant_limit_steps, 0);
    }
}

#[test]
fn thyristors_that_are_always_fired_are_diodes() {
    // A thyristor whose gate always fires turns on at the zero crossing of
    // its voltage, as a diode: the firing angle is 0.
    let h = 2e-5;
    let (rd, e) = load(0.0);
    for switching in ["interpolated", "grid"] {
        let diodes = run_model(&converter(h, T1, rd, e, Gates::Diodes, switching));
        let always = run_model(&converter(h, T1, rd, e, Gates::Always, switching));
        let scale = diodes.rows.iter().map(|r| r[0].abs()).fold(0.0, f64::max);
        let largest = diodes
            .rows
            .iter()
            .zip(&always.rows)
            .map(|(a, b)| (a[0] - b[0]).abs())
            .fold(0.0, f64::max);
        println!(
            "{switching}: thyristors always fired against diodes: {:.2e} relative",
            largest / scale
        );
        assert!(largest <= 1e-9 * scale, "{switching}: {largest}");
        let times = |r: &Run| -> Vec<(String, &str)> {
            let mut v: Vec<_> = r
                .stats
                .events
                .iter()
                .map(|e| (format!("{:.12e} {}", e.time_s, e.component), e.action))
                .collect();
            v.sort();
            v
        };
        assert_eq!(times(&diodes), times(&always), "{switching}");
        // Fired by pulses that start at the natural commutation itself
        // (alpha = 0), the instant of the gate and the zero crossing of the
        // voltage coincide. At t = 0 only the two fired thyristors carry the
        // load current, not all six diodes (see the events), so the load
        // current takes another way to its mean; the averages agree with
        // the formula and with each other to 1e-4.
        let (out, u, i) = run(h, 0.0, switching);
        let (ud, id) = average(&diodes, rd, e);
        println!(
            "{switching}: alpha = 0: U_d {u:.4} V at {i:.4} A (formula {:.4} V) against the diodes {ud:.4} V at {id:.4} A (formula {:.4} V): {:.1e} relative; {} events",
            formula(0.0, i),
            formula(0.0, id),
            (u - ud).abs() / ud,
            out.stats.events.len()
        );
        if switching == "interpolated" {
            assert!((u - formula(0.0, i)).abs() < 1e-3 * u);
            assert!((u - ud).abs() < 3e-4 * ud);
        }
    }
}

#[test]
fn at_grid_points_the_firing_waits_for_the_next_grid_point() {
    // With switching at grid points the firing moves to the first grid
    // point at which the gate signal is >= 0.5, later by a fraction of a
    // step that the phase of the firing against the grid decides: the error
    // of the average is of first order in h.
    let alpha = 30.0;
    let mut errors = Vec::new();
    for h in [4e-5, 2e-5, 1e-5, 5e-6] {
        let (_, u, i) = run(h, alpha, "grid");
        let (_, ui, ii) = run(h, alpha, "interpolated");
        let (e, ei) = (u - formula(alpha, i), ui - formula(alpha, ii));
        println!(
            "h {:4.0} us: grid {e:+.4} V, interpolated {ei:+.4} V",
            h * 1e6
        );
        errors.push((h, e));
        // Later firing lowers the average.
        assert!(e < 0.0);
        assert!(ei.abs() < e.abs());
    }
    // A delay of d h costs (3 sqrt 2/pi) V_LL sin(alpha) w d h.
    let slope = 3.0 * 2f64.sqrt() / PI * V_LL * alpha.to_radians().sin() * w();
    for (h, e) in &errors {
        let d = -e / (slope * h);
        println!("h {:4.0} us: mean delay {d:.2} of a step", h * 1e6);
        assert!(d > 0.1 && d < 1.0, "{d}");
    }
}

#[test]
fn a_thyristor_needs_version_0_9_and_its_gate() {
    let mut model = converter(
        2e-5,
        0.001,
        10.0,
        0.0,
        Gates::Pulses { alpha_deg: 30.0 },
        "grid",
    );
    model["version"] = "0.8".into();
    model["simulation"]
        .as_object_mut()
        .unwrap()
        .remove("switching");
    let d = diagnostics(&model);
    assert!(
        d.iter()
            .any(|d| d.code == "E102" && d.message.contains("thyristor")),
        "{d:#?}"
    );
    let mut model = converter(
        2e-5,
        0.001,
        10.0,
        0.0,
        Gates::Pulses { alpha_deg: 30.0 },
        "grid",
    );
    model["components"][1]["gate"] = "missing".into();
    let d = diagnostics(&model);
    assert!(
        d.iter()
            .any(|d| d.severity == openkeito_model::Severity::Error),
        "{d:#?}"
    );
}

/// A thyristor on a sine source (100 V peak, 50 Hz) and a resistance of 10
/// ohm, fired by a gate signal that is 1 from `on` to `off` (from a ramp of
/// the time and two comparators, so that the instants lie within steps).
/// The voltage of the thyristor becomes positive at 20 ms.
fn single(h: f64, method: &str, on: f64, off: f64) -> Value {
    json!({
        "format": "openkeito-model",
        "version": "0.9",
        "simulation": {"time_step_s": h, "end_time_s": 0.025, "method": method,
            "switching": "interpolated"},
        "components": [
            {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"],
             "waveform": {"type": "sine", "amplitude": 100.0, "frequency_hz": 50.0}},
            {"type": "thyristor", "name": "T1", "nodes": ["n1", "n2"], "gate": "g"},
            {"type": "resistor", "name": "R1", "nodes": ["n2", "gnd"], "resistance_ohm": 10.0},
        ],
        "controls": [
            {"type": "ramp", "name": "t", "slope_per_s": 1.0},
            {"type": "constant", "name": "t_on", "value": on},
            {"type": "constant", "name": "t_off", "value": off},
            {"type": "comparator", "name": "after_on", "inputs": ["t", "t_on"]},
            {"type": "comparator", "name": "before_off", "inputs": ["t_off", "t"]},
            {"type": "product", "name": "g", "inputs": ["after_on", "before_off"]},
        ],
        "outputs": [{"type": "branch_current", "component": "R1"}],
    })
}

fn closings(r: &Run) -> Vec<f64> {
    r.stats
        .events
        .iter()
        .filter(|e| e.component == "T1" && e.action == "close")
        .map(|e| e.time_s)
        .collect()
}

#[test]
fn a_thyristor_fired_while_reverse_biased_turns_on_at_the_zero_crossing() {
    // The gate starts firing at 19.981 ms, within the step [19.98, 20.01]
    // ms of 30 us, while the voltage is negative; the voltage becomes
    // positive at 20 ms in the same step. The zero crossing is placed
    // between the value at the firing instant and the value at the end of
    // the part: with one stage (trapezoidal rule) as with two. The same
    // holds when the gate starts firing so close before the grid point
    // 19.98 ms (10 ps) that the grid point takes it.
    for on in [19.981e-3, 19.98e-3 - 1e-11] {
        for method in ["trapezoidal", "2s-dirk"] {
            let out = run_model(&single(3e-5, method, on, 20.5e-3));
            let c = closings(&out);
            println!("gate from {on:e} s, {method}: T1 closes at {c:?}");
            assert_eq!(c.len(), 1, "{on:e}, {method}: {c:?}");
            assert!((c[0] - 0.02).abs() < 1e-9, "{on:e}, {method}: {c:?}");
        }
    }
}

#[test]
fn a_thyristor_whose_gate_stops_before_its_voltage_is_positive_keeps_blocking() {
    // The gate fires from 19.981 to 19.9999 ms only, while the voltage is
    // negative: the thyristor does not turn on, neither at 20 ms nor later.
    for method in ["trapezoidal", "2s-dirk"] {
        let out = run_model(&single(3e-5, method, 19.981e-3, 19.9999e-3));
        let c = closings(&out);
        assert!(c.is_empty(), "{method}: {c:?}");
        let largest = out.rows.iter().map(|r| r[0].abs()).fold(0.0, f64::max);
        // The leakage through 1e6 ohm: 1e-4 A at most.
        assert!(largest < 1.1e-4, "{method}: {largest}");
    }
}
