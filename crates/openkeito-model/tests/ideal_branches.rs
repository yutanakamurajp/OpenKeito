//! Validation of loops of ideal branches (E142, W142) and of nodes whose
//! voltage nothing holds (E143, W143). `crates/openkeito-core/tests/
//! ideal_branches.rs` shows that the solver finds the same circuits
//! singular (E200).

use openkeito_model::{load_str, Diagnostic, Severity};
use serde_json::{json, Value};

fn model(components: Value) -> Value {
    json!({
        "format": "openkeito-model",
        "version": "0.5",
        "simulation": {"time_step_s": 1e-4, "end_time_s": 1e-2},
        "components": components,
    })
}

/// The errors of a model that is rejected.
fn errors(components: Value) -> Vec<Diagnostic> {
    load_str(&model(components).to_string()).expect_err("model must be rejected")
}

/// The warnings of a model that loads.
fn warnings(components: Value) -> Vec<Diagnostic> {
    match load_str(&model(components).to_string()) {
        Ok(loaded) => loaded.warnings,
        Err(e) => panic!("model must load: {e:#?}"),
    }
}

/// The one diagnostic of a model, which has the code `code`.
fn single(d: Vec<Diagnostic>, code: &str) -> Diagnostic {
    assert_eq!(d.len(), 1, "{d:#?}");
    assert_eq!(d[0].code, code, "{d:#?}");
    d.into_iter().next().unwrap()
}

fn dc(name: &str, nodes: [&str; 2], value: f64) -> Value {
    json!({"type": "voltage_source", "name": name, "nodes": nodes,
           "waveform": {"type": "dc", "value": value}})
}

fn current(name: &str, nodes: [&str; 2]) -> Value {
    json!({"type": "current_source", "name": name, "nodes": nodes,
           "waveform": {"type": "dc", "value": 1.0}})
}

fn resistor(name: &str, nodes: [&str; 2]) -> Value {
    json!({"type": "resistor", "name": name, "nodes": nodes, "resistance_ohm": 10.0})
}

fn three_phase_source(name: &str, bus: &str, impedance: bool) -> Value {
    let mut s = json!({"type": "three_phase_source", "name": name, "bus": bus,
                       "line_voltage_rms_v": 6600.0, "frequency_hz": 50.0});
    if impedance {
        s["impedance"] = json!({"type": "sequence", "r1_ohm": 0.1, "l1_h": 1e-3,
                                "r0_ohm": 0.1, "l0_h": 1e-3});
    }
    s
}

fn load(name: &str, bus: &str) -> Value {
    json!({"type": "three_phase_load", "name": name, "bus": bus, "rated_voltage_kv": 6.6,
           "rated_frequency_hz": 50.0, "active_power_mw": 1.0, "reactive_power_mvar": 0.0})
}

#[test]
fn two_voltage_sources_in_parallel_are_a_loop() {
    let d = single(
        errors(json!([
            dc("E1", ["n1", "gnd"], 1.0),
            dc("E2", ["n1", "gnd"], 1.0),
            resistor("R1", ["n1", "gnd"]),
        ])),
        "E142",
    );
    assert_eq!(d.severity, Severity::Error);
    assert_eq!(d.path, "components[0]");
    assert!(
        d.message
            .starts_with("\"E1\" (voltage_source) and \"E2\" (voltage_source) form a loop"),
        "{}",
        d.message
    );
    assert!(d.hint.contains("resistor in series"), "{}", d.hint);
    // With a resistor in series, as the hint says, the model loads.
    assert!(warnings(json!([
        dc("E1", ["n1", "gnd"], 1.0),
        dc("E2", ["n2", "gnd"], 1.0),
        resistor("R1", ["n1", "n2"]),
    ]))
    .is_empty());
}

#[test]
fn a_voltage_source_and_closed_switches_are_a_loop() {
    // E1 - S1 - S2 - gnd: the loop is named by all three, not by R1.
    let d = single(
        errors(json!([
            dc("E1", ["n1", "gnd"], 1.0),
            {"type": "switch", "name": "S1", "nodes": ["n1", "n2"], "initially_closed": true},
            resistor("R1", ["n2", "gnd"]),
            {"type": "switch", "name": "S2", "nodes": ["n2", "gnd"], "close_time_s": 0.0},
        ])),
        "E142",
    );
    for name in ["\"E1\"", "\"S1\"", "\"S2\""] {
        assert!(d.message.contains(name), "{}", d.message);
    }
    assert!(!d.message.contains("R1"), "{}", d.message);
    // Two closed switches in parallel share a current that nothing divides.
    let d = single(
        errors(json!([
            dc("E1", ["n1", "gnd"], 1.0),
            {"type": "switch", "name": "S1", "nodes": ["n1", "n2"], "initially_closed": true},
            {"type": "switch", "name": "S2", "nodes": ["n1", "n2"], "initially_closed": true},
            resistor("R1", ["n2", "gnd"]),
        ])),
        "E142",
    );
    assert!(d.message.starts_with("\"S1\" (switch) and \"S2\" (switch)"));
}

#[test]
fn two_ideal_three_phase_sources_on_one_bus_are_a_loop() {
    // One diagnostic for the three phases.
    let d = single(
        errors(json!([
            three_phase_source("S1", "b1", false),
            three_phase_source("S2", "b1", false),
            load("LD", "b1"),
        ])),
        "E142",
    );
    assert!(d.message.contains("\"S1\" (three_phase_source) and \"S2\""));
    assert!(d.hint.contains("\"impedance\""), "{}", d.hint);
    // With an impedance in one of them the current is shared.
    assert!(warnings(json!([
        three_phase_source("S1", "b1", false),
        three_phase_source("S2", "b1", true),
        load("LD", "b1"),
    ]))
    .is_empty());
}

#[test]
fn an_ideal_transformer_between_two_ideal_sources_is_a_loop() {
    let transformer = json!({"type": "three_phase_ideal_transformer", "name": "PST",
        "buses": ["b1", "b2"], "rated_voltage_kv": [6.6, 6.6], "phase_shift_deg": 30.0});
    let e = errors(json!([
        three_phase_source("S1", "b1", false),
        transformer,
        three_phase_source("S2", "b2", false),
        load("LD", "b2"),
    ]));
    // The three equations of the transformer are three loops of the same
    // components: one diagnostic.
    let d = single(e, "E142");
    for name in [
        "\"S1\"",
        "\"PST\" (three_phase_ideal_transformer)",
        "\"S2\"",
    ] {
        assert!(d.message.contains(name), "{}", d.message);
    }
    // Two ideal transformers in parallel: the voltages of the second are
    // those of the first.
    let d = single(
        errors(json!([
            three_phase_source("S1", "b1", false),
            {"type": "three_phase_ideal_transformer", "name": "T1", "buses": ["b1", "b2"],
             "rated_voltage_kv": [66.0, 6.6]},
            {"type": "three_phase_ideal_transformer", "name": "T2", "buses": ["b1", "b2"],
             "rated_voltage_kv": [66.0, 6.6]},
            load("LD", "b2"),
        ])),
        "E142",
    );
    assert!(d.message.contains("\"T1\"") && d.message.contains("\"T2\""));
    // One of them alone holds b2.
    assert!(warnings(json!([
        three_phase_source("S1", "b1", false),
        {"type": "three_phase_ideal_transformer", "name": "T1", "buses": ["b1", "b2"],
         "rated_voltage_kv": [66.0, 6.6], "phase_shift_deg": 30.0},
        load("LD", "b2"),
    ]))
    .is_empty());
}

#[test]
fn a_node_fed_only_by_current_sources_floats() {
    let d = single(
        errors(json!([
            dc("E1", ["n1", "gnd"], 1.0),
            resistor("R1", ["n1", "gnd"]),
            current("J1", ["gnd", "n2"]),
            current("J2", ["n2", "n1"]),
        ])),
        "E143",
    );
    assert_eq!(d.path, "components");
    assert!(d.message.contains("the nodes n2 at t = 0"), "{}", d.message);
    assert!(
        d.message
            .contains("\"J1\" (current_source) and \"J2\" (current_source)"),
        "{}",
        d.message
    );
    assert!(d.hint.contains("resistor to gnd"), "{}", d.hint);
    // The three-phase current sources hold no voltage either.
    let d = single(
        errors(json!([
            three_phase_source("S1", "b1", true),
            load("LD", "b1"),
            {"type": "three_phase_current_source", "name": "J1", "bus": "b2",
             "current_rms_a": 10.0, "frequency_hz": 50.0},
        ])),
        "E143",
    );
    assert!(d.message.contains("b2.a, b2.b, b2.c"), "{}", d.message);
}

#[test]
fn a_node_behind_an_open_switch_floats() {
    let d = single(
        errors(json!([
            dc("E1", ["n1", "gnd"], 1.0),
            resistor("R1", ["n1", "gnd"]),
            {"type": "switch", "name": "S1", "nodes": ["n1", "n2"]},
            current("J1", ["gnd", "n2"]),
        ])),
        "E143",
    );
    assert!(d.message.contains("the nodes n2 "), "{}", d.message);
    assert!(d.message.contains("\"S1\" (switch)"), "{}", d.message);
    // A switch that closes later does not hold the node at t = 0.
    let d = single(
        errors(json!([
            dc("E1", ["n1", "gnd"], 1.0),
            resistor("R1", ["n1", "gnd"]),
            {"type": "switch", "name": "S1", "nodes": ["n1", "n2"], "close_time_s": 1e-3},
            {"type": "inductor", "name": "L1", "nodes": ["n2", "n3"], "inductance_h": 1e-3},
        ])),
        "E143",
    );
    assert!(d.message.contains("the nodes n2, n3 "), "{}", d.message);
    // A switch that is commanded closed at t = 0 holds it from the start.
    assert!(warnings(json!([
        dc("E1", ["n1", "gnd"], 1.0),
        {"type": "switch", "name": "S1", "nodes": ["n1", "n2"], "close_time_s": 0.0},
        resistor("R1", ["n2", "gnd"]),
    ]))
    .is_empty());
}

#[test]
fn a_bus_with_only_a_fault_that_has_not_started_floats() {
    let fault = |start: f64| {
        json!({"type": "fault", "name": "F1", "bus": "b2", "phases": ["a", "b", "c"],
               "ground": true, "start_time_s": start})
    };
    let network = |start: f64| {
        json!([
            three_phase_source("S1", "b1", true),
            load("LD", "b1"),
            {"type": "three_phase_current_source", "name": "J1", "bus": "b2",
             "current_rms_a": 10.0, "frequency_hz": 50.0},
            fault(start),
        ])
    };
    let d = single(errors(network(2e-3)), "E143");
    assert!(d.message.contains("b2.a, b2.b, b2.c"), "{}", d.message);
    assert!(d.message.contains("\"F1\" (fault)"), "{}", d.message);
    // A fault from t = 0 on connects the bus to gnd.
    assert!(warnings(network(0.0)).is_empty());
}

#[test]
fn an_opening_that_leaves_a_node_floating_is_a_warning() {
    let d = single(
        warnings(json!([
            current("J1", ["gnd", "n1"]),
            {"type": "switch", "name": "S1", "nodes": ["n1", "n2"], "initially_closed": true,
             "open_time_s": 5e-4},
            resistor("R1", ["n2", "gnd"]),
        ])),
        "W143",
    );
    assert_eq!(d.severity, Severity::Warning);
    assert_eq!(d.path, "components[1]");
    assert!(
        d.message
            .starts_with("after the action of \"S1\" (switch) at t = 5e-4 s, nothing holds the voltage of the nodes n1"),
        "{}",
        d.message
    );
    // An action after the last step never takes place.
    assert!(warnings(json!([
        current("J1", ["gnd", "n1"]),
        {"type": "switch", "name": "S1", "nodes": ["n1", "n2"], "initially_closed": true,
         "open_time_s": 1e-2},
        resistor("R1", ["n2", "gnd"]),
    ]))
    .is_empty());
    // A fault that is cleared leaves its bus as it was before it started.
    let d = single(
        warnings(json!([
            three_phase_source("S1", "b1", true),
            load("LD", "b1"),
            {"type": "three_phase_current_source", "name": "J1", "bus": "b2",
             "current_rms_a": 10.0, "frequency_hz": 50.0},
            {"type": "fault", "name": "F1", "bus": "b2", "phases": ["a", "b", "c"],
             "ground": true, "start_time_s": 0.0, "clear_time_s": 2e-3},
        ])),
        "W143",
    );
    assert!(
        d.message.contains("\"F1\" (fault) at t = 2e-3 s"),
        "{}",
        d.message
    );
}

#[test]
fn a_closing_that_makes_a_loop_is_a_warning() {
    let network = |open_s1: f64| {
        json!([
            dc("E1", ["n1", "gnd"], 1.0),
            dc("E2", ["n2", "gnd"], 1.0),
            {"type": "switch", "name": "S1", "nodes": ["n1", "n3"], "initially_closed": true,
             "open_time_s": open_s1, "open_at_current_zero": false},
            {"type": "switch", "name": "S2", "nodes": ["n2", "n3"], "close_time_s": 2e-3},
            resistor("R1", ["n3", "gnd"]),
        ])
    };
    // S2 closes while S1 is closed: E1 - S1 - S2 - E2.
    let d = single(warnings(network(3e-3)), "W142");
    assert_eq!(d.path, "components[3]");
    assert!(
        d.message
            .starts_with("after the action of \"S2\" (switch) at t = 2e-3 s, "),
        "{}",
        d.message
    );
    for name in ["\"E1\"", "\"E2\"", "\"S1\"", "\"S2\""] {
        assert!(d.message.contains(name), "{}", d.message);
    }
    // S1 opens first: a transfer from one source to the other, no loop.
    assert!(warnings(network(1e-3)).is_empty());
    // A fault without resistance between the phases of an ideal source.
    let d = single(
        warnings(json!([
            three_phase_source("S1", "b1", false),
            load("LD", "b1"),
            {"type": "fault", "name": "F1", "bus": "b1", "phases": ["a", "b"],
             "start_time_s": 1e-3},
        ])),
        "W142",
    );
    assert!(d
        .message
        .contains("\"S1\" (three_phase_source) and \"F1\" (fault)"));
}

#[test]
fn components_with_a_conductance_hold_their_nodes() {
    // A node between capacitors has a voltage within a time step, and so
    // has one behind a diode or a controlled switch that blocks (their off
    // resistance) or behind an inductor.
    assert!(warnings(json!([
        dc("E1", ["n1", "gnd"], 1.0),
        {"type": "capacitor", "name": "C1", "nodes": ["n1", "n2"], "capacitance_f": 1e-6},
        {"type": "capacitor", "name": "C2", "nodes": ["n2", "gnd"], "capacitance_f": 1e-6},
        {"type": "diode", "name": "D1", "nodes": ["gnd", "n3"]},
        current("J1", ["gnd", "n3"]),
        {"type": "inductor", "name": "L1", "nodes": ["n1", "n4"], "inductance_h": 1e-3},
        current("J2", ["n4", "gnd"]),
    ]))
    .is_empty());
    // A transformer holds a secondary side that has its own connection to
    // gnd, and an ideal transformer a side that has none.
    assert!(warnings(json!([
        {"type": "voltage_source", "name": "E1", "nodes": ["p", "gnd"],
         "waveform": {"type": "sine", "amplitude": 100.0, "frequency_hz": 50.0}},
        {"type": "transformer", "name": "T1", "windings": [["p", "gnd"], ["s", "gnd"]],
         "rated_voltage_kv": [0.1, 0.05], "rated_power_mva": 0.001, "rated_frequency_hz": 50.0,
         "impedance_percent": 5.0},
        resistor("R1", ["s", "gnd"]),
    ]))
    .is_empty());
}

#[test]
fn an_action_just_after_the_start_is_not_judged_with_instants_within_steps() {
    // With "switching": "interpolated" the solver takes an action that
    // follows t = 0 by a small part of a step at t = 0. S1 closes 1 % of a
    // step after t = 0: whether n2 has a voltage in the first solution
    // depends on the solver, so it is no error. With switching at grid
    // points S1 closes at the first step, and n2 floats until then.
    let circuit = json!([
        dc("E1", ["n1", "gnd"], 1.0),
        resistor("R1", ["n1", "gnd"]),
        {"type": "switch", "name": "S1", "nodes": ["n1", "n2"], "close_time_s": 1e-6},
        current("J1", ["gnd", "n2"]),
    ]);
    let mut interpolated = model(circuit.clone());
    interpolated["simulation"]["switching"] = "interpolated".into();
    let loaded = load_str(&interpolated.to_string());
    assert!(loaded.is_ok_and(|l| l.warnings.is_empty()));
    single(errors(circuit), "E143");
}
