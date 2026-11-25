//! Switching instants within time steps (`simulation.switching` =
//! "interpolated") against solutions for ideal switching instants: the exact
//! solution of the buck converter and of the diode bridge, the textbook
//! average of the six-pulse rectifier, and the closed forms of circuits with
//! time-controlled switches. With switching at grid points an instant moves
//! by up to one time step, an error of first order; here the instants are
//! placed within the step and the error is that of the integration method,
//! second order.

mod common;

use common::*;
use serde_json::{json, Value};

/// Largest deviation of i_L and v_C from the exact solution for ideal
/// instants.
fn buck_error(
    b: &Buck,
    method: &str,
    h: f64,
    end: f64,
    x0: [f64; 2],
    switching: &str,
) -> (f64, f64, Run) {
    let out = run_with(&b.model(method, h, end, x0), switching);
    let exact = b.ideal(h, out.t.len() - 1, x0);
    let (mut ei, mut ev): (f64, f64) = (0.0, 0.0);
    for (row, x) in out.rows.iter().zip(&exact) {
        ei = ei.max((row[0] - x[0]).abs());
        ev = ev.max((row[1] - x[1]).abs());
    }
    (ei, ev, out)
}

#[test]
fn buck_in_continuous_conduction_follows_the_ideal_instants_to_second_order() {
    // The duty 0.405 puts the comparator crossings at 20.25 and 79.75 us of
    // every 100 us period: between grid points for every step used here.
    let b = &Buck { duty: 0.405, ..CCM };
    let x0 = [2.89, 40.5];
    let end = 2e-3;
    // 2S-DIRK: the local errors C3 h^3 |i_L'''| summed over the steps, with
    // |i_L'''| <= (E/L)/(L C), as in `power_electronics.rs`. Nothing is added
    // for the switching: every part of a split step is a step of the method.
    let di3 = (b.e / b.l) / (b.l * b.c);
    let dirk_bound = |h: f64| end / h * dirk_error_constant() * h.powi(3) * di3;
    // Trapezoidal rule: the part after each of the 40 gate instants is two
    // backward-Euler half steps, local error (h^2/4) |i_L''| at most, with
    // |i_L''| = |i_C|/(L C) <= 1.2 A/(L C).
    let be_bound = |h: f64| 40.0 * h * h / 4.0 * 1.2 / (b.l * b.c);
    for method in ["2s-dirk", "trapezoidal"] {
        let bound = |h: f64| {
            dirk_bound(h)
                + if method == "trapezoidal" {
                    be_bound(h)
                } else {
                    0.0
                }
        };
        let (h1, h2) = (2e-6, 1e-6);
        let (ei1, ev1, out) = buck_error(b, method, h1, end, x0, "interpolated");
        let (ei2, ev2, _) = buck_error(b, method, h2, end, x0, "interpolated");
        let (eg, _, grid) = buck_error(b, method, h2, end, x0, "grid");
        println!(
            "{method}: i_L error {ei1:.3e} -> {ei2:.3e} A (bounds {:.1e}, {:.1e}), v_C error {ev1:.3e} -> {ev2:.3e} V; \
             at grid points {eg:.3e} A; {} instants within steps, {} factorizations ({} at grid points)",
            bound(h1),
            bound(h2),
            out.stats.instants_within_steps,
            out.stats.factorizations,
            grid.stats.factorizations
        );
        assert!(ei1 < bound(h1) && ei2 < bound(h2), "{method}: {ei1}, {ei2}");
        // Second order, across the gate instants too. Under the trapezoidal
        // rule the backward-Euler half steps cover what is left of the step
        // after an instant, (1 - theta) h, and the place theta of the
        // instant within its step changes with h (0.125 and 0.875 at 2 us,
        // 0.25 and 0.75 at 1 us): the ratio is between 4 and
        // 4 (0.875/0.75)^2 (0.875/0.25)^2.
        if method == "2s-dirk" {
            assert!(
                (3.5..4.5).contains(&(ei1 / ei2)),
                "{method}: ratio {}",
                ei1 / ei2
            );
            assert!(
                (3.5..4.5).contains(&(ev1 / ev2)),
                "{method}: ratio {}",
                ev1 / ev2
            );
        } else {
            assert!(
                ei1 / ei2 > 3.5 && ev1 / ev2 > 3.5,
                "{method}: {}",
                ei1 / ei2
            );
        }
        // With switching at grid points the on-time is 41 steps instead of
        // 40.5: the current is off by (E/L) h/2 after every period and the
        // error grows to several times that within the run.
        assert!(eg > 0.5 * (b.e / b.l) * h2, "{method}: {eg}");
        assert!(ei2 < 1e-3 * eg, "{method}: {ei2} vs {eg}");
        // Two gate instants per period, each one instant within a step. The
        // diode follows the switch at the same instant.
        assert_eq!(out.stats.instants_within_steps, 40);
        assert_eq!(out.stats.instant_limit_steps, 0);
        assert_eq!(out.stats.cycle_fallbacks, 0);
        assert!(out.stats.max_resolves_per_step <= 2);
    }
}

#[test]
fn a_ten_times_wider_step_keeps_the_accuracy_of_switching_at_grid_points() {
    // What the interpolation is for: the same or a better accuracy with a
    // wider step. At 10 us (10 steps per carrier period) the error of the
    // split steps is the integration error, C3 h^2 (end) |i_L'''| = 8e-3 A at
    // most; at grid points with 1 us it is the switching-time error of
    // first order, about 0.2 A.
    let b = &Buck { duty: 0.405, ..CCM };
    let x0 = [2.89, 40.5];
    let end = 2e-3;
    let (wide, _, _) = buck_error(b, "2s-dirk", 1e-5, end, x0, "interpolated");
    let (narrow, _, _) = buck_error(b, "2s-dirk", 1e-6, end, x0, "grid");
    println!("i_L error: {wide:.3e} A at 10 us with instants within steps, {narrow:.3e} A at 1 us at grid points");
    assert!(wide < end / 1e-5 * dirk_error_constant() * 1e-15 * (b.e / b.l) / (b.l * b.c));
    assert!(wide < narrow / 100.0);
}

#[test]
fn the_duty_is_the_reference_and_the_average_output_converges_with_second_order() {
    let b = &Buck { duty: 0.405, ..CCM };
    let x0 = [2.89, 40.5];
    let end = 0.02;
    let (t0, t1) = (0.019, 0.02);
    let mut errors = Vec::new();
    for h in [2e-5, 1e-5, 5e-6] {
        let out = run_with(&b.model("2s-dirk", h, end, x0), "interpolated");
        let grid = run_with(&b.model("2s-dirk", h, end, x0), "grid");
        let exact = b.ideal(h, out.t.len() - 1, x0);
        let k0 = (t0 / h).round() as usize;
        let k1 = (t1 / h).round() as usize;
        let v_exact = exact[k0..k1].iter().map(|x| x[1]).sum::<f64>() / (k1 - k0) as f64;
        let v = mean(&out, 1, t0, t1);
        let vg = mean(&grid, 1, t0, t1);
        let d = on_time_fraction(&out.stats, "S1", t0, t1);
        let dg = on_time_fraction(&grid.stats, "S1", t0, t1);
        println!(
            "h = {h:e}: duty {d:.9} (at grid points {dg}), average {v:.7} V, exact {v_exact:.7} V, \
             difference {:+.3e} V (at grid points {:+.3e} V, E h f = {:.2e} V)",
            v - v_exact,
            vg - v_exact,
            b.e * h * b.f
        );
        // The gate instants are the crossings of reference and carrier, found
        // by bisection to 2^-40 of a step: the on-time over 10 periods is
        // exact to rounding.
        assert!((d - b.duty).abs() < 1e-9, "duty {d}");
        // At grid points the duty is quantized to h f and the average moves
        // by up to E h f; here it does, by more than half of that.
        assert!((dg - b.duty).abs() > 0.2 * h * b.f);
        assert!((vg - v_exact).abs() > 0.2 * b.e * h * b.f);
        // What is left is the integration error, three orders below E h f.
        assert!(
            (v - v_exact).abs() < 1e-3 * b.e * h * b.f,
            "{v} vs {v_exact}"
        );
        errors.push((v - v_exact).abs());
    }
    // It falls with h^2 (the first-order error E h f would halve).
    for k in 1..errors.len() {
        let ratio = errors[k - 1] / errors[k];
        assert!((3.0..5.5).contains(&ratio), "ratio {ratio}");
    }
}

#[test]
fn buck_in_discontinuous_conduction_has_no_reverse_current_after_the_turn_off() {
    let b = &DCM;
    let x0 = [0.0, 80.0];
    let end = 0.01;
    let periods = end * b.f;
    // The blocking resistances of S1 (to 100 V) and D1 (to gnd) carry
    // (v_out - E/2)/(R_off/2) through the inductor while both are off: the
    // only negative current of the ideal solution.
    let leak = (80.2 - b.e / 2.0) / (ROFF / 2.0);
    let mut errors = Vec::new();
    for h in [2e-6, 1e-6] {
        let (ei, ev, out) = buck_error(b, "2s-dirk", h, end, x0, "interpolated");
        let (eg, _, grid) = buck_error(b, "2s-dirk", h, end, x0, "grid");
        let lowest = out.rows.iter().map(|r| r[0]).fold(f64::MAX, f64::min);
        let lowest_grid = grid.rows.iter().map(|r| r[0]).fold(f64::MAX, f64::min);
        println!(
            "DCM h = {h:e}: i_L error {ei:.3e} A (at grid points {eg:.3e} A), v_C error {ev:.3e} V, \
             lowest i_L {lowest:.3e} A (at grid points {lowest_grid:.3e} A); {} re-solutions, {} instants",
            out.stats.resolves, out.stats.instants_within_steps
        );
        // At grid points the diode opens up to one step late: the row after
        // shows up to (v/L) h of reverse current (first order). Here it
        // opens at the zero crossing, which the current reaches on a
        // straight line: nothing but the leak is left.
        assert!(lowest_grid < -0.5 * 80.0 / b.l * h);
        assert!(lowest > -1.05 * leak, "{lowest}");
        // The integration error over the run: C3 h^2 (end) |i_L'''| with
        // |i_L'''| <= (v/L)/(L C) = 3.2e13 A/s^3.
        let bound = end / h * dirk_error_constant() * h.powi(3) * (80.0 / b.l) / (b.l * b.c);
        assert!(ei < bound, "{ei} vs {bound}");
        assert!(ei < 1e-4 * eg);
        // Three instants per period (two gate crossings, one current zero)
        // and the settling solutions of the gate instants: no chattering.
        assert_eq!(out.stats.instants_within_steps as f64, 3.0 * periods);
        assert!(out.stats.resolves as f64 <= 5.0 * periods);
        assert!(out.stats.max_resolves_per_step <= 2);
        assert_eq!(out.stats.cycle_fallbacks, 0);
        assert_eq!(out.stats.instant_limit_steps, 0);
        errors.push(ei);
    }
    let ratio = errors[0] / errors[1];
    assert!((3.5..4.5).contains(&ratio), "second order: ratio {ratio}");
}

#[test]
fn bridge_rectifier_changes_at_the_zero_crossing_of_the_source() {
    let (r, h) = (10.0, 2e-5);
    let vp = V_RMS * 2f64.sqrt();
    let w = 2.0 * std::f64::consts::PI * F_AC;
    // The source starts 6.1 us after a zero crossing: no crossing on a grid
    // point.
    let shift = 6.1e-6;
    let mut m = bridge(h, 0.04, r, None, RON);
    m["components"][0]["waveform"]["phase_deg"] = json!(360.0 * F_AC * shift);
    let (g, gl, goff) = (1.0 / RON, 1.0 / r, 1.0 / ROFF);
    let ideal = |t: f64| {
        // p = x and n = |v_s| - x by symmetry; KCL at p with two conducting
        // and two blocking diodes.
        let vs = (vp * (w * (t + shift)).sin()).abs();
        2.0 * vs * (g + gl) / (g + goff + 2.0 * gl) - vs
    };
    let error = |out: &Run| {
        out.t
            .iter()
            .zip(&out.rows)
            .skip(1)
            .map(|(t, row)| (row[0] - ideal(*t)).abs())
            .fold(0.0, f64::max)
    };
    let out = run_with(&m, "interpolated");
    let grid = run_with(&m, "grid");
    let (e, eg) = (error(&out), error(&grid));
    println!(
        "R load: deviation from the ideal rectifier {e:.2e} V (at grid points {eg:.2e} V); {} events, {} instants",
        out.stats.events.len(),
        out.stats.instants_within_steps
    );
    // No states: only rounding is left (values up to 141 V). At grid points
    // the row after a crossing still has the old states, up to
    // 2 w Vp h = 1.8 V off.
    assert!(e < 1e-9, "{e}");
    assert!(eg > 0.1);
    // The four diodes change together at each of the four crossings within
    // the run (0.04 s; the last one 6.1 us before its end).
    assert_eq!(out.stats.instants_within_steps, 4);
    assert_eq!(out.stats.events.len(), 2 + 4 * 4);
    for e in out.stats.events.iter().skip(2) {
        let crossing = ((e.time_s + shift) * 2.0 * F_AC).round() / (2.0 * F_AC) - shift;
        // Linear interpolation of a current that is a sine: the crossing is
        // off by at most (1/8) h^2 w^2 / w of time, 1.6e-10 s.
        assert!(
            (e.time_s - crossing).abs() < h * h * w / 8.0 + 1e-12,
            "{} at {}",
            e.component,
            e.time_s
        );
    }
    assert!(out.stats.max_resolves_per_step <= 2);
    assert_eq!(out.stats.cycle_fallbacks, 0);
}

#[test]
fn a_diode_that_is_opened_too_early_leaves_one_event() {
    // The bridge of `examples/rectifier.json` (on-resistance 1 mohm) at
    // h = 50 us. The zero of the diode current comes w h/(2 sqrt 2) early
    // under 2S-DIRK (see the next test): 7 us before the ideal instant the
    // diodes are opened, the trial solution over the next 1.7 us agrees
    // (0.07 mV of forward voltage, below the threshold of 0.14 mV), and the
    // rest of the step then shows 0.17 mV from its start: the diodes are
    // closed again at the same instant and opened 11 us later, for good.
    // The events used to list an opening and a closing at one instant.
    let (r, c, ron, h) = (100.0, 1e-3, 1e-3, 5e-5);
    let w = 2.0 * std::f64::consts::PI * F_AC;
    let (off, _, _) = capacitor_input_angles(w, r, c);
    let out = run_with(&bridge(h, 0.1, r, Some(c), ron), "interpolated");
    let events = &out.stats.events;
    for (k, e) in events.iter().enumerate() {
        let again = events[..k]
            .iter()
            .find(|o| o.time_s == e.time_s && o.component == e.component);
        assert!(
            again.is_none(),
            "{} changes twice at t = {:e} s: {} after {}",
            e.component,
            e.time_s,
            e.action,
            again.map_or("", |o| o.action)
        );
    }
    // Every diode closes and opens once per period, in turn.
    for d in ["D1", "D2", "D3", "D4"] {
        let actions: Vec<&str> = events
            .iter()
            .filter(|e| e.component == d)
            .map(|e| e.action)
            .collect();
        assert_eq!(actions.len(), 10, "{d}: {actions:?}");
        assert!(actions.chunks(2).all(|p| p == ["close", "open"]), "{d}");
    }
    // The opening that is left is the later one, within the bound of the
    // next test of the ideal angle.
    let bound = w * h / (2.0 * 2f64.sqrt()) + 4e-5;
    for e in events.iter().filter(|e| e.action == "open") {
        let angle = (w * e.time_s) % std::f64::consts::PI;
        assert!(
            (angle - off).abs() < bound,
            "{} at {angle} vs {off}",
            e.component
        );
    }
    assert_eq!(out.stats.cycle_fallbacks, 0);
    assert!(out.stats.warnings.is_empty(), "{:?}", out.stats.warnings);
}

#[test]
fn capacitor_input_rectifier_turns_on_and_off_at_the_ideal_angles() {
    let (r, c, ron) = (100.0, 1e-3, 1e-5);
    let w = 2.0 * std::f64::consts::PI * F_AC;
    let (off, on, _) = capacitor_input_angles(w, r, c);
    for (method, h) in [("2s-dirk", 5e-6), ("2s-dirk", 2e-5), ("trapezoidal", 2e-5)] {
        let mut m = bridge(h, 0.1, r, Some(c), ron);
        m["simulation"]["method"] = method.into();
        let out = run_with(&m, "interpolated");
        let last = 0.08;
        let of = |action: &str| -> Vec<f64> {
            out.stats
                .events
                .iter()
                .filter(|e| e.time_s >= last && e.action == action)
                .filter(|e| e.component == "D1" || e.component == "D2")
                .map(|e| (w * e.time_s) % std::f64::consts::PI)
                .collect()
        };
        let (opens, closes) = (of("open"), of("close"));
        println!(
            "C input, {method}, h = {h:e}: turn-off {:?} rad, turn-on {:?} rad from the ideal angles (w h = {:.2e})",
            opens.iter().map(|a| format!("{:+.3e}", a - off)).collect::<Vec<_>>(),
            closes.iter().map(|a| format!("{:+.3e}", a - on)).collect::<Vec<_>>(),
            w * h
        );
        assert_eq!((opens.len(), closes.len()), (2, 2));
        // Turn-on: the source reaches the capacitor voltage, both known to
        // second order. The blocking diodes leak, which speeds the decay of
        // the capacitor by R/R_off = 1e-4 of the droop of 11.7 V: the
        // crossing comes 1.2e-3 V/(Vp w cos(on)) = 2e-5 rad early.
        for a in &closes {
            assert!((a - on).abs() < 4e-5, "{method}: turn-on {a} vs {on}");
        }
        // Turn-off: the zero of the diode current, which here is C dv/dt of
        // a voltage that the source dictates. 2S-DIRK gives such a current
        // to first order only (numerical-method, "Currents that a source
        // dictates"): it is h i'/(2 sqrt 2) early, and so is its zero, by
        // w h/(2 sqrt 2). The trapezoidal rule gives it to second order, but
        // keeps what the two backward-Euler half steps after the turn-on
        // leave in this current ((h/4) i' at most) as an oscillation from
        // step to step, which moves the zero by less than that.
        let bound = w * h / (2.0 * 2f64.sqrt()) + 4e-5;
        for a in &opens {
            assert!(
                (a - off).abs() < bound,
                "{method}: turn-off {a} vs {off}, bound {bound}"
            );
        }
        assert!(out.stats.max_resolves_per_step <= 2);
        assert_eq!(out.stats.cycle_fallbacks, 0);
        assert_eq!(out.stats.instant_limit_steps, 0);
    }
}

#[test]
fn pi_controlled_buck_has_no_limit_cycle_of_the_quantized_duty() {
    let b = &CCM;
    let h = 2e-6;
    let model = |switching: &str| {
        let mut m = b.model("2s-dirk", h, 0.15, [0.0, 0.0]);
        m["controls"] = json!([
            {"type": "constant", "name": "v_ref", "value": 30.0},
            {"type": "measurement", "name": "v_out", "quantity": {"type": "node_voltage", "node": "out"}},
            {"type": "sum", "name": "error", "inputs": ["v_ref", "v_out"], "signs": [1.0, -1.0]},
            {"type": "pi", "name": "duty", "input": "error", "proportional_gain": 0.001,
             "integral_gain_per_s": 3.0, "output_min": 0.0, "output_max": 0.95},
            {"type": "triangle_wave", "name": "carrier", "frequency_hz": b.f},
            {"type": "comparator", "name": "pwm", "inputs": ["duty", "carrier"]},
        ]);
        m["outputs"] = json!([
            {"type": "node_voltage", "node": "out"},
            {"type": "signal", "signal": "duty"},
        ]);
        with_switching(m, switching)
    };
    let out = run_with(&model("interpolated"), "interpolated");
    let (t0, t1) = (0.1, 0.15);
    let v = mean(&out, 0, t0, t1);
    let d = mean(&out, 1, t0, t1);
    // Averages over single carrier periods.
    let per = (1.0 / (b.f * h)).round() as usize;
    let mut k = (t0 / h).round() as usize;
    let (mut lo, mut hi) = (f64::MAX, f64::MIN);
    while k + per < out.rows.len() {
        let a = out.rows[k..k + per].iter().map(|r| r[0]).sum::<f64>() / per as f64;
        lo = lo.min(a);
        hi = hi.max(a);
        k += per;
    }
    let realized = on_time_fraction(&out.stats, "S1", t0, t1);
    println!("PI: average v_out {v:.6} V, duty signal {d:.6}, on-time {realized:.6}, period averages {lo:.5}..{hi:.5} V");
    // With switching at grid points the comparator realizes whole steps
    // only (15 or 16 of 50 near D = 0.3), and the integrator moves the duty
    // across that boundary for ever (`power_electronics.rs`: levels 2 V
    // apart at 1 us). Here the on-time follows the duty signal
    // continuously, and the loop settles: after 30 time constants of
    // 1/(E K_i) = 3.3 ms the period averages agree to 1e-4 V.
    assert!((v - 30.0).abs() < 1e-4, "{v}");
    assert!(hi - lo < 1e-4, "{lo}..{hi}");
    // The average of v_sw is D E less the drop on the conducting switch or
    // diode (R_on times the load current of 3 A): the on-time is
    // 30/100 + 3 A * 1 mohm / 100 V.
    assert!((realized - 0.30003).abs() < 2e-6, "{realized}");
    // The duty signal is sampled at grid points and held for a step, so the
    // edge it gives is that of the value one step before at most: with a
    // ripple of the duty signal of K_p * 0.12 V the two differ by 1e-4.
    assert!((d - realized).abs() < 2e-4, "{d} vs {realized}");
}

// ---------------------------------------------------------------------------
// Time-controlled elements.

fn rl_model(method: &str, h: f64, end: f64, switching: &str, components: Value) -> Value {
    with_switching(
        json!({
            "format": "openkeito-model",
            "version": "0.2",
            "simulation": {"time_step_s": h, "end_time_s": end, "method": method},
            "components": components,
            "outputs": [{"type": "branch_current", "component": "L1"}],
        }),
        switching,
    )
}

#[test]
fn a_switch_and_a_step_source_act_at_their_times_within_a_step() {
    // 10 V on R = 2 ohm and L = 10 mH from t_c, which is 0.37 of a step
    // after a grid point: i = 5 (1 - exp(-200 (t - t_c))).
    let (h, tc) = (1e-5, 1.2337e-3);
    let exact = |t: f64| {
        if t < tc {
            0.0
        } else {
            5.0 * (1.0 - (-200.0 * (t - tc)).exp())
        }
    };
    let by_switch = json!([
        {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {"type": "dc", "value": 10.0}},
        {"type": "switch", "name": "SW1", "nodes": ["n1", "n2"], "close_time_s": tc},
        {"type": "resistor", "name": "R1", "nodes": ["n2", "n3"], "resistance_ohm": 2.0},
        {"type": "inductor", "name": "L1", "nodes": ["n3", "gnd"], "inductance_h": 0.01},
    ]);
    let by_step = json!([
        {"type": "voltage_source", "name": "E1", "nodes": ["n2", "gnd"], "waveform": {"type": "step", "time_s": tc, "before": 0.0, "after": 10.0}},
        {"type": "resistor", "name": "R1", "nodes": ["n2", "n3"], "resistance_ohm": 2.0},
        {"type": "inductor", "name": "L1", "nodes": ["n3", "gnd"], "inductance_h": 0.01},
    ]);
    for (name, components) in [("switch", by_switch), ("step", by_step)] {
        for method in ["2s-dirk", "trapezoidal"] {
            let error = |switching: &str| {
                let out = run_model(&rl_model(method, h, 0.02, switching, components.clone()));
                let e = out
                    .t
                    .iter()
                    .zip(&out.rows)
                    .map(|(t, r)| (r[0] - exact(*t)).abs())
                    .fold(0.0, f64::max);
                (e, out)
            };
            let (e, out) = error("interpolated");
            let (eg, grid) = error("grid");
            println!("{name}, {method}: current error {e:.3e} A (at grid points {eg:.3e} A)");
            // The action is at its time, not at the grid point after it.
            assert_eq!(out.stats.events.len(), 1);
            assert_eq!(out.stats.events[0].time_s, tc);
            assert!((grid.stats.events[0].time_s - 1.24e-3).abs() < 1e-12);
            // At the grid point the response starts 0.63 h late:
            // i' (0.63 h) = 1000 A/s * 6.3 us. Here the error is that of the
            // method for the exponential, (1/12)(h/tau)^2 of 5 A/e at most
            // for the trapezoidal rule (6e-7 A), plus for it the two
            // backward-Euler half steps after the action, (h^2/4) i'' = 5e-6 A.
            assert!(eg > 4e-3, "{eg}");
            assert!(e < 1e-5, "{name}, {method}: {e}");
        }
    }
}

#[test]
fn a_contact_opens_at_its_current_zero_within_a_step() {
    // Sine source on R = 1 ohm and L = 10 mH, closed from t = 0:
    // i = (Vp/|Z|) (sin(w t - phi) + sin(phi) exp(-t R/L)).
    let h = 2e-5;
    let w = 2.0 * std::f64::consts::PI * 50.0;
    let (r, l, vp) = (1.0, 0.01, 100.0);
    let z = (r * r + w * w * l * l).sqrt();
    let phi = (w * l / r).atan();
    let exact = |t: f64| vp / z * ((w * t - phi).sin() + phi.sin() * (-t * r / l).exp());
    let command = 0.0123;
    // The first zero of the exact current after the command.
    let (mut lo, mut hi) = (command, command + 0.011);
    assert!(exact(lo) * exact(hi) < 0.0);
    for _ in 0..80 {
        let mid = 0.5 * (lo + hi);
        if exact(lo) * exact(mid) <= 0.0 {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    let zero = lo;
    for method in ["2s-dirk", "trapezoidal"] {
        let components = json!([
            {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {"type": "sine", "amplitude": vp, "frequency_hz": 50.0}},
            {"type": "switch", "name": "SW1", "nodes": ["n1", "n2"], "initially_closed": true, "open_time_s": command},
            {"type": "resistor", "name": "R1", "nodes": ["n2", "n3"], "resistance_ohm": r},
            {"type": "inductor", "name": "L1", "nodes": ["n3", "gnd"], "inductance_h": l},
        ]);
        let out = run_model(&rl_model(
            method,
            h,
            0.03,
            "interpolated",
            components.clone(),
        ));
        let grid = run_model(&rl_model(method, h, 0.03, "grid", components));
        assert_eq!(out.stats.events.len(), 1, "{method}");
        let e = &out.stats.events[0];
        let chopped = e.interrupted_current_a.unwrap();
        let chopped_grid = grid.stats.events[0].interrupted_current_a.unwrap();
        println!(
            "{method}: opens at {:.9} s (exact zero {zero:.9} s, at grid points {:.9} s), interrupts {chopped:.3e} A (at grid points {chopped_grid:.3e} A)",
            e.time_s, grid.stats.events[0].time_s
        );
        assert_eq!(e.action, "open");
        // The zero is placed by linear interpolation between two points of
        // a sine at most h apart: (1/8) h^2 w^2 Ip of current, or h^2 w/8 of
        // time (1.6e-8 s). The current itself is off by the error of the
        // method, 1e-5 of its peak at most, or 1e-5/w of time.
        let ip = vp / z;
        assert!(
            (e.time_s - zero).abs() < h * h * w / 8.0 + 1e-5 / w,
            "{method}: {}",
            e.time_s - zero
        );
        assert!(
            chopped.abs() < h * h * w * w * ip / 4.0,
            "{method}: {chopped}"
        );
        // At the grid point: up to one step late, w Ip h of current.
        assert!(grid.stats.events[0].time_s - zero > 0.0);
        assert!(chopped_grid.abs() > 100.0 * chopped.abs());
        // Before the opening the run follows the exact current; afterwards
        // the current is zero.
        for (t, row) in out.t.iter().zip(&out.rows) {
            if *t < e.time_s {
                assert!((row[0] - exact(*t)).abs() < 1e-3, "{method} at {t}");
            } else if *t > e.time_s + h {
                assert!(row[0].abs() < 1e-9, "{method} at {t}: {}", row[0]);
            }
        }
    }
}

#[test]
fn each_phase_of_a_fault_clears_at_its_current_zero_within_a_step() {
    // The network of `three_phase.rs`: a 66 kV source and a coupled line,
    // with a three-phase fault without ground. Z1 = 1.5 + j9 ohm.
    let (f, v_ll) = (50.0, 66_000.0);
    let w = 2.0 * std::f64::consts::PI * f;
    let h = 2e-5;
    let model = |switching: &str| {
        with_switching(
            json!({
                "format": "openkeito-model",
                "version": "0.3",
                "simulation": {"time_step_s": h, "end_time_s": 0.34},
                "components": [
                    {"type": "three_phase_source", "name": "G1", "bus": "s", "line_voltage_rms_v": v_ll,
                     "frequency_hz": f, "angle_deg": 20.0,
                     "impedance": {"type": "sequence", "r1_ohm": 0.5, "l1_h": 5.0 / w, "r0_ohm": 1.0, "l0_h": 15.0 / w}},
                    {"type": "coupled_rl", "name": "LINE", "buses": ["s", "f"],
                     "impedance": {"type": "sequence", "r1_ohm": 1.0, "l1_h": 4.0 / w, "r0_ohm": 3.0, "l0_h": 12.0 / w}},
                    {"type": "fault", "name": "F1", "bus": "f", "phases": ["a", "b", "c"],
                     "start_time_s": 0.0, "clear_time_s": 0.3},
                ],
                "outputs": [{"type": "branch_current", "component": "F1"}],
            }),
            switching,
        )
    };
    let out = run_model(&model("interpolated"));
    let grid = run_model(&model("grid"));
    let opens = |r: &Run| -> Vec<(f64, f64)> {
        r.stats
            .events
            .iter()
            .filter(|e| e.action == "open")
            .map(|e| (e.time_s, e.interrupted_current_a.unwrap()))
            .collect()
    };
    let (o, og) = (opens(&out), opens(&grid));
    println!("fault clearing: {o:?}; at grid points {og:?}");
    assert_eq!(o.len(), 3);
    let peak = 2f64.sqrt() * (v_ll / 3f64.sqrt()) / (1.5f64 * 1.5 + 81.0).sqrt();
    // Every pole interrupts what linear interpolation of a sine leaves,
    // (1/8) h^2 w^2 Ip at most (twice that allowed here); at grid points up
    // to w Ip h.
    for (t, i) in &o {
        assert!(*t > 0.3);
        assert!(
            i.abs() < h * h * w * w * peak / 4.0,
            "interrupts {i} A at {t}"
        );
    }
    assert!(og.iter().any(|(_, i)| i.abs() > 0.1 * w * peak * h));
    // The first pole clears alone; the other two then carry one current,
    // which for a symmetrical fault reaches zero a quarter period later.
    // The dc components of the fault current have decayed by
    // exp(-0.3 R/L) = 1.5e-7 of the peak.
    assert_eq!(o[1].0, o[2].0);
    let delay = o[1].0 - o[0].0;
    assert!((delay - 0.25 / f).abs() < h * h * w / 4.0 + 1e-8, "{delay}");
    // No fault current after the last opening.
    for (t, row) in out.t.iter().zip(&out.rows) {
        if *t > o[2].0 + h {
            assert!(row.iter().all(|i| *i == 0.0), "at {t}: {row:?}");
        }
    }
}

// ---------------------------------------------------------------------------
// Six-pulse diode rectifier with a source inductance and a smoothed load
// current.

/// The rectifier: a three-phase source with the inductance `ls` per phase,
/// six diodes, and a load of `rd` in series with a smoothing inductor `ld`
/// that starts at `id`. Output: the load current.
fn six_pulse(h: f64, end: f64, v_ll: f64, ls: f64, rd: f64, ld: f64, id: f64) -> Value {
    let diode =
        |name: &str, a: &str, k: &str| json!({"type": "diode", "name": name, "nodes": [a, k]});
    json!({
        "format": "openkeito-model",
        "version": "0.3",
        "simulation": {"time_step_s": h, "end_time_s": end},
        "components": [
            {"type": "three_phase_source", "name": "G1", "bus": "ac", "line_voltage_rms_v": v_ll, "frequency_hz": 50.0,
             "impedance": {"type": "sequence", "r1_ohm": 0.0, "l1_h": ls, "r0_ohm": 0.0, "l0_h": ls}},
            diode("D1", "ac.a", "p"), diode("D3", "ac.b", "p"), diode("D5", "ac.c", "p"),
            diode("D4", "n", "ac.a"), diode("D6", "n", "ac.b"), diode("D2", "n", "ac.c"),
            {"type": "resistor", "name": "RD", "nodes": ["p", "m"], "resistance_ohm": rd},
            {"type": "inductor", "name": "LD", "nodes": ["m", "n"], "inductance_h": ld, "initial_current_a": id},
        ],
        "outputs": [{"type": "branch_current", "component": "LD"}],
    })
}

#[test]
fn six_pulse_rectifier_has_the_textbook_average_with_commutation_overlap() {
    // V_d = (3 sqrt 2/pi) V_LL - (3/pi) w L_s I_d for a constant load
    // current (Mohan, Undeland, Robbins, ch. 5): the second term is the
    // voltage lost while two diodes commutate through the source
    // inductances. 400 V, 1 mH, 50 A: 540.2 - 15.0 V, overlap angle
    // acos(1 - 2 w Ls Id/(sqrt 2 V_LL)) = 19.2 deg.
    let (v_ll, ls, id) = (400.0, 1e-3, 50.0);
    let w = 2.0 * std::f64::consts::PI * 50.0;
    let formula = |i: f64| {
        3.0 * 2f64.sqrt() / std::f64::consts::PI * v_ll
            - 3.0 / std::f64::consts::PI * w * ls * i
            // Two conducting diodes of 1 mohm (three during the overlap,
            // which changes this drop by less than a quarter of it).
            - 2.0 * 1e-3 * i
    };
    // The load holds the current: 1 H against the ripple of the rectified
    // voltage (sixth harmonic, 30 V at most) leaves 30/(6 w L) = 0.016 A.
    // The average of the load voltage over whole periods is then
    // R mean(i) + L (i(t1) - i(t0))/(t1 - t0), from the current alone, which
    // is continuous (the voltage jumps at every commutation, and the mean
    // of its samples would be off by a jump times h f).
    let (rd, ld) = (formula(id) / id, 1.0);
    let h = 2e-5;
    let (t0, t1) = (0.04, 0.06);
    let average = |switching: &str| {
        let model = with_switching(six_pulse(h, t1, v_ll, ls, rd, ld, id), switching);
        let out = run_model(&model);
        let i = mean(&out, 0, t0, t1);
        let k0 = (t0 / h).round() as usize;
        let di = out.rows[out.rows.len() - 1][0] - out.rows[k0][0];
        (rd * i + ld * di / (t1 - t0), i, out)
    };
    let (v, i, out) = average("interpolated");
    let (vg, ig, grid) = average("grid");
    println!(
        "six-pulse: average {v:.4} V at {i:.4} A, formula {:.4} V (at grid points {vg:.4} V at {ig:.4} A, formula {:.4} V); \
         {} instants within steps, {} re-solutions (at most {}), {} at grid points",
        formula(i),
        formula(ig),
        out.stats.instants_within_steps,
        out.stats.resolves,
        out.stats.max_resolves_per_step,
        grid.stats.resolves
    );
    // What is left: the drop on a third conducting diode during the overlap
    // (below 0.025 V) and the ripple of the load current.
    assert!((v - formula(i)).abs() < 0.03, "{v} vs {}", formula(i));
    // The average hardly shows where the commutations end (at grid points
    // the outgoing diode carries a reverse current for up to a step, which
    // the source inductances give back).
    assert!((vg - formula(ig)).abs() < 0.03);
    // The overlap does: from the turn-on of a diode of the upper half to
    // the turn-off of the one it takes over from,
    // u = acos(1 - 2 w Ls Id/(sqrt 2 V_LL)).
    let overlaps = |r: &Run| -> Vec<f64> {
        let upper = ["D1", "D3", "D5"];
        let events: Vec<_> = r
            .stats
            .events
            .iter()
            .filter(|e| e.time_s > t0 && upper.contains(&e.component.as_str()))
            .collect();
        events
            .windows(2)
            .filter(|p| p[0].action == "close" && p[1].action == "open")
            .map(|p| p[1].time_s - p[0].time_s)
            .collect()
    };
    let u = (1.0 - 2.0 * w * ls * i / (2f64.sqrt() * v_ll)).acos() / w;
    let worst = |v: &[f64]| v.iter().map(|x| (x - u).abs()).fold(0.0, f64::max);
    let (o, og) = (overlaps(&out), overlaps(&grid));
    println!(
        "six-pulse: overlap {:.4e} s by the formula; largest deviation {:.2e} s over {} commutations (at grid points {:.2e} s)",
        u,
        worst(&o),
        o.len(),
        worst(&og)
    );
    assert_eq!(o.len(), 3);
    // The ripple of the load current (0.02 A) moves the overlap by
    // 2 w Ls dI/(sqrt 2 V_LL sin u)/w = 2e-7 s; the zero of the outgoing
    // current is placed by linear interpolation of a current that ends as
    // (1 - cos): (1/8) h^2 i''/i' <= h^2 w/(8 tan u) = 5e-8 s.
    assert!(worst(&o) < 5e-7, "{o:?} vs {u}");
    // At grid points both ends are grid points: the overlap is a whole
    // number of steps, off by 6.7 us or more here.
    assert!(worst(&og) > 5e-6, "{og:?} vs {u}");
    assert_eq!(out.stats.cycle_fallbacks, 0);
    assert_eq!(out.stats.instant_limit_steps, 0);
    assert!(out.stats.max_resolves_per_step <= 3);
}

// ---------------------------------------------------------------------------
// An element with a history: the travelling-wave line.

#[test]
fn a_line_energized_within_a_step_carries_the_front_from_that_instant() {
    // 100 V through a switch onto phase a of a lossless line (400 ohm, 25 km
    // at 4 us/km: 100 us) that ends in 400 ohm: no reflection. The far end
    // is at 0 until t_c + tau and at 100 V from then on.
    let (h, tau, tc) = (1e-5, 1e-4, 1.237e-4);
    let model = |switching: &str| {
        with_switching(
            json!({
                "format": "openkeito-model",
                "version": "0.5",
                "simulation": {"time_step_s": h, "end_time_s": 5e-4},
                "components": [
                    {"type": "voltage_source", "name": "E1", "nodes": ["e", "gnd"], "waveform": {"type": "dc", "value": 100.0}},
                    {"type": "switch", "name": "SW1", "nodes": ["e", "s.a"], "close_time_s": tc},
                    {"type": "resistor", "name": "RB", "nodes": ["s.b", "gnd"], "resistance_ohm": 400.0},
                    {"type": "resistor", "name": "RC", "nodes": ["s.c", "gnd"], "resistance_ohm": 400.0},
                    {"type": "bergeron_line", "name": "LINE", "buses": ["s", "r"], "length_km": 25.0,
                     "per_km": {"type": "sequence",
                        "r1_ohm_per_km": 0.0, "l1_h_per_km": 1.6e-3, "c1_f_per_km": 1e-8,
                        "r0_ohm_per_km": 0.0, "l0_h_per_km": 1.6e-3, "c0_f_per_km": 1e-8}},
                    {"type": "resistor", "name": "TA", "nodes": ["r.a", "gnd"], "resistance_ohm": 400.0},
                    {"type": "resistor", "name": "TB", "nodes": ["r.b", "gnd"], "resistance_ohm": 400.0},
                    {"type": "resistor", "name": "TC", "nodes": ["r.c", "gnd"], "resistance_ohm": 400.0},
                ],
                "outputs": [
                    {"type": "node_voltage", "node": "r.a"},
                    {"type": "node_voltage", "node": "s.a"},
                ],
            }),
            switching,
        )
    };
    let out = run_model(&model("interpolated"));
    let grid = run_model(&model("grid"));
    assert_eq!(out.stats.events[0].time_s, tc);
    let arrival = tc + tau;
    let first_full = |r: &Run| {
        r.t.iter()
            .zip(&r.rows)
            .find(|(_, row)| (row[0] - 100.0).abs() < 1e-9)
            .map(|(t, _)| *t)
            .unwrap()
    };
    println!(
        "line: front due at {arrival:.4e} s; the far end is at 100 V from {:.4e} s (at grid points from {:.4e} s)",
        first_full(&out),
        first_full(&grid)
    );
    for (t, row) in out.t.iter().zip(&out.rows) {
        // The line keeps the value before and the one after the closing at
        // its place in the step: rows up to the arrival read the first,
        // later ones the second.
        if *t <= arrival {
            assert!(row[0].abs() < 1e-9, "at {t}: {}", row[0]);
        } else if *t >= arrival + h {
            assert!((row[0] - 100.0).abs() < 1e-9, "at {t}: {}", row[0]);
        }
        // The sending end is at 100 V from the first grid point after t_c.
        if *t > tc {
            assert!((row[1] - 100.0).abs() < 1e-9, "at {t}");
        }
    }
    // At grid points the switch closes at 130 us, and the far end reads the
    // value after the action one travel time later: complete at 230 us,
    // 6.3 us after the front is due.
    assert!(first_full(&out) < arrival + h);
    assert!((first_full(&grid) - (1.3e-4 + tau)).abs() < 1e-12);
}

// ---------------------------------------------------------------------------
// Default resistances at high voltage (W271).

fn codes(run: &Run) -> Vec<String> {
    run.stats
        .warnings
        .iter()
        .map(|d| d.code.to_string())
        .collect()
}

#[test]
fn default_resistances_above_the_voltage_of_one_device_give_one_warning() {
    // The six-pulse rectifier at 200 kV and 2 kA (13.6 mH for an overlap of
    // 20 degrees): every diode blocks the peak of the line voltage, 283 kV,
    // far more than one device does (8.5 kV in published data sheets;
    // the warning starts at 10 kV).
    let (v_ll, ls, id) = (200e3, 13.6e-3, 2e3);
    let rd = (3.0 * 2f64.sqrt() / std::f64::consts::PI * v_ll) / id;
    let model = six_pulse(2e-5, 0.03, v_ll, ls, rd, 1.0, id);
    let out = run_with(&model, "interpolated");
    assert_eq!(codes(&out), ["W271"]);
    let w = &out.stats.warnings[0];
    println!("{}\n{}", w.message, w.hint);
    // One warning names all six diodes, with what each blocked and carried.
    assert_eq!(w.path, "D1, D3, D5, D4, D6, D2");
    assert!(w
        .message
        .starts_with("6 elements with default resistances blocked more than 10.000 kV"));
    // The peak of the line voltage less the commutation notches: between
    // 270 and 283 kV; V^2/R_off is then 73 to 80 kW. The load current
    // starts at 2 kA and falls towards 1.94 kA (the load resistance is that
    // of the voltage without the commutation drop of 3 %): a diode carries
    // 1.9 to 2.0 kA, which drops 1.9 to 2.0 V on 1 mohm.
    let d1 = w.message.split("; ").collect::<Vec<_>>().join(" ");
    let number = |text: &str, before: &str, after: &str| -> f64 {
        let a = text
            .find(before)
            .unwrap_or_else(|| panic!("{before} in {text}"))
            + before.len();
        let b = a + text[a..].find(after).unwrap();
        text[a..b].trim().parse().unwrap()
    };
    let blocked = number(
        &d1,
        "D1 (default on_resistance_ohm and off_resistance_ohm blocked ",
        " kV",
    );
    let leak = number(&d1, "V^2/R_off = ", " kW");
    let drop = number(&d1, "R_on I = ", " V)");
    assert!((270.0..283.0).contains(&blocked), "{blocked}");
    assert!(
        (leak - blocked * blocked / 1e3).abs() < 0.01 * leak,
        "{leak}"
    );
    assert!((1.9..2.0).contains(&drop), "{drop}");
    assert!(w.hint.contains("(V_T0 + r_T I)/I") && w.hint.contains("write them in the model"));

    // The same values written in the model are a decision: no warning.
    let mut written = model.clone();
    for c in written["components"].as_array_mut().unwrap() {
        if c["type"] == "diode" {
            c["on_resistance_ohm"] = json!(1e-3);
            c["off_resistance_ohm"] = json!(1e6);
        }
    }
    let quiet = run_with(&written, "interpolated");
    assert!(codes(&quiet).is_empty(), "{:?}", quiet.stats.warnings);
    // The results are the same to the last bit.
    assert_eq!(quiet.rows, out.rows);
    // One resistance left to its default still warns, and says which.
    written["components"][1]
        .as_object_mut()
        .unwrap()
        .remove("on_resistance_ohm");
    let one = run_with(&written, "grid");
    assert_eq!(codes(&one), ["W271"]);
    assert_eq!(one.stats.warnings[0].path, "D1");
    assert!(one.stats.warnings[0]
        .message
        .contains("D1 (default on_resistance_ohm;"));

    // At 400 V the defaults are those of one device: no warning.
    let low = run_with(
        &six_pulse(2e-5, 0.03, 400.0, 1e-3, 10.5, 1.0, 50.0),
        "interpolated",
    );
    assert!(codes(&low).is_empty());
}

#[test]
fn the_examples_do_not_block_more_than_one_device_with_default_resistances() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let mut seen = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "json") {
            let model: Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            let out = run_model(&model);
            assert!(
                !codes(&out).contains(&"W271".to_string()),
                "{}",
                path.display()
            );
            seen += 1;
        }
    }
    assert!(seen >= 11);
}
