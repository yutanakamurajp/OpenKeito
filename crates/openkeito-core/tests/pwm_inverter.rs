//! A three-phase two-level PWM inverter solved with large time steps
//! (1 to 25 µs) against the same model solved with switching instants
//! within the step at h = 0.25 µs (issue #48).
//!
//! The bridge is built from its elements: per phase a `controlled_switch`
//! to each rail with an antiparallel `diode`, between DC sources of
//! +-Vdc/2 whose midpoint is grounded. An L filter (`coupled_rl`) joins it
//! to a grid of a three-phase source behind a `coupled_rl`. Sine-triangle
//! PWM with a carrier of 2450 Hz, whose period (408.16 µs) is not a whole
//! number of any of the time steps. Two controls (modulation fixed, or PI
//! current control in the rotating frame) and two dead times (0 and 2 µs,
//! the second shorter than the steps).
//!
//! What is compared, on the phase currents over one period of the
//! fundamental (40 to 60 ms), with the reference sampled at the grid points
//! of the coarse run: the largest difference in a row, the largest
//! difference of the averages over carrier periods (both curves integrated
//! the same way between their samples), the amplitude and phase of the
//! fundamental (a DFT over that period), and the THD up to the 40th
//! harmonic.
//!
//! The ordinary test runs one case at 10, 20 and 25 µs against 1 µs (at
//! 25 µs the narrowest pulses begin and end within a step). The table
//! of the documents comes from the ignored test:
//! `cargo test --release -p openkeito-core --test pwm_inverter -- --ignored --nocapture`.

mod common;

use std::f64::consts::PI;
use std::time::Instant;

use common::*;
use serde_json::{json, Value};

const F: f64 = 50.0;
const FC: f64 = 2450.0;
const V_LL: f64 = 400.0;
const VDC: f64 = 750.0;
/// Filter and grid: R and L per phase.
const RF: f64 = 0.2;
const LF: f64 = 2e-3;
const RG: f64 = 0.02;
const LG: f64 = 0.2e-3;
/// Modulation index and angle of the open loop: about 100 A in phase with
/// the grid voltage.
const M: f64 = 0.89;
const DELTA_DEG: f64 = 11.9;
/// Peak of the rated current, 50 kVA at 400 V.
fn rated_peak() -> f64 {
    50e3 / (3f64.sqrt() * V_LL) * 2f64.sqrt()
}

/// The window: one period of the fundamental, after the start.
const T0: f64 = 0.04;
const T1: f64 = 0.06;
/// The interval of the rows of the reference.
const H_ROW: f64 = 1e-6;

#[derive(Clone, Copy, Debug)]
enum Control {
    /// Fixed modulation.
    Open,
    /// PI control of the current in the rotating frame of the grid.
    Pi,
}

#[derive(Clone, Copy, Debug)]
struct Case {
    control: Control,
    dead_s: f64,
}

fn model(case: Case, h: f64, switching: &str, end: f64) -> Value {
    let w = 2.0 * PI * F;
    let seq = |r: f64, l: f64| json!({"type": "sequence", "r1_ohm": r, "l1_h": l, "r0_ohm": r, "l0_h": l});
    let mut components = vec![
        json!({"type": "voltage_source", "name": "EP", "nodes": ["dcp", "gnd"], "waveform": {"type": "dc", "value": VDC / 2.0}}),
        json!({"type": "voltage_source", "name": "EN", "nodes": ["dcn", "gnd"], "waveform": {"type": "dc", "value": -VDC / 2.0}}),
        json!({"type": "coupled_rl", "name": "LF", "buses": ["sw", "pcc"], "impedance": seq(RF, LF)}),
        json!({"type": "coupled_rl", "name": "ZG", "buses": ["pcc", "g"], "impedance": seq(RG, LG)}),
        json!({"type": "three_phase_source", "name": "G1", "bus": "g", "line_voltage_rms_v": V_LL, "frequency_hz": F}),
    ];
    // The dead time: the carrier is shifted by +-2 fc Td for the two
    // comparisons, which moves each edge by Td/2 (the carrier from -1 to 1
    // has the slope 4 fc), so both switches are off for Td.
    let shift = 2.0 * FC * case.dead_s;
    let mut controls = vec![
        json!({"type": "triangle_wave", "name": "carrier", "frequency_hz": FC, "minimum": -1.0, "maximum": 1.0}),
        json!({"type": "constant", "name": "shift", "value": shift}),
        json!({"type": "sum", "name": "cp", "inputs": ["carrier", "shift"], "signs": [1.0, 1.0]}),
        json!({"type": "sum", "name": "cn", "inputs": ["carrier", "shift"], "signs": [1.0, -1.0]}),
    ];
    match case.control {
        Control::Open => {
            for (ph, angle) in [("a", 0.0), ("b", -120.0), ("c", 120.0)] {
                controls.push(json!({"type": "sine_wave", "name": format!("m_{ph}"), "amplitude": M, "frequency_hz": F,
                    "phase_deg": 90.0 + DELTA_DEG + angle}));
            }
        }
        Control::Pi => {
            let vm = V_LL * (2.0f64 / 3.0).sqrt();
            controls.extend([
                json!({"type": "ramp", "name": "theta", "slope_per_s": w}),
                json!({"type": "three_phase_measurement", "name": "i", "quantity": {"type": "branch_current", "component": "LF"}}),
                json!({"type": "abc_to_dq0", "name": "i_dq", "inputs": ["i.a", "i.b", "i.c"], "angle": "theta"}),
                json!({"type": "constant", "name": "id_ref", "value": 100.0}),
                json!({"type": "constant", "name": "iq_ref", "value": 0.0}),
                json!({"type": "sum", "name": "ed", "inputs": ["id_ref", "i_dq.d"], "signs": [1.0, -1.0]}),
                json!({"type": "sum", "name": "eq", "inputs": ["iq_ref", "i_dq.q"], "signs": [1.0, -1.0]}),
                json!({"type": "constant", "name": "ff_d", "value": vm}),
                json!({"type": "constant", "name": "ff_q", "value": 0.0}),
                // A bandwidth of about 300 Hz on L = 2.2 mH, the zero on R/L.
                json!({"type": "pi_dq", "name": "e", "inputs": ["ed", "eq"], "feedforward": ["ff_d", "ff_q"],
                    "proportional_gain": 4.0, "integral_gain_per_s": 400.0}),
                json!({"type": "dq0_to_abc", "name": "v_ref", "d": "e.d", "q": "e.q", "angle": "theta"}),
            ]);
            for ph in ["a", "b", "c"] {
                controls.push(json!({"type": "gain", "name": format!("m_{ph}"), "input": format!("v_ref.{ph}"), "gain": 2.0 / VDC}));
            }
        }
    }
    for ph in ["a", "b", "c"] {
        let sw = format!("sw.{ph}");
        components.extend([
            json!({"type": "controlled_switch", "name": format!("SP{ph}"), "nodes": ["dcp", sw], "gate": format!("gp_{ph}")}),
            json!({"type": "controlled_switch", "name": format!("SN{ph}"), "nodes": [sw, "dcn"], "gate": format!("gn_{ph}")}),
            json!({"type": "diode", "name": format!("DP{ph}"), "nodes": [sw, "dcp"]}),
            json!({"type": "diode", "name": format!("DN{ph}"), "nodes": ["dcn", sw]}),
        ]);
        controls.push(json!({"type": "comparator", "name": format!("gp_{ph}"), "inputs": [format!("m_{ph}"), "cp"]}));
        controls.push(json!({"type": "comparator", "name": format!("gn_{ph}"), "inputs": ["cn", format!("m_{ph}")]}));
    }
    let interval = ((H_ROW / h).round() as u64).max(1);
    json!({"format": "openkeito-model", "version": "0.5",
        "simulation": {"time_step_s": h, "end_time_s": end, "switching": switching, "output_interval_steps": interval},
        "components": components, "controls": controls,
        "outputs": [{"type": "branch_current", "component": "LF"}]})
}

/// A run with its time of computation.
struct Timed {
    run: Result<Run, String>,
    seconds: f64,
}

fn solve(case: Case, h: f64, switching: &str, end: f64) -> Timed {
    let loaded = openkeito_model::load_str(&model(case, h, switching, end).to_string())
        .unwrap_or_else(|e| panic!("model is valid: {e:#?}"));
    let sim = openkeito_core::Simulator::from_model(&loaded.model).expect("circuit builds");
    let h_row = h * loaded.model.simulation.output_interval_steps.unwrap_or(1) as f64;
    let start = Instant::now();
    let run = try_run_sim(sim, h_row).map_err(|e| e.to_diagnostic().code.to_string());
    Timed {
        run,
        seconds: start.elapsed().as_secs_f64(),
    }
}

/// The rows of `run` (interval `run.h`) at the times t0 + k h_c up to t1.
fn at_times(run: &Run, h_c: f64, t0: f64, t1: f64) -> Vec<(f64, [f64; 3])> {
    let step = (h_c / run.h).round() as usize;
    let k0 = (t0 / run.h).round() as usize;
    let k1 = (t1 / run.h).round() as usize;
    (k0..=k1)
        .step_by(step)
        .map(|k| (run.t[k], [run.rows[k][0], run.rows[k][1], run.rows[k][2]]))
        .collect()
}

/// The integral of the line through the samples from a to b.
fn integral(s: &[(f64, f64)], a: f64, b: f64) -> f64 {
    let value = |t: f64| -> f64 {
        let k = s.partition_point(|p| p.0 <= t).clamp(1, s.len() - 1);
        let (p, q) = (s[k - 1], s[k]);
        p.1 + (q.1 - p.1) * (t - p.0) / (q.0 - p.0)
    };
    let mut points = vec![a];
    points.extend(s.iter().map(|p| p.0).filter(|&t| t > a && t < b));
    points.push(b);
    points
        .windows(2)
        .map(|w| 0.5 * (value(w[0]) + value(w[1])) * (w[1] - w[0]))
        .sum()
}

/// The averages over the carrier periods that fit into the samples.
fn carrier_averages(s: &[(f64, f64)]) -> Vec<f64> {
    let tc = 1.0 / FC;
    let (t0, t1) = (s[0].0, s[s.len() - 1].0);
    let count = ((t1 - t0) / tc).floor() as usize;
    (0..count)
        .map(|j| {
            let a = t0 + j as f64 * tc;
            integral(s, a, a + tc) / tc
        })
        .collect()
}

/// Harmonic n of the samples over one period of the fundamental (the last
/// sample is the first one again): its amplitude and phase.
fn harmonic(s: &[(f64, f64)], n: usize) -> (f64, f64) {
    let samples = &s[..s.len() - 1];
    let t0 = s[0].0;
    let w = 2.0 * PI * F * n as f64;
    let (mut re, mut im) = (0.0, 0.0);
    for &(t, x) in samples {
        re += x * (w * (t - t0)).cos();
        im -= x * (w * (t - t0)).sin();
    }
    let scale = 2.0 / samples.len() as f64;
    ((re * scale).hypot(im * scale), im.atan2(re))
}

fn thd(s: &[(f64, f64)]) -> f64 {
    let fundamental = harmonic(s, 1).0;
    (2..=40)
        .map(|n| harmonic(s, n).0.powi(2))
        .sum::<f64>()
        .sqrt()
        / fundamental
}

/// The differences of a run from the reference, largest over the phases.
#[derive(Debug, Default, Clone, Copy)]
struct Errors {
    row: f64,
    average: f64,
    amplitude: f64,
    phase_deg: f64,
    thd: f64,
}

fn errors(coarse: &Run, h: f64, reference: &Run, (t0, t1): (f64, f64)) -> Errors {
    let a = at_times(coarse, h, t0, t1);
    let b = at_times(reference, h, t0, t1);
    assert_eq!(a.len(), b.len());
    let mut e = Errors::default();
    for ph in 0..3 {
        let x: Vec<(f64, f64)> = a.iter().map(|p| (p.0, p.1[ph])).collect();
        let y: Vec<(f64, f64)> = b.iter().map(|p| (p.0, p.1[ph])).collect();
        let row = x
            .iter()
            .zip(&y)
            .map(|(p, q)| (p.1 - q.1).abs())
            .fold(0.0, f64::max);
        let (ax, ay) = (carrier_averages(&x), carrier_averages(&y));
        let average = ax
            .iter()
            .zip(&ay)
            .map(|(p, q)| (p - q).abs())
            .fold(0.0, f64::max);
        let (mx, px) = harmonic(&x, 1);
        let (my, py) = harmonic(&y, 1);
        let mut dp = (px - py).to_degrees();
        dp -= 360.0 * (dp / 360.0).round();
        e.row = e.row.max(row);
        e.average = e.average.max(average);
        e.amplitude = e.amplitude.max((mx - my).abs() / my);
        e.phase_deg = e.phase_deg.max(dp.abs());
        e.thd = e.thd.max((thd(&x) - thd(&y)).abs());
    }
    e
}

/// The instants of the dead time against the time steps: the largest
/// number of switching events within one step of `h`.
fn events_per_step(run: &Run, h: f64) -> usize {
    let mut counts = std::collections::BTreeMap::new();
    for e in &run.stats.events {
        *counts.entry((e.time_s / h).floor() as i64).or_insert(0) += 1;
    }
    counts.values().copied().max().unwrap_or(0)
}

#[test]
fn a_pwm_inverter_at_large_steps_keeps_the_fundamental_of_fine_steps() {
    // One case, 25 ms (from 5 to 25 ms is one period of the fundamental:
    // the window of this test only), against 1 µs.
    let case = Case {
        control: Control::Open,
        dead_s: 2e-6,
    };
    let end = 0.025;
    let reference = solve(case, 1e-6, "interpolated", end).run.unwrap();
    let mut last: Option<Errors> = None;
    for h in [2e-5, 1e-5] {
        let coarse = solve(case, h, "interpolated", end).run.unwrap();
        let e = errors(&coarse, h, &reference, (0.005, 0.025));
        println!(
            "h {:.0} us: {e:?}, {} events (reference {})",
            h * 1e6,
            coarse.stats.events.len(),
            reference.stats.events.len()
        );
        // Measured: amplitude 6.7e-5, phase 1.1e-3 deg, carrier average
        // 0.0117 A at 20 us (1.1e-4 of the rated peak).
        assert!(e.amplitude < 3e-4, "{e:?}");
        assert!(e.phase_deg < 5e-3, "{e:?}");
        assert!(e.average < 0.05, "{e:?}");
        assert_eq!(coarse.stats.instant_limit_steps, 0);
        // Every pulse is seen: 2 events fewer than the reference (1147 of
        // 1149), as in the table of the documents (2743 of 2745 in 60 ms,
        // and 2739 of 2741 without the dead time).
        let missing = reference.stats.events.len() - coarse.stats.events.len();
        assert!(missing <= 2, "{missing}");
        if let Some(l) = last {
            // Second order: about a quarter at half the step (the
            // reference at 1 µs has an error of 1/100 of that at 10 µs).
            let ratio = l.average / e.average;
            assert!(ratio > 2.5 && ratio < 6.0, "{ratio}");
        }
        last = Some(e);
    }
    // At 25 µs the narrowest pulses (22.4 µs) begin and end within one
    // step. They are found at the vertices of the carrier (see the
    // documents): every pulse is seen, as at 10 and 20 µs (before, 1135
    // events of 1149), and the error is that of the step.
    let h = 2.5e-5;
    let coarse = solve(case, h, "interpolated", end).run.unwrap();
    let e = errors(&coarse, h, &reference, (0.005, 0.025));
    let (n, n_ref) = (coarse.stats.events.len(), reference.stats.events.len());
    println!("h 25 us: {e:?}, {n} events (reference {n_ref})");
    // Measured: amplitude 1.1e-4, phase 1.8e-3 deg, carrier average
    // 0.0158 A (1.36 times that at 20 us; (25/20)^2 = 1.56).
    assert!(n + 2 >= n_ref, "{n} vs {n_ref}");
    assert!(e.amplitude < 3e-4, "{e:?}");
    assert!(e.phase_deg < 5e-3, "{e:?}");
    assert!(e.average < 0.05, "{e:?}");
    assert_eq!(coarse.stats.instant_limit_steps, 0);
}

#[test]
#[ignore]
fn table_of_the_errors_of_a_pwm_inverter_against_fine_steps() {
    let cases = [
        Case {
            control: Control::Open,
            dead_s: 0.0,
        },
        Case {
            control: Control::Open,
            dead_s: 2e-6,
        },
        Case {
            control: Control::Pi,
            dead_s: 0.0,
        },
        Case {
            control: Control::Pi,
            dead_s: 2e-6,
        },
    ];
    let peak = rated_peak();
    println!("rated peak {peak:.2} A, carrier {FC} Hz, window {T0}..{T1} s");
    for case in cases {
        let reference = solve(case, 2.5e-7, "interpolated", T1);
        let r = reference.run.as_ref().unwrap();
        println!(
            "\n{case:?}: reference h 0.25 us, {:.1} s for {T1} s, {} events",
            reference.seconds,
            r.stats.events.len()
        );
        println!("| switching | h (us) | row (A) | carrier average (A) | of rated peak | amplitude | phase (deg) | THD diff | events | most events in a step | W270 steps | error | s per s |");
        let mut rows = Vec::new();
        for (switching, steps) in [
            ("interpolated", vec![5e-7, 1e-6, 5e-6, 1e-5, 2e-5, 2.5e-5]),
            ("grid", vec![1e-6, 5e-6, 1e-5, 2e-5, 2.5e-5]),
        ] {
            for h in steps {
                let t = solve(case, h, switching, T1);
                match &t.run {
                    Ok(run) => {
                        let e = errors(run, h, r, (T0, T1));
                        println!(
                            "| {switching} | {:.2} | {:.3e} | {:.3e} | {:.2e} | {:.2e} | {:.2e} | {:.2e} | {} | {} | {} | — | {:.3} |",
                            h * 1e6,
                            e.row,
                            e.average,
                            e.average / peak,
                            e.amplitude,
                            e.phase_deg,
                            e.thd,
                            run.stats.events.len(),
                            events_per_step(run, h),
                            run.stats.instant_limit_steps,
                            t.seconds / T1
                        );
                        rows.push((switching, h, e));
                    }
                    Err(code) => println!(
                        "| {switching} | {:.2} | | | | | | | | | | {code} | |",
                        h * 1e6
                    ),
                }
            }
        }
        // The reference: the difference of 0.5 µs and 1 µs from it falls
        // as h^2 when the reference itself is far more accurate.
        let of = |h: f64| {
            rows.iter()
                .find(|r| r.0 == "interpolated" && r.1 == h)
                .map(|r| r.2)
        };
        if let (Some(a), Some(b)) = (of(5e-7), of(1e-6)) {
            println!(
                "ratio 1 us / 0.5 us: row {:.2}, carrier average {:.2}, amplitude {:.2}",
                b.row / a.row,
                b.average / a.average,
                b.amplitude / a.amplitude
            );
        }
        for pair in [(1e-5, 2e-5), (5e-6, 1e-5)] {
            if let (Some(a), Some(b)) = (of(pair.0), of(pair.1)) {
                println!(
                    "ratio {:.0} us / {:.0} us: row {:.2}, carrier average {:.2}, amplitude {:.2}, phase {:.2}",
                    pair.1 * 1e6,
                    pair.0 * 1e6,
                    b.row / a.row,
                    b.average / a.average,
                    b.amplitude / a.amplitude,
                    b.phase_deg / a.phase_deg
                );
            }
        }
    }
}
