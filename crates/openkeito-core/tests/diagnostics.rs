//! What the validation says and what a run or a scan says agree: a model
//! that passes the validation does not stop on something the validation
//! could have named, and a warning of the scan is one of the validation.

use openkeito_core::{FrequencyScanner, Simulator};
use openkeito_model::{Diagnostic, Severity};
use serde_json::{json, Value};

/// The errors and the warnings of the validation.
fn validated(model: &Value) -> (Vec<Diagnostic>, Vec<Diagnostic>) {
    match openkeito_model::load_str(&model.to_string()) {
        Ok(loaded) => (Vec::new(), loaded.warnings),
        Err(d) => d.into_iter().partition(|d| d.severity == Severity::Error),
    }
}

/// The error with which the run of a model that passed the validation
/// stops, if it does.
fn run_error(model: &Value) -> Option<Diagnostic> {
    let loaded = openkeito_model::load_str(&model.to_string()).expect("model is valid");
    let sim = Simulator::from_model(&loaded.model).expect("circuit builds");
    sim.run(|_, _| Ok(())).err().map(|e| e.to_diagnostic())
}

// ------------------------------------------------- E139 before E204

/// A source behind a resistor and a coupled branch to gnd whose sequences
/// differ: the same resistance in both, and the inductances `l1` and `l0`.
/// At the smallest stage coefficient (2 / h) the resistance decides and the
/// phase matrix R + alpha L is well conditioned; at the largest (1e6 / h, at
/// t = 0+) the inductances decide, with a ratio of l0 / l1.
fn uneven_branch(simulation: Value, l1: f64, l0: f64) -> Value {
    json!({
        "format": "openkeito-model",
        "version": "0.5",
        "simulation": simulation,
        "components": [
            {"type": "three_phase_source", "name": "G1", "bus": "s", "line_voltage_rms_v": 400.0,
             "frequency_hz": 50.0,
             "impedance": {"type": "sequence", "r1_ohm": 0.1, "l1_h": 1e-3, "r0_ohm": 0.1, "l0_h": 1e-3}},
            {"type": "coupled_rl", "name": "Z1", "buses": ["s", "gnd"],
             "impedance": {"type": "sequence", "r1_ohm": 1.0, "l1_h": l1, "r0_ohm": 1.0, "l0_h": l0}},
        ],
        "outputs": [{"type": "bus_voltage", "bus": "s"}],
    })
}

#[test]
fn a_branch_that_cannot_be_solved_at_some_stage_coefficient_is_found_by_the_validation() {
    // h = 1e-4 s: alpha is 2e4 / s in the half steps and 1e10 / s at t = 0+.
    // With l1 = 1e-12 H and l0 = 1e5 H the ratio between the sequences is
    // 2e9 at the first (the resistance of 1 ohm decides the positive
    // sequence) and 1e15 at the second: the validation used to look at the
    // first only, and the run then stopped with E204.
    let simulation = json!({"time_step_s": 1e-4, "end_time_s": 1e-3});
    let (errors, _) = validated(&uneven_branch(simulation.clone(), 1e-12, 1e5));
    assert_eq!(errors.len(), 1, "{errors:#?}");
    let d = &errors[0];
    assert_eq!(
        (d.code, d.path.as_str()),
        ("E139", "components[1].impedance")
    );
    // The message names the stage coefficient that fails and their range.
    assert!(d.message.contains("alpha = 1.000e10 / s"), "{}", d.message);
    assert!(
        d.message.contains("from 2.000e4 to 1.000e10 / s"),
        "{}",
        d.message
    );
    assert!(
        d.hint.contains("the inductances at the largest"),
        "{}",
        d.hint
    );

    // The same ratio in the resistances and the inductances is what the
    // check has always found.
    let even =
        json!({"type": "sequence", "r1_ohm": 1.0, "l1_h": 1e-3, "r0_ohm": 1e15, "l0_h": 1e12});
    let mut model = uneven_branch(simulation, 1e-3, 1e-3);
    model["components"][1]["impedance"] = even;
    let (errors, _) = validated(&model);
    assert_eq!(errors.len(), 1, "{errors:#?}");
    assert_eq!(errors[0].code, "E139");
}

#[test]
fn no_model_that_passes_the_validation_stops_with_e204() {
    // l0 / l1 from 1e9 to 1e17 in steps of a factor of 10^(1/4), across
    // both borders (the validation refuses below a pivot of 1e-12, the
    // solver below 1e-14), for both methods, both places of the switching
    // instants and three time steps. Every model is either refused by the
    // validation with E139 or runs to its end.
    let mut refused = 0;
    let mut completed = 0;
    for method in ["2s-dirk", "trapezoidal"] {
        for switching in ["grid", "interpolated"] {
            for h in [1e-6, 1e-4, 1e-3] {
                for quarter_decades in 36..=68 {
                    let ratio = 10f64.powf(f64::from(quarter_decades) / 4.0);
                    let simulation = json!({"time_step_s": h, "end_time_s": 3.0 * h,
                                            "method": method, "switching": switching});
                    let model = uneven_branch(simulation, 1e-12, 1e-12 * ratio);
                    let (errors, _) = validated(&model);
                    if errors.is_empty() {
                        let error = run_error(&model);
                        assert!(
                            error.is_none(),
                            "{method}, {switching}, h = {h}, l0/l1 = {ratio:e}: the validation \
                             passes and the run stops with {error:#?}"
                        );
                        completed += 1;
                    } else {
                        let codes: Vec<&str> = errors.iter().map(|d| d.code).collect();
                        assert_eq!(codes, ["E139"], "{method}, {switching}, h = {h}, {ratio:e}");
                        refused += 1;
                    }
                }
            }
        }
    }
    // Both sides of the border are in the sweep.
    assert!(
        refused > 100 && completed > 100,
        "{refused} refused, {completed} completed"
    );
}

// ------------------------------------------------------ W192 before a scan

fn rectifier_with_a_scan(scan: bool) -> Value {
    let mut model = json!({
        "format": "openkeito-model",
        "version": "0.5",
        "simulation": {"time_step_s": 1e-5, "end_time_s": 1e-3},
        "components": [
            {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"],
             "waveform": {"type": "sine", "amplitude": 100.0, "frequency_hz": 50.0}},
            {"type": "resistor", "name": "R1", "nodes": ["n1", "n2"], "resistance_ohm": 1.0},
            {"type": "diode", "name": "D1", "nodes": ["n2", "n3"]},
            {"type": "controlled_switch", "name": "S1", "nodes": ["n3", "gnd"], "gate": "g"},
            {"type": "resistor", "name": "R2", "nodes": ["n3", "gnd"], "resistance_ohm": 10.0},
        ],
        "controls": [{"type": "constant", "name": "g", "value": 0.0}],
    });
    if scan {
        model["frequency_scan"] =
            json!({"start_hz": 10.0, "stop_hz": 1000.0, "points": 3, "nodes": ["n3"]});
    }
    model
}

#[test]
fn the_validation_gives_the_warning_of_a_scan_with_diodes_and_switches() {
    let model = rectifier_with_a_scan(true);
    let (errors, warnings) = validated(&model);
    assert!(errors.is_empty(), "{errors:#?}");
    let of_validation: Vec<&Diagnostic> = warnings.iter().filter(|d| d.code == "W192").collect();
    assert_eq!(of_validation.len(), 1, "{warnings:#?}");
    let d = of_validation[0];
    assert!(
        d.message.ends_with("in the frequency scan: D1, S1"),
        "{}",
        d.message
    );
    assert!(d.hint.contains("resistor or a closed switch"), "{}", d.hint);

    // The scan gives the same warning, once, and scans the blocking
    // resistances: n3 sees R2 = 10 ohm beside S1 (1e6 ohm) and beside D1
    // with R1 (1e6 + 1 ohm).
    let loaded = openkeito_model::load_str(&model.to_string()).unwrap();
    let scanner = FrequencyScanner::from_model(&loaded.model).expect("scan builds");
    let mut rows = Vec::new();
    let stats = scanner
        .run(|_, values| {
            rows.push(values.to_vec());
            Ok(())
        })
        .expect("scan completes");
    let of_scan: Vec<&Diagnostic> = stats.warnings.iter().filter(|d| d.code == "W192").collect();
    assert_eq!(of_scan.len(), 1, "{:#?}", stats.warnings);
    assert_eq!(
        (of_scan[0].path.as_str(), &of_scan[0].message),
        (d.path.as_str(), &d.message)
    );
    let expected = 1.0 / (1.0 / 10.0 + 1.0 / 1e6 + 1.0 / (1e6 + 1.0));
    let z = rows[0][0].expect("not singular");
    assert!(
        (z - expected).abs() < 1e-9 * expected,
        "{z} against {expected}"
    );

    // A run of the same model carries the warning of the validation, and
    // nothing else of the scan.
    assert!(run_error(&model).is_none());

    // Without a scan there is nothing to warn of.
    let (errors, warnings) = validated(&rectifier_with_a_scan(false));
    assert!(errors.is_empty(), "{errors:#?}");
    assert!(warnings.iter().all(|d| d.code != "W192"), "{warnings:#?}");
}
