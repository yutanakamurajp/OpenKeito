//! The terminals of the components and the document `topology` is made of.

use openkeito_model::{component_catalog, topology, Component, Control, Model, GROUND};
use serde_json::json;

/// The example of every component type of the catalog.
fn catalog_components() -> Vec<Component> {
    component_catalog()
        .into_iter()
        .filter(|info| info.category == "circuit")
        .map(|info| {
            serde_json::from_value(info.example.clone())
                .unwrap_or_else(|e| panic!("{}: {e}", info.type_name))
        })
        .collect()
}

/// The terminals of every component type are its nodes, in their order.
/// Beyond them a terminal may only be at gnd: a star point that the model
/// leaves at its default.
#[test]
fn the_terminals_of_every_type_are_its_nodes() {
    let components = catalog_components();
    assert_eq!(components.len(), 31);
    for c in &components {
        let nodes = c.node_names();
        let of_terminals: Vec<String> = c.terminals().into_iter().flat_map(|t| t.nodes).collect();
        let mut expected = nodes.iter();
        let mut next = expected.next();
        for n in &of_terminals {
            if next == Some(n) {
                next = expected.next();
            } else {
                assert_eq!(n, GROUND, "{}: {of_terminals:?} / {nodes:?}", c.type_name());
            }
        }
        assert_eq!(
            next,
            None,
            "{}: {of_terminals:?} / {nodes:?}",
            c.type_name()
        );
        // The name of a terminal is a field of the component.
        let written = serde_json::to_value(c).unwrap();
        for t in c.terminals() {
            let field = t.name.split('[').next().unwrap();
            let defaulted = ["neutral", "hv_neutral", "lv_neutral", "ground"].contains(&field);
            assert!(
                written.get(field).is_some() || defaulted,
                "{}: no field {field}",
                c.type_name()
            );
            assert_eq!(
                t.phases.is_some(),
                t.nodes.len() != 1 || field.starts_with("bus")
            );
        }
    }
}

fn model(components: serde_json::Value, controls: serde_json::Value) -> Model {
    serde_json::from_value(json!({
        "format": "openkeito-model", "version": "0.5",
        "simulation": {"time_step_s": 1e-5, "end_time_s": 1e-3},
        "components": components, "controls": controls,
    }))
    .unwrap()
}

#[test]
fn three_nodes_written_out_are_a_bus_when_they_are_its_phases() {
    let m = model(
        json!([
            {"type": "three_phase_source", "name": "G1", "bus": "b1", "line_voltage_rms_v": 6600.0, "frequency_hz": 50.0},
            {"type": "three_phase_load", "name": "LD1", "bus": ["b1.a", "b1.b", "b1.c"], "rated_voltage_kv": 6.6,
             "rated_frequency_hz": 50.0, "active_power_mw": 1.0, "reactive_power_mvar": 0.0, "neutral": "n"},
            {"type": "three_phase_load", "name": "LD2", "bus": ["b1.b", "b1.a", "b1.c"], "rated_voltage_kv": 6.6,
             "rated_frequency_hz": 50.0, "active_power_mw": 1.0, "reactive_power_mvar": 0.0},
            {"type": "resistor", "name": "R1", "nodes": ["n", "gnd"], "resistance_ohm": 1.0},
            {"type": "resistor", "name": "R2", "nodes": ["b1.c", "x.a"], "resistance_ohm": 1.0}
        ]),
        json!([]),
    );
    let t = topology(&m, &[]);
    assert_eq!(t.buses.len(), 1);
    assert_eq!(t.buses[0].components, ["G1", "LD1"]);
    assert_eq!(t.components[1].terminals[0].bus.as_deref(), Some("b1"));
    assert_eq!(t.components[2].terminals[0].bus, None);
    assert_eq!(t.components[2].terminals[0].nodes, ["b1.b", "b1.a", "b1.c"]);
    let node = |name: &str| t.nodes.iter().find(|n| n.name == name).unwrap();
    assert_eq!(node("b1.c").bus.as_deref(), Some("b1"));
    assert_eq!(node("b1.c").phase, Some("c"));
    assert_eq!(node("b1.c").components, ["G1", "LD1", "LD2", "R2"]);
    // "x" is no bus: "x.a" is a node like any other.
    assert_eq!(node("x.a").bus, None);
    assert!(node("gnd").ground && !node("n").ground);
    assert_eq!(node("n").components, ["LD1", "R1"]);
}

#[test]
fn the_blocks_of_a_converter_have_their_owner() {
    let converter = component_catalog()
        .into_iter()
        .find(|c| c.type_name == "grid_following_converter")
        .unwrap()
        .example;
    let name = converter["name"].as_str().unwrap().to_string();
    let m = model(
        json!([converter]),
        json!([{"type": "constant", "name": "q_command", "value": 0.0}]),
    );
    let t = topology(&m, &[]);
    assert_eq!(t.controls[0].owner, None);
    assert_eq!(t.controls[0].path, "controls[0]");
    assert!(t.controls.len() > 30);
    for c in &t.controls[1..] {
        assert_eq!(c.owner.as_deref(), Some(name.as_str()), "{}", c.name);
        assert_eq!(c.path, "components[0]");
        assert!(c.name.starts_with(&format!("{name}.")));
    }
    // Every signal has the block that gives it; the outputs of the blocks
    // are the signals, in their order.
    let outputs: Vec<&String> = t.controls.iter().flat_map(|c| &c.outputs).collect();
    let signals: Vec<&String> = t.signals.iter().map(|s| &s.name).collect();
    assert_eq!(outputs, signals);
    assert!(t.signals.iter().all(|s| s.from.is_some()));
    let all: Vec<Control> = m.controls.clone();
    assert_eq!(all.len(), 1);
}

#[test]
fn a_signal_that_no_block_gives_is_listed_without_a_source() {
    let m = model(
        json!([
            {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {"type": "dc", "value": 1.0}},
            {"type": "controlled_switch", "name": "S1", "nodes": ["n1", "gnd"], "gate": "missing"}
        ]),
        json!([{"type": "gain", "name": "g", "input": "missing", "gain": 2.0}]),
    );
    let t = topology(&m, &[]);
    let s = t.signals.iter().find(|s| s.name == "missing").unwrap();
    assert_eq!(s.from, None);
    assert_eq!(s.to.len(), 2);
    assert_eq!(s.to[0].control.as_deref(), Some("g"));
    assert_eq!(s.to[1].component.as_deref(), Some("S1"));
    assert_eq!(s.to[1].field.as_deref(), Some("gate"));
}
