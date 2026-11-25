//! The currents and branch voltages that the model declares for a component
//! (`Component::current_suffixes`, `voltage_suffixes`) are those the circuit
//! is built with. Validation checks the `terminal` and `phase` of a
//! measurement against the declaration, so a type that does not declare its
//! currents, or declares others, fails here. The same for the signals it
//! offers (`Component::signal_names`).

use serde_json::{json, Value};

use super::build;
use openkeito_model::Model;

/// A model of `components`, with a constant block for every signal they read.
fn model_of(components: Vec<Value>) -> Model {
    try_model_of(components).expect("the model parses")
}

fn try_model_of(components: Vec<Value>) -> serde_json::Result<Model> {
    let mut signals: Vec<String> = Vec::new();
    for c in &components {
        for key in [
            "signal",
            "gate",
            "field_voltage_signal",
            "mechanical_torque_signal",
        ] {
            signals.extend(c.get(key).and_then(Value::as_str).map(str::to_owned));
        }
        for s in c
            .get("signals")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            signals.extend(s.as_str().map(str::to_owned));
        }
    }
    signals.sort();
    signals.dedup();
    let controls: Vec<Value> = signals
        .iter()
        .map(|s| json!({"type": "constant", "name": s, "value": 0.0}))
        .collect();
    serde_json::from_value(json!({
        "format": "openkeito-model",
        "version": "0.5",
        "simulation": {"time_step_s": 1e-5, "end_time_s": 1e-3},
        "components": components,
        "controls": controls,
    }))
}

/// Compares the declaration of every component of `m` with its expansion.
fn compare(m: &Model, origin: &str) {
    let net = build(m);
    for c in &m.components {
        let what = format!("{origin}: \"{}\" ({})", c.name(), c.type_name());
        let built: Vec<String> = net.currents[c.name()]
            .iter()
            .map(|q| q.suffix.clone())
            .collect();
        assert_eq!(
            c.current_suffixes(),
            Some(built),
            "{what}: ComponentSpec::current_suffixes is not the currents of the expansion"
        );
        let built: Vec<String> = net
            .voltages
            .get(c.name())
            .into_iter()
            .flatten()
            .map(|q| q.suffix.clone())
            .collect();
        assert_eq!(
            c.voltage_suffixes(),
            built,
            "{what}: has_branch_voltage is not the branch voltages of the expansion"
        );
        // The signals a measurement may read as `component_signal`.
        let prefix = format!("{}.", c.name());
        let mut built: Vec<&str> = net
            .roles
            .signals
            .iter()
            .filter_map(|s| s.name.strip_prefix(&prefix))
            .collect();
        built.sort_unstable();
        let mut declared = c.signal_names();
        declared.sort_unstable();
        assert_eq!(
            declared, built,
            "{what}: ComponentSpec::signal_names is not the signals of its elements"
        );
    }
}

#[test]
fn every_component_type_declares_the_currents_and_voltages_it_is_built_with() {
    let mut count = 0;
    for info in openkeito_model::component_catalog() {
        if info.category != "circuit" {
            continue;
        }
        compare(&model_of(vec![info.example.clone()]), info.type_name);
        count += 1;
    }
    assert!(count >= 24, "{count} component types");
}

#[test]
fn the_declared_currents_follow_the_parameters_that_change_them() {
    // The currents of a fault are those of its phases, and of the ground
    // path when it has one.
    let fault = |phases: Value, ground: bool| {
        json!({"type": "fault", "name": "F1", "bus": "b1", "phases": phases, "ground": ground,
               "fault_resistance_ohm": 0.1, "ground_resistance_ohm": 1.0,
               "start_time_s": 0.01, "clear_time_s": 0.1})
    };
    for (phases, ground, expected) in [
        (json!(["a"]), true, vec![".a", ".g"]),
        (json!(["b", "c"]), false, vec![".b", ".c"]),
        (json!(["a", "b", "c"]), true, vec![".a", ".b", ".c", ".g"]),
        (json!(["a", "b", "c"]), false, vec![".a", ".b", ".c"]),
    ] {
        let m = model_of(vec![fault(phases, ground)]);
        compare(&m, "fault");
        assert_eq!(m.components[0].current_suffixes().unwrap(), expected);
    }
    // A catalog example with an optional part removed (when it is optional).
    for info in openkeito_model::component_catalog() {
        let mut example = info.example.clone();
        let fields = example.as_object_mut().unwrap();
        let removed = ["saturation", "impedance", "steady_state"]
            .iter()
            .filter(|key| fields.remove(**key).is_some())
            .count();
        if info.category == "circuit" && removed > 0 {
            if let Ok(m) = try_model_of(vec![example]) {
                compare(&m, info.type_name);
            }
        }
    }
}

#[test]
fn the_components_of_the_examples_declare_what_they_are_built_with() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let mut count = 0;
    for entry in std::fs::read_dir(dir).expect("the examples are there") {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        // Batch files and schedules are not models.
        if openkeito_model::document_format(&text).as_deref() != Some("openkeito-model") {
            continue;
        }
        let loaded = openkeito_model::load_str(&text).expect("the example is valid");
        compare(&loaded.model, &path.display().to_string());
        count += 1;
    }
    assert!(count >= 15, "{count} examples");
}
