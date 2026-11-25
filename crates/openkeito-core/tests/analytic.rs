//! Comparison of simulated waveforms with closed-form solutions.

mod common;

use common::phasors::METHODS;
use common::{run_model as run, Run};
use openkeito_core::{SimError, Simulator};
use serde_json::{json, Value};

fn build(model: &Value) -> Simulator {
    let loaded = openkeito_model::load_str(&model.to_string()).expect("model is valid");
    Simulator::from_model(&loaded.model).expect("circuit builds")
}

fn model(method: &str, h: f64, end: f64, components: Value, outputs: Value) -> Value {
    json!({
        "format": "openkeito-model",
        "version": "0.1",
        "simulation": {"time_step_s": h, "end_time_s": end, "method": method},
        "components": components,
        "outputs": outputs,
    })
}

/// Largest |simulated - exact| of one column over the run.
fn max_error(r: &Run, column: usize, exact: impl Fn(f64) -> f64) -> f64 {
    r.t.iter()
        .zip(&r.rows)
        .map(|(t, row)| (row[column] - exact(*t)).abs())
        .fold(0.0, f64::max)
}

fn rc(method: &str, h: f64) -> Run {
    run(&model(
        method,
        h,
        5e-3,
        json!([
            {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {"type": "dc", "value": 1.0}},
            {"type": "resistor", "name": "R1", "nodes": ["n1", "n2"], "resistance_ohm": 1000.0},
            {"type": "capacitor", "name": "C1", "nodes": ["n2", "gnd"], "capacitance_f": 1e-6},
        ]),
        json!([
            {"type": "node_voltage", "node": "n2"},
            {"type": "branch_current", "component": "C1"},
            {"type": "branch_current", "component": "E1"},
        ]),
    ))
}

#[test]
fn rc_charging_matches_the_exponential_with_second_order_accuracy() {
    let tau = 1e-3;
    for method in METHODS {
        let coarse = rc(method, 1e-5);
        let fine = rc(method, 5e-6);
        assert_eq!(coarse.rows.len(), 501);
        assert_eq!(coarse.t[0], 0.0);
        assert!((coarse.t[500] - 5e-3).abs() < 1e-15);

        let v = |t: f64| 1.0 - (-t / tau).exp();
        let i = |t: f64| 1e-3 * (-t / tau).exp();
        let (ev, ev2) = (max_error(&coarse, 0, v), max_error(&fine, 0, v));
        assert!(ev < 2e-5, "{method}: voltage error {ev}");
        let ratio = ev / ev2;
        assert!(
            (3.3..4.7).contains(&ratio),
            "{method}: halving the step changed the error by {ratio}, expected about 4"
        );
        let ei = max_error(&coarse, 1, i);
        assert!(ei < 2e-7, "{method}: current error {ei}");

        // First row is t = 0+ (at most 2e-6 of a step after 0): the capacitor
        // is uncharged and carries E/R.
        assert!(coarse.rows[0][0].abs() < 1e-7);
        assert!((coarse.rows[0][1] - 1e-3).abs() < 1e-9);
        // The source delivers power, so its branch current is negative.
        assert!((coarse.rows[0][2] + 1e-3).abs() < 1e-9);
    }
}

fn rlc(method: &str, h: f64) -> Run {
    run(&model(
        method,
        h,
        5e-3,
        json!([
            {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {"type": "dc", "value": 1.0}},
            {"type": "resistor", "name": "R1", "nodes": ["n1", "n2"], "resistance_ohm": 10.0},
            {"type": "inductor", "name": "L1", "nodes": ["n2", "n3"], "inductance_h": 0.01},
            {"type": "capacitor", "name": "C1", "nodes": ["n3", "gnd"], "capacitance_f": 1e-6},
        ]),
        json!([
            {"type": "node_voltage", "node": "n3"},
            {"type": "branch_current", "component": "L1"},
        ]),
    ))
}

#[test]
fn series_rlc_step_response_matches_the_damped_oscillation() {
    let (r, l, c): (f64, f64, f64) = (10.0, 0.01, 1e-6);
    let a = r / (2.0 * l);
    let wd = (1.0 / (l * c) - a * a).sqrt();
    let v = |t: f64| 1.0 - (-a * t).exp() * ((wd * t).cos() + a / wd * (wd * t).sin());
    let i = |t: f64| (-a * t).exp() * (wd * t).sin() / (l * wd);
    for method in METHODS {
        let coarse = rlc(method, 2e-6);
        let fine = rlc(method, 1e-6);
        let (ev, ev2) = (max_error(&coarse, 0, v), max_error(&fine, 0, v));
        assert!(ev < 2e-3, "{method}: voltage error {ev}");
        let ratio = ev / ev2;
        assert!(
            (3.3..4.7).contains(&ratio),
            "{method}: halving the step changed the error by {ratio}, expected about 4"
        );
        // Peak current is about 1/(L*wd) = 0.01 A.
        let ei = max_error(&fine, 1, i);
        assert!(ei < 1e-5, "{method}: current error {ei}");
    }
}

#[test]
fn rl_energized_by_a_sine_source_shows_the_dc_offset() {
    let (r, l, f): (f64, f64, f64) = (1.0, 0.01, 50.0);
    let w = 2.0 * std::f64::consts::PI * f;
    let z = (r * r + w * w * l * l).sqrt();
    let phi = (w * l / r).atan();
    let i = |t: f64| 100.0 / z * ((w * t - phi).sin() + phi.sin() * (-r / l * t).exp());
    for method in METHODS {
        let out = run(&model(
            method,
            2e-5,
            0.06,
            json!([
                {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {"type": "sine", "amplitude": 100.0, "frequency_hz": f}},
                {"type": "resistor", "name": "R1", "nodes": ["n1", "n2"], "resistance_ohm": r},
                {"type": "inductor", "name": "L1", "nodes": ["n2", "gnd"], "inductance_h": l},
            ]),
            json!([{"type": "branch_current", "component": "L1"}]),
        ));
        // Peak current is about 30 A (58 A with the offset).
        let e = max_error(&out, 0, i);
        assert!(e < 2e-3, "{method}: current error {e}");
    }
}

#[test]
fn closing_switch_energizes_an_rl_branch() {
    let i = |t: f64| {
        if t < 1e-3 {
            0.0
        } else {
            5.0 * (1.0 - (-200.0 * (t - 1e-3)).exp())
        }
    };
    for method in METHODS {
        let out = run(&model(
            method,
            1e-5,
            0.02,
            json!([
                {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {"type": "dc", "value": 10.0}},
                {"type": "switch", "name": "SW1", "nodes": ["n1", "n2"], "close_time_s": 1e-3},
                {"type": "resistor", "name": "R1", "nodes": ["n2", "n3"], "resistance_ohm": 2.0},
                {"type": "inductor", "name": "L1", "nodes": ["n3", "gnd"], "inductance_h": 0.01},
            ]),
            json!([{"type": "branch_current", "component": "SW1"}]),
        ));
        let e = max_error(&out, 0, i);
        assert!(e < 1e-5, "{method}: current error {e}");
        assert_eq!(out.stats.events.len(), 1);
        assert_eq!(out.stats.events[0].action, "close");
        assert!((out.stats.events[0].time_s - 1e-3).abs() < 1e-12);
        // t = 0 and the closing each need a factorization; the first output
        // row adds one.
        assert_eq!(out.stats.factorizations, 3);
    }
}

#[test]
fn switch_opens_at_the_first_current_zero_after_the_command() {
    let h = 1e-5;
    for method in METHODS {
        let out = run(&model(
            method,
            h,
            0.04,
            json!([
                {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {"type": "sine", "amplitude": 100.0, "frequency_hz": 50.0}},
                {"type": "switch", "name": "SW1", "nodes": ["n1", "n2"], "initially_closed": true, "open_time_s": 0.012},
                {"type": "resistor", "name": "R1", "nodes": ["n2", "n3"], "resistance_ohm": 1.0},
                {"type": "inductor", "name": "L1", "nodes": ["n3", "gnd"], "inductance_h": 0.01},
            ]),
            json!([
                {"type": "branch_current", "component": "SW1"},
                {"type": "node_voltage", "node": "n2"},
            ]),
        ));
        assert_eq!(out.stats.events.len(), 1, "{method}");
        let event = &out.stats.events[0];
        assert_eq!(event.action, "open");
        assert!(event.time_s > 0.012 && event.time_s < 0.012 + 0.011);
        let k = (event.time_s / h).round() as usize;
        // The current changed sign in the step that ended at the opening.
        assert!(
            out.rows[k][0] * out.rows[k - 1][0] <= 0.0,
            "{method}: no zero crossing at the opening"
        );
        // No earlier zero crossing after the command.
        let first = (0.012 / h).round() as usize;
        for j in first + 1..k {
            assert!(out.rows[j][0] * out.rows[j - 1][0] > 0.0, "{method}");
        }
        // The interrupted current is at most one step's change (peak 30 A).
        let chopped = event.interrupted_current_a.unwrap();
        assert_eq!(chopped, out.rows[k][0]);
        assert!(
            chopped.abs() < 30.3 * 314.2 * h * 1.2,
            "{method}: {chopped}"
        );
        // Afterwards the current is zero and the load side settles at once.
        for row in &out.rows[k + 1..] {
            assert_eq!(row[0], 0.0);
        }
        for row in &out.rows[k + 3..] {
            assert!(row[1].abs() < 1e-6, "{method}: {}", row[1]);
        }
    }
}

#[test]
fn chopping_an_inductor_current_leaves_no_numerical_oscillation() {
    let h = 1e-5;
    for method in METHODS {
        let out = run(&model(
            method,
            h,
            0.01,
            json!([
                {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {"type": "dc", "value": 10.0}},
                {"type": "resistor", "name": "R1", "nodes": ["n1", "n2"], "resistance_ohm": 1.0},
                {"type": "switch", "name": "SW1", "nodes": ["n2", "n3"], "initially_closed": true, "open_time_s": 5e-3, "open_at_current_zero": false},
                {"type": "inductor", "name": "L1", "nodes": ["n3", "gnd"], "inductance_h": 0.01},
            ]),
            json!([
                {"type": "node_voltage", "node": "n3"},
                {"type": "branch_current", "component": "L1"},
            ]),
        ));
        let event = &out.stats.events[0];
        assert!((event.time_s - 5e-3).abs() < 1e-12);
        let i_before = 10.0 * (1.0 - (-0.5_f64).exp());
        assert!((event.interrupted_current_a.unwrap() - i_before).abs() < 1e-4);
        let k = (event.time_s / h).round() as usize;
        for row in &out.rows[k + 2..] {
            assert!(row[0].abs() < 1e-6, "{method}: voltage {}", row[0]);
            assert!(row[1].abs() < 1e-9, "{method}: current {}", row[1]);
        }
    }
}

#[test]
fn initial_conditions_are_used() {
    // A charged capacitor discharging into a resistor.
    for method in METHODS {
        let out = run(&model(
            method,
            1e-5,
            3e-3,
            json!([
                {"type": "capacitor", "name": "C1", "nodes": ["n1", "gnd"], "capacitance_f": 1e-6, "initial_voltage_v": 5.0},
                {"type": "resistor", "name": "R1", "nodes": ["n1", "gnd"], "resistance_ohm": 1000.0},
            ]),
            json!([{"type": "node_voltage", "node": "n1"}]),
        ));
        assert!((out.rows[0][0] - 5.0).abs() < 1e-7);
        let e = max_error(&out, 0, |t| 5.0 * (-t / 1e-3).exp());
        assert!(e < 1e-4, "{method}: {e}");
    }
}

#[test]
fn a_node_fed_only_by_a_current_source_is_reported_by_name() {
    // The switch opens at the first step: with a switch that is open from
    // the start, validation rejects the model (E143) before the run.
    let m = model(
        "2s-dirk",
        1e-5,
        1e-3,
        json!([
            {"type": "current_source", "name": "J1", "nodes": ["gnd", "n1"], "waveform": {"type": "dc", "value": 1.0}},
            {"type": "switch", "name": "SW1", "nodes": ["n1", "n2"], "initially_closed": true,
             "open_time_s": 1e-5, "open_at_current_zero": false},
            {"type": "resistor", "name": "R1", "nodes": ["n2", "gnd"], "resistance_ohm": 1.0},
        ]),
        json!([{"type": "node_voltage", "node": "n1"}]),
    );
    let err = build(&m).run(|_, _| Ok(())).unwrap_err();
    match &err {
        SimError::Singular { unknown, time_s } => {
            assert_eq!(unknown, "V(n1)");
            assert_eq!(*time_s, 1e-5);
        }
        other => panic!("unexpected error {other:?}"),
    }
    let d = err.to_diagnostic();
    assert_eq!(d.code, "E200");
    assert!(d.hint.contains("resistor"));
}

#[test]
fn default_outputs_are_all_node_voltages_and_branch_currents() {
    let mut m = model(
        "2s-dirk",
        1e-5,
        1e-4,
        json!([
            {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {"type": "dc", "value": 1.0}},
            {"type": "resistor", "name": "R1", "nodes": ["n1", "gnd"], "resistance_ohm": 2.0},
        ]),
        json!(null),
    );
    m.as_object_mut().unwrap().remove("outputs");
    let sim = build(&m);
    let names: Vec<&str> = sim.columns().iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["V(n1)", "I(E1)", "I(R1)"]);
}

/// A voltage step at T into a series RL: i(t) = (E/R) (1 - e^{-(t - T) R/L})
/// for t > T and 0 before. The step acts at its grid point like a closing
/// switch: the row at T still shows the state before it.
#[test]
fn a_step_source_acts_at_its_grid_point_like_a_switch() {
    let (e, r, l, t_step) = (100.0, 1.0, 0.01, 1e-3);
    let exact = |t: f64| {
        if t > t_step {
            e / r * (1.0 - (-(t - t_step) * r / l).exp())
        } else {
            0.0
        }
    };
    let step = |method: &str, h: f64| {
        run(&model(
            method,
            h,
            3e-3,
            json!([
                {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"],
                 "waveform": {"type": "step", "time_s": t_step, "before": 0.0, "after": e}},
                {"type": "resistor", "name": "R1", "nodes": ["n1", "n2"], "resistance_ohm": r},
                {"type": "inductor", "name": "L1", "nodes": ["n2", "gnd"], "inductance_h": l},
            ]),
            json!([{"type": "branch_current", "component": "L1"}]),
        ))
    };
    for method in METHODS {
        let coarse = step(method, 1e-4);
        let fine = step(method, 5e-5);
        // Nothing flows up to and including the row at the time of the step.
        assert!(coarse
            .t
            .iter()
            .zip(&coarse.rows)
            .all(|(t, row)| *t > t_step + 1e-12 || row[0] == 0.0));
        let (e1, e2) = (max_error(&coarse, 0, exact), max_error(&fine, 0, exact));
        // Second order from the first step on; an early step would leave an
        // error of the first order (0.3 A at this step size).
        assert!(e1 < 5e-3, "{method}: error {e1}");
        let ratio = e1 / e2;
        assert!((3.0..5.0).contains(&ratio), "{method}: ratio {ratio}");
        assert_eq!(coarse.stats.events.len(), 1);
        let event = &coarse.stats.events[0];
        assert_eq!((event.component.as_str(), event.action), ("E1", "step"));
        assert!((event.time_s - t_step).abs() < 1e-12);
    }
}

/// A direct current never crosses zero: a switch told to open at a current
/// zero stays closed, and the run says so.
#[test]
fn an_opening_that_never_finds_a_current_zero_is_a_warning() {
    let components = |at_zero: bool| {
        json!([
            {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {"type": "dc", "value": 10.0}},
            {"type": "switch", "name": "S1", "nodes": ["n1", "n2"], "initially_closed": true,
             "open_time_s": 1e-3, "open_at_current_zero": at_zero},
            {"type": "resistor", "name": "R1", "nodes": ["n2", "gnd"], "resistance_ohm": 5.0},
        ])
    };
    let outputs = json!([{"type": "branch_current", "component": "R1"}]);
    let waiting = run(&model(
        "2s-dirk",
        1e-4,
        5e-3,
        components(true),
        outputs.clone(),
    ));
    assert!(waiting.stats.events.is_empty());
    assert_eq!(waiting.rows.last().unwrap()[0], 2.0);
    let codes: Vec<&str> = waiting.stats.warnings.iter().map(|w| w.code).collect();
    assert_eq!(codes, ["W221"]);
    assert_eq!(waiting.stats.warnings[0].path, "S1");

    let forced = run(&model("2s-dirk", 1e-4, 5e-3, components(false), outputs));
    assert_eq!(forced.stats.events.len(), 1);
    assert!(forced.stats.warnings.is_empty());
    assert_eq!(forced.rows.last().unwrap()[0], 0.0);
}

/// A run that fails keeps the events up to the failure: here the opening
/// that leaves a node fed only by a current source.
#[test]
fn a_failed_run_keeps_its_events() {
    let m = model(
        "2s-dirk",
        1e-4,
        2e-3,
        json!([
            {"type": "current_source", "name": "J1", "nodes": ["gnd", "n1"], "waveform": {"type": "dc", "value": 1.0}},
            {"type": "switch", "name": "S1", "nodes": ["n1", "n2"], "initially_closed": true,
             "open_time_s": 5e-4, "open_at_current_zero": false},
            {"type": "resistor", "name": "R1", "nodes": ["n2", "gnd"], "resistance_ohm": 5.0},
        ]),
        json!([{"type": "node_voltage", "node": "n1"}]),
    );
    let (stats, result) = build(&m).run_with_stats(|_, _| Ok(()));
    assert!(matches!(result, Err(SimError::Singular { .. })));
    assert_eq!(stats.events.len(), 1);
    assert_eq!(stats.events[0].component, "S1");
    assert_eq!(stats.steps, 5);
}
