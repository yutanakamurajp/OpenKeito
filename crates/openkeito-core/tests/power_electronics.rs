//! Power-electronic circuits compared with closed-form solutions: buck
//! converter (continuous and discontinuous conduction, open and closed loop)
//! and single-phase diode bridge (resistive and capacitive load).
//!
//! Two kinds of error are separated (docs/numerical-method.md):
//! - integration error: the waveform is compared with the exact solution of
//!   the same circuit (switches as resistors with two values) for the gate
//!   instants the solver used and ideally timed diode zero crossings;
//! - switching-time error: averages, ripple and angles are compared with the
//!   textbook formulas for ideal instants. Switching at grid points moves an
//!   instant by less than one time step.

mod common;

use common::{run_model as run, *};
use openkeito_core::{RunStats, Simulator};
use serde_json::json;

#[test]
fn matrix_exponential_matches_known_cases() {
    // Undamped oscillator: rotation.
    let p = expm([[0.0, 1.0], [-1.0, 0.0]], 0.3);
    assert!((p[0][0] - 0.3_f64.cos()).abs() < 1e-14 && (p[0][1] - 0.3_f64.sin()).abs() < 1e-14);
    // Diagonal stiff system.
    let p = expm([[-5e8, 0.0], [0.0, -2.0]], 1e-6);
    assert!(p[0][0].abs() < 1e-200 && (p[1][1] - (-2e-6_f64).exp()).abs() < 1e-15);
    // Triangular: e^{At} = [[e^-t, t e^-t], [0, e^-t]].
    let p = expm([[-1.0, 1.0], [0.0, -1.0]], 2.0);
    assert!((p[0][1] - 2.0 * (-2.0_f64).exp()).abs() < 1e-12);
}

/// State of S1 over every step, from the switching events.
fn switch_states(stats: &RunStats, name: &str, h: f64, steps: usize) -> Vec<bool> {
    let mut on = vec![false; steps];
    let mut state = false;
    let mut events = stats
        .events
        .iter()
        .filter(|e| e.component == name)
        .peekable();
    for (k, s) in on.iter_mut().enumerate() {
        let t = k as f64 * h;
        while let Some(e) = events.peek() {
            if e.time_s <= t + 1e-9 * h {
                state = e.action == "close";
                events.next();
            } else {
                break;
            }
        }
        *s = state;
    }
    on
}

/// Largest deviation of i_L and v_C from the exact solution.
fn buck_error(buck: &Buck, method: &str, h: f64, end: f64, x0: [f64; 2]) -> (f64, f64, Run) {
    let out = run(&buck.model(method, h, end, x0));
    let steps = out.t.len() - 1;
    let s = switch_states(&out.stats, "S1", h, steps);
    let exact = buck.reference(h, &s, x0);
    let (mut ei, mut ev): (f64, f64) = (0.0, 0.0);
    for (row, x) in out.rows.iter().zip(&exact) {
        ei = ei.max((row[0] - x[0]).abs());
        ev = ev.max((row[1] - x[1]).abs());
    }
    (ei, ev, out)
}

#[test]
fn buck_in_continuous_conduction_follows_the_exact_solution_to_second_order() {
    let b = &CCM;
    // Start at the operating point (i_L at the bottom of its ripple,
    // 4 - 2.4/2 A; v_C = D*E), so that the inductor current stays positive
    // and every switching instant is a gate action on the grid.
    let x0 = [2.8, 40.0];
    let (h, end) = (1e-6, 2e-3);
    // Bound of the 2S-DIRK error: the local errors C3 h^3 |i_L'''| summed
    // over the steps, with |i_L'''| <= |i_L'| w0^2 = (E - v)/L / (L C).
    let c3 = dirk_error_constant().abs();
    let di3 = (b.e / b.l) / (b.l * b.c);
    let dirk_bound = end / h * c3 * h.powi(3) * di3;
    // The trapezoidal rule replaces the step after each of the 40 gate
    // actions by two backward-Euler half steps, local error h^2/4 |i_L''|
    // with |i_L''| = |i_C|/(L C) <= 1.2 A / (L C).
    let be_bound = 40.0 * h * h / 4.0 * 1.2 / (b.l * b.c);
    for (method, bound) in [
        ("2s-dirk", dirk_bound),
        ("trapezoidal", dirk_bound + be_bound),
    ] {
        let (ei1, ev1, out) = buck_error(b, method, h, end, x0);
        let (ei2, ev2, _) = buck_error(b, method, h / 2.0, end, x0);
        println!(
            "{method}: i_L error {ei1:.3e} -> {ei2:.3e} A (bound {bound:.1e}), v_C error {ev1:.3e} -> {ev2:.3e} V; \
             {} events, {} re-solutions (at most {} at a grid point)",
            out.stats.events.len(),
            out.stats.resolves,
            out.stats.max_resolves_per_step
        );
        assert!(ei1 < bound, "{method}: {ei1} > {bound}");
        // An error in i_L moves v_C by about sqrt(L/C) times as much.
        assert!(ev1 < bound * (b.l / b.c).sqrt(), "{method}: {ev1}");
        // Second order, also right after the switching actions: no
        // numerical oscillation.
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
        assert_eq!(out.stats.cycle_fallbacks, 0);
        assert!(out.stats.max_resolves_per_step <= 2);
    }
}

/// Fraction of the steps in t0 <= t < t1 with S1 on.
fn on_fraction(out: &Run, h: f64, t0: f64, t1: f64) -> f64 {
    let s = switch_states(&out.stats, "S1", h, out.t.len() - 1);
    let k0 = (t0 / h).round() as usize;
    let k1 = (t1 / h).round() as usize;
    s[k0..k1].iter().filter(|x| **x).count() as f64 / (k1 - k0) as f64
}

#[test]
fn buck_in_continuous_conduction_has_the_textbook_average_and_ripple() {
    let h = 1e-6;
    // A duty whose comparator crossings fall between grid points (20.25 and
    // 79.75 steps into the period): the switch is on for 41 steps in every
    // period. With D = 0.4 the crossings fall on grid points and rounding of
    // the carrier decides them, which makes the duty irregular.
    let b = &Buck { duty: 0.405, ..CCM };
    let out = run(&b.model("2s-dirk", h, 0.02, [2.89, 41.0]));
    let (t0, t1) = (0.019, 0.02);
    let d = on_fraction(&out, h, t0, t1);
    // An edge moves by less than one step, so the duty used differs from
    // the reference by less than h*f = 0.01.
    assert!((d - b.duty).abs() < h * b.f, "duty {d}");
    assert!((d - 0.41).abs() < 1e-12);
    // Average over a period from v_sw = (G_S E - i_L)/(G_S + G_D) and the
    // zero averages of v_L and i_C.
    let (gon, goff) = (1.0 / RON, 1.0 / ROFF);
    let g = gon + goff;
    let expected = b.e * (d * gon + (1.0 - d) * goff) / g / (1.0 + 1.0 / (b.r * g));
    let v = mean(&out, 1, t0, t1);
    let i = mean(&out, 0, t0, t1);
    println!(
        "CCM: duty used {d} (reference {}), average v_out {v:.6} V (expected {expected:.6} V, ideal D*E {} V)",
        b.duty,
        b.duty * b.e
    );
    // The LC transient from the start (< 1 V) has decayed by
    // exp(-0.019 s / (2 R C)) = 7.5e-5; the integration error is below
    // 2e-4 V (previous test).
    assert!(
        (v - expected).abs() < 1e-3,
        "average {v}, expected {expected}"
    );
    assert!((i - v / b.r).abs() < 1e-4);
    // With the reference duty the average is within E*h*f of D*E.
    assert!((v - b.duty * b.e).abs() <= b.e * h * b.f);
    // Ripple (E - V) D/(L f) = (1 - D) D E/(L f). The output ripple
    // (Delta I/(8 f C) = 0.3 V peak to peak) changes the slope (E - v)/L by
    // up to 0.3/60 = 0.5 %; the extremes of i_L fall on grid points.
    let window: Vec<f64> = out
        .t
        .iter()
        .zip(&out.rows)
        .filter(|(t, _)| **t >= t0 - 1e-12)
        .map(|(_, r)| r[0])
        .collect();
    let ripple = window.iter().cloned().fold(f64::MIN, f64::max)
        - window.iter().cloned().fold(f64::MAX, f64::min);
    let formula = (b.e - v) * d / (b.l * b.f);
    println!("CCM: ripple {ripple:.5} A, formula {formula:.5} A");
    assert!(
        (ripple - formula).abs() < 0.005 * formula,
        "ripple {ripple}, formula {formula}"
    );
}

/// Average output voltage of the buck converter in discontinuous conduction
/// for a constant output voltage: M = 2/(1 + sqrt(1 + 4K/D^2)), K = 2L/(R T)
/// (Erickson and Maksimovic, ch. 5).
fn dcm_ratio(b: &Buck, d: f64) -> f64 {
    let k = 2.0 * b.l * b.f / b.r;
    2.0 / (1.0 + (1.0 + 4.0 * k / (d * d)).sqrt())
}

#[test]
fn buck_in_discontinuous_conduction_converges_to_first_order() {
    let b = &DCM;
    assert!((dcm_ratio(b, 0.4) - 0.8).abs() < 1e-12);
    assert!(2.0 * b.l * b.f / b.r < 1.0 - b.duty, "discontinuous");
    let (end, t0, t1) = (0.03, 0.029, 0.03);
    let periods = end * b.f;
    let mut errors = Vec::new();
    let mut offsets = Vec::new();
    for h in [2e-6, 1e-6, 5e-7, 2.5e-7] {
        // The output settles with a time constant of about 3 ms.
        let x0 = [0.0, 80.0];
        let out = run(&b.model("2s-dirk", h, end, x0));
        let steps = out.t.len() - 1;
        let s = switch_states(&out.stats, "S1", h, steps);
        let exact = b.reference(h, &s, x0);
        let mut e: f64 = 0.0;
        for (row, x) in out.rows.iter().zip(&exact) {
            e = e.max((row[0] - x[0]).abs());
        }
        let v = mean(&out, 1, t0, t1);
        let k0 = (t0 / h).round() as usize;
        let k1 = (t1 / h).round() as usize;
        let v_exact = exact[k0..k1].iter().map(|x| x[1]).sum::<f64>() / (k1 - k0) as f64;
        let d = on_fraction(&out, h, t0, t1);
        let textbook = dcm_ratio(b, d) * b.e;
        println!(
            "DCM h = {h:e}: i_L error {e:.3e} A, average {v:.5} V, exact with the same gate {v_exact:.5} V, \
             textbook M({d}) E = {textbook:.5} V, M({}) E = {:.5} V; {} re-solutions (at most {} at a grid point)",
            b.duty,
            dcm_ratio(b, b.duty) * b.e,
            out.stats.resolves,
            out.stats.max_resolves_per_step
        );
        // The diode opens at the first grid point after the zero crossing of
        // i_L, so i_L there is up to one step's change, (v_out/L) h, below 0.
        assert!(e <= 1.02 * v / b.l * h, "h = {h}: {e}");
        // A late opening draws at most (1/2)(v/L) h^2 from the output per
        // period: 8 mA at h = 1 us, or 27 mV through the output conductance
        // of about 0.3 S (load 0.05 S, converter 0.25 S).
        assert!(
            (v - v_exact).abs() < 0.03 * h / 1e-6,
            "h = {h}: {v} vs {v_exact}"
        );
        // The textbook formula assumes a constant output voltage; the ripple
        // is about 0.2 V.
        assert!(
            (v_exact - textbook).abs() < 0.2,
            "h = {h}: {v_exact} vs {textbook}"
        );
        assert!((d - b.duty).abs() < h * b.f);
        // Two re-solutions per period: no chattering.
        assert!(
            out.stats.resolves as f64 <= 5.0 * periods,
            "{}",
            out.stats.resolves
        );
        assert!(out.stats.max_resolves_per_step <= 2);
        assert_eq!(out.stats.cycle_fallbacks, 0);
        errors.push(e);
        offsets.push((v - v_exact).abs());
    }
    for k in 1..errors.len() {
        let ratio = errors[k - 1] / errors[k];
        assert!((1.8..2.2).contains(&ratio), "first order: ratio {ratio}");
        assert!(offsets[k] < offsets[k - 1]);
    }
}

// ---------------------------------------------------------------------------
// Single-phase diode bridge: source between a and gnd, D1 a->p, D3 gnd->p,
// D2 n->a, D4 n->gnd, load between p and n.

#[test]
fn bridge_rectifier_with_resistive_load_gives_the_rectified_sine() {
    let (h, r) = (1e-6, 10.0);
    let vp = V_RMS * 2f64.sqrt();
    let w = 2.0 * std::f64::consts::PI * F_AC;
    let out = run(&bridge(h, 0.04, r, None, RON));
    let steps = out.t.len() - 1;
    let state: Vec<Vec<bool>> = ["D1", "D2", "D3", "D4"]
        .iter()
        .map(|d| switch_states(&out.stats, d, h, steps))
        .collect();
    let g = |on: bool| if on { 1.0 / RON } else { 1.0 / ROFF };
    let vs = |t: f64| vp * (w * t).sin();
    let mut worst: f64 = 0.0;
    let mut late = 0;
    for k in 1..out.t.len() {
        // The states over step k-1 are decided at t_{k-1} from the solution
        // there: those of the ideal rectifier for the sign of the source at
        // t_{k-1}.
        let before = vs(out.t[k - 1]);
        let on = |d: usize| state[d][k - 1];
        if before > 1e-6 {
            assert!(on(0) && on(3) && !on(1) && !on(2), "t = {}", out.t[k - 1]);
        } else if before < -1e-6 {
            assert!(on(1) && on(2) && !on(0) && !on(3), "t = {}", out.t[k - 1]);
        }
        if before * vs(out.t[k]) < 0.0 {
            late += 1;
        }
        // Load voltage of the resistive network with these states (nodal
        // equations of p and n): no states, so no integration error.
        let (g1, g2, g3, g4) = (g(on(0)), g(on(1)), g(on(2)), g(on(3)));
        let v = vs(out.t[k]);
        let gl = 1.0 / r;
        let (a11, a22) = (g1 + g3 + gl, g2 + g4 + gl);
        let det = a11 * a22 - gl * gl;
        let vpn = (g1 * v * a22 + gl * g2 * v - gl * g1 * v - a11 * g2 * v) / det;
        worst = worst.max((out.rows[k][0] - vpn).abs());
    }
    println!(
        "R load: deviation from the network solution {worst:.2e} V, {late} rows after a zero crossing, \
         {} events, {} re-solutions (at most {} at a grid point)",
        out.stats.events.len(),
        out.stats.resolves,
        out.stats.max_resolves_per_step
    );
    // Rounding only (values up to 141 V).
    assert!(worst < 1e-9, "{worst}");
    // Zero crossings at 10, 20, 30 and 40 ms fall on grid points, where
    // the sign of sin(k pi) is rounding.
    assert!((3..=4).contains(&late), "{late}");
    // Average over two cycles: 2 sqrt(2) V/pi with the drop on two
    // conducting diodes. The rows after the four zero crossings use the old
    // states; each differs by at most 2 w Vp h = 0.09 V, 9e-6 V in the mean.
    // Blocking diodes leak V/R_off, which changes the load voltage by less
    // than R/R_off = 1e-5 of 90 V.
    let n = out.rows.len() - 1;
    let mean = out.rows[..n].iter().map(|r| r[0]).sum::<f64>() / n as f64;
    let expected = 2.0 * 2f64.sqrt() * V_RMS / std::f64::consts::PI * r / (r + 2.0 * RON);
    println!("R load: average {mean:.6} V, expected {expected:.6} V");
    assert!((mean - expected).abs() < 1e-3, "{mean} vs {expected}");
    // Four diodes change at each zero crossing and settle after one more
    // solution.
    assert_eq!(out.stats.events.len(), 2 + 4 * 3);
    assert!(out.stats.max_resolves_per_step <= 2);
    assert_eq!(out.stats.cycle_fallbacks, 0);
}

#[test]
fn capacitor_input_rectifier_matches_the_conduction_angles() {
    let (r, c) = (100.0, 1e-3);
    // Small on-resistance: the charging path 2 R_on C = 20 ns lags the
    // source by w * 20 ns = 6e-6 rad, negligible against the ideal formulas.
    let ron = 1e-5;
    let vp = V_RMS * 2f64.sqrt();
    let w = 2.0 * std::f64::consts::PI * F_AC;
    let (off, on, vmin) = capacitor_input_angles(w, r, c);
    println!(
        "C input, ideal: turn-off {off:.6} rad, turn-on {on:.6} rad, conduction {:.4} deg, v_min {:.5} V",
        (off - on).to_degrees(),
        vp * vmin
    );
    for h in [2e-6, 1e-6, 5e-7] {
        let out = run(&bridge(h, 0.1, r, Some(c), ron));
        // Last cycle. D1 conducts in the positive half wave, D2 in the
        // negative one.
        let last = 0.08;
        let angle = |e: &openkeito_core::SwitchEvent| (w * e.time_s) % std::f64::consts::PI;
        let of = |action: &str| -> Vec<f64> {
            out.stats
                .events
                .iter()
                .filter(|e| e.time_s >= last && e.action == action)
                .filter(|e| e.component == "D1" || e.component == "D2")
                .map(angle)
                .collect()
        };
        let (opens, closes) = (of("open"), of("close"));
        let tail: Vec<(f64, &Vec<f64>)> = out
            .t
            .iter()
            .cloned()
            .zip(&out.rows)
            .filter(|(t, _)| *t >= last)
            .collect();
        let lowest = tail.iter().map(|(_, r)| r[0]).fold(f64::MAX, f64::min);
        let highest = tail.iter().map(|(_, r)| r[0]).fold(f64::MIN, f64::max);
        println!(
            "C input h = {h:e}: turn-off {:.6?} rad, turn-on {:.6?} rad, v_min {lowest:.5} V, v_max {highest:.5} V; \
             {} re-solutions (at most {} at a grid point)",
            opens, closes, out.stats.resolves, out.stats.max_resolves_per_step
        );
        assert_eq!((opens.len(), closes.len()), (2, 2));
        // A state changes at the first grid point after the ideal instant:
        // late by at most one step, w h.
        for a in &opens {
            assert!(
                *a - off > -1e-5 && *a - off < w * h + 1e-5,
                "turn-off {a} vs {off}"
            );
        }
        for a in &closes {
            assert!(
                *a - on > -1e-5 && *a - on < w * h + 1e-5,
                "turn-on {a} vs {on}"
            );
        }
        // The late turn-on lowers the minimum by up to (v/(R C)) h; the
        // blocking diodes leak, which speeds the decay by about
        // R/R_off = 1e-4 of the droop of 11.7 V.
        let bound = vp * vmin / (r * c) * h + 1e-4 * vp * (1.0 - vmin) + 1e-4;
        assert!(
            (lowest - vp * vmin).abs() < bound,
            "v_min {lowest} vs {}",
            vp * vmin
        );
        // The peak is reached while conducting, at a grid point within
        // h/2 of w t = pi/2: error (1/2) Vp (w h/2)^2 plus 2 R_on Vp/R.
        assert!((highest - vp).abs() < 1e-4, "v_max {highest}");
        // No oscillation after a turn-on: from the second row on, the load
        // voltage follows |v_s| within the drop on two conducting diodes.
        let mut worst: f64 = 0.0;
        for (k, (t, row)) in tail.iter().enumerate().skip(2) {
            let th = (w * t) % std::f64::consts::PI;
            let th_before = (w * tail[k - 2].0) % std::f64::consts::PI;
            if th_before > on + w * h && th_before < th && th < off {
                let i = row[1].abs().max(row[2].abs());
                let drop = (row[0] - (vp * (w * t).sin()).abs()).abs();
                worst = worst.max(drop - 2.0 * ron * i);
            }
        }
        println!("C input h = {h:e}: largest deviation from |v_s| while conducting {worst:.2e} V");
        assert!(worst < 1e-6, "{worst}");
        assert!(out.stats.max_resolves_per_step <= 2);
        assert_eq!(out.stats.cycle_fallbacks, 0);
    }
}

// ---------------------------------------------------------------------------
// Closed loop.

#[test]
fn pi_controlled_buck_holds_the_reference_voltage() {
    let b = &CCM;
    let h = 1e-6;
    let mut m = b.model("2s-dirk", h, 0.25, [0.0, 0.0]);
    // duty = PI(v_ref - v_out), limited to [0, 0.95]. The integral gain sets
    // the crossover of the loop at about E * K_i / (2 pi) = 50 Hz, well
    // below the LC resonance (503 Hz, Q = 3.2); the proportional gain keeps
    // E * K_p * Q below 1.
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
        {"type": "signal", "signal": "error"},
    ]);
    let out = run(&m);
    let (t0, t1) = (0.05, 0.25);
    let v = mean(&out, 0, t0, t1);
    let d = mean(&out, 1, t0, t1);
    println!("PI: average v_out {v:.5} V, average duty {d:.5} over {t0}..{t1} s");
    // Integral action removes the error of the average. The loop settles
    // with a time constant of 1/(E K_i) = 3.3 ms. With 100 steps per period
    // the comparator can only realize 29 or 31 steps on near D = 0.3, so the
    // integral keeps moving the duty across that boundary: a limit cycle
    // between output levels 2 V apart. Averaged over 200 ms (tens of these
    // cycles) the error is below 0.01 V.
    assert!((v - 30.0).abs() < 0.01, "{v}");
    assert!((d - 0.3).abs() < 0.01, "{d}");
}

// ---------------------------------------------------------------------------
// Time of a long run.

/// 10 kHz buck converter, time step 1 us, 1 s. Run with
/// `cargo test --release -p openkeito-core --test power_electronics -- --ignored --nocapture`.
#[test]
#[ignore]
fn time_of_one_second_of_a_10_khz_buck_converter() {
    let b = &Buck { duty: 0.405, ..CCM };
    let model = b.model("2s-dirk", 1e-6, 1.0, [2.89, 41.0]);
    let loaded = openkeito_model::load_str(&model.to_string()).unwrap();
    let sim = Simulator::from_model(&loaded.model).unwrap();
    let mut rows = 0u64;
    let started = std::time::Instant::now();
    let stats = sim
        .run(|_, _| {
            rows += 1;
            Ok(())
        })
        .unwrap();
    let elapsed = started.elapsed().as_secs_f64();
    println!(
        "1 s at 10 kHz, h = 1 us: {elapsed:.3} s wall clock ({:.1} ns per step); factorize {:.3} s, solve {:.3} s, \
         output {:.3} s; {} steps, {} factorizations, {} solutions, {} re-solutions, {} events, {rows} rows",
        elapsed / stats.steps as f64 * 1e9,
        stats.timing.factorize_s,
        stats.timing.solve_s,
        stats.timing.output_s,
        stats.steps,
        stats.factorizations,
        stats.solves,
        stats.resolves,
        stats.events.len()
    );
}
