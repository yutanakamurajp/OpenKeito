//! `topology` against the circuit the solver builds, for every example and
//! for the example of every component type of the catalog.

use std::fs;
use std::path::PathBuf;

use openkeito_cli::{compare_with_netlist, topology_mismatches, topology_report};
use serde_json::{json, Value};

fn examples() -> Vec<(String, String)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let mut found: Vec<(String, String)> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .map(|p| {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            (name, fs::read_to_string(&p).unwrap())
        })
        .collect();
    found.sort();
    found
}

#[test]
fn the_terminals_of_every_example_are_the_nodes_of_the_circuit() {
    let examples = examples();
    assert!(examples.len() >= 21);
    let mut components = 0;
    for (name, text) in &examples {
        let loaded = openkeito_model::load_str(text).unwrap();
        components += loaded.model.components.len();
        assert_eq!(
            topology_mismatches(&loaded.model),
            Vec::<String>::new(),
            "{name}"
        );
    }
    assert!(components > 100, "{components}");
}

/// The comparison is not one that always agrees: a terminal at another
/// node, and two terminals the other way round, are found.
#[test]
fn a_topology_that_differs_from_the_circuit_is_found() {
    let text = fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/rlc.json"),
    )
    .unwrap();
    let model = openkeito_model::load_str(&text).unwrap().model;
    let built = openkeito_core::component_nodes(&model);
    let good = openkeito_model::topology(&model, &[]);
    assert!(compare_with_netlist(&good, &built).is_empty());

    let mut moved = good.clone();
    moved.components[1].terminals[1].nodes = vec!["n3".into()];
    let found = compare_with_netlist(&moved, &built);
    assert!(found.iter().any(|m| m.contains("\"R1\"")), "{found:?}");

    let mut turned = good.clone();
    turned.components[2].terminals.reverse();
    let found = compare_with_netlist(&turned, &built);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].starts_with("U(L1)"), "{found:?}");

    let mut fewer = good;
    fewer.components.pop();
    assert!(!compare_with_netlist(&fewer, &built).is_empty());
}

/// The document of a valid model: every component with its place in the
/// model, the nodes with what is at them, no diagnostics.
#[test]
fn the_document_of_a_model() {
    let text = fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/rlc.json"),
    )
    .unwrap();
    let (document, code) = topology_report(&text);
    assert_eq!(code, 0, "{document}");
    assert_eq!(document["command"], "topology");
    assert_eq!(document["valid"], true);
    assert_eq!(document["errors"], json!([]));
    assert_eq!(
        document["components"][1],
        json!({
            "name": "R1", "type": "resistor", "path": "components[1]",
            "terminals": [
                {"name": "nodes[0]", "path": "components[1].nodes[0]", "nodes": ["n1"]},
                {"name": "nodes[1]", "path": "components[1].nodes[1]", "nodes": ["n2"]},
            ],
        })
    );
    let nodes: Vec<&str> = document["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["name"].as_str().unwrap())
        .collect();
    assert_eq!(nodes, ["n1", "gnd", "n2", "n3"]);
    assert_eq!(
        document["nodes"][1],
        json!({"name": "gnd", "ground": true, "components": ["E1", "C1"]})
    );
    assert_eq!(document["buses"], json!([]));
    assert_eq!(document["controls"], json!([]));
    assert_eq!(document["diagnostics"], json!([]));
    // No path of a file and no time: the same text gives the same document.
    assert_eq!(topology_report(&text).0, document);
    assert!(document["model"].get("path").is_none());
}

fn names(v: &Value) -> Vec<&str> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap())
        .collect()
}

/// Buses, phases and the signals between blocks and components.
#[test]
fn buses_and_signals() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let fault = fs::read_to_string(dir.join("three_phase_fault.json")).unwrap();
    let (document, code) = topology_report(&fault);
    assert_eq!(code, 0);
    let bus = &document["buses"][0];
    assert_eq!(bus["nodes"], json!(["src.a", "src.b", "src.c"]), "{bus}");
    let fault = document["components"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["type"] == "fault")
        .unwrap();
    assert_eq!(fault["terminals"][0]["phases"], json!(["a"]));
    assert_eq!(fault["terminals"][0]["nodes"], json!(["b2.a"]));
    let node = document["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["name"] == "b2.b")
        .unwrap();
    assert_eq!((&node["bus"], &node["phase"]), (&json!("b2"), &json!("b")));

    let buck = fs::read_to_string(dir.join("buck.json")).unwrap();
    let (document, _) = topology_report(&buck);
    let switch = document["components"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["type"] == "controlled_switch")
        .unwrap();
    let gate = switch["signals"][0]["signal"].as_str().unwrap();
    let signal = document["signals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == gate)
        .unwrap();
    assert_eq!(signal["from"], gate);
    assert!(signal["to"]
        .as_array()
        .unwrap()
        .iter()
        .any(|end| end["component"] == switch["name"] && end["field"] == "gate"));
    // Every input of every block is an end of a signal that a block gives.
    for c in document["controls"].as_array().unwrap() {
        for input in c["inputs"].as_array().unwrap() {
            let s = document["signals"]
                .as_array()
                .unwrap()
                .iter()
                .find(|s| s["name"] == input["signal"])
                .unwrap();
            assert!(s["from"].is_string(), "{s}");
        }
    }
}

/// A model with errors still has its connections, and the diagnostics say
/// what they are about; a text that is no model has only the error.
#[test]
fn a_model_that_is_not_valid_has_marks() {
    let text = r#"{"format": "openkeito-model", "version": "0.1",
      "simulation": {"time_step_s": 1e-6, "end_time_s": 1e-3},
      "components": [
        {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {"type": "dc", "value": 1.0}},
        {"type": "resistor", "name": "R1", "nodes": ["n1", "gnd"], "resistance_ohm": -1.0},
        {"type": "capacitor", "name": "C1", "nodes": ["x", "y"], "capacitance_f": 1e-6}
      ],
      "outputs": [{"type": "node_voltage", "node": "x"}]}"#;
    let (document, code) = topology_report(text);
    assert_eq!(code, 2);
    assert_eq!(document["valid"], false);
    assert_eq!(document["components"].as_array().unwrap().len(), 3);
    let marks = document["diagnostics"].as_array().unwrap();
    let errors = document["errors"].as_array().unwrap();
    let warnings = document["warnings"].as_array().unwrap();
    assert_eq!(marks.len(), errors.len() + warnings.len());
    for (mark, d) in marks.iter().zip(errors.iter().chain(warnings)) {
        assert_eq!((&mark["code"], &mark["path"]), (&d["code"], &d["path"]));
    }
    let of = |code: &str| marks.iter().find(|m| m["code"] == code).unwrap();
    assert_eq!(names(&of("E130")["components"]), ["R1"]);
    // The nodes that float, by what the diagnostic says of itself.
    assert_eq!(names(&of("E141")["nodes"]), ["x", "y"]);

    let (document, code) = topology_report("{ not JSON");
    assert_eq!(code, 2);
    assert_eq!(document["errors"][0]["code"], "E001");
    assert!(document.get("components").is_none());
    let batch = r#"{"format": "openkeito-batch", "version": "0.1", "cases": []}"#;
    let (document, code) = topology_report(batch);
    assert_eq!((code, &document["errors"][0]["code"]), (2, &json!("E300")));
}

/// A terminal of a diagnostic: the node of `components[i].nodes[k]`, and
/// the components of a loop of ideal branches.
#[test]
fn marks_name_terminals_and_loops() {
    let text = r#"{"format": "openkeito-model", "version": "0.1",
      "simulation": {"time_step_s": 1e-6, "end_time_s": 1e-3},
      "components": [
        {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {"type": "dc", "value": 1.0}},
        {"type": "voltage_source", "name": "E2", "nodes": ["n1", "gnd"], "waveform": {"type": "dc", "value": 2.0}},
        {"type": "resistor", "name": "R1", "nodes": ["n1", "gnd"], "resistance_ohm": 1.0}
      ]}"#;
    let (document, _) = topology_report(text);
    let marks = document["diagnostics"].as_array().unwrap();
    let e142 = marks.iter().find(|m| m["code"] == "E142").unwrap();
    assert_eq!(names(&e142["components"]), ["E1", "E2"], "{document}");

    let text = text.replace(
        r#""name": "R1", "nodes": ["n1", "gnd"]"#,
        r#""name": "R1", "nodes": ["n1", "n 1"]"#,
    );
    let (document, _) = topology_report(&text);
    let marks = document["diagnostics"].as_array().unwrap();
    let at = marks
        .iter()
        .find(|m| m["path"] == "components[2].nodes[1]")
        .unwrap_or_else(|| panic!("{document}"));
    assert_eq!(names(&at["components"]), ["R1"]);
    assert_eq!(names(&at["nodes"]), ["n 1"]);
}

/// The component types that no example has, and star points at nodes of
/// their own, agree with the circuit too.
#[test]
fn the_types_without_an_example_agree_with_the_circuit() {
    let model = json!({
        "format": "openkeito-model", "version": "0.5",
        "simulation": {"time_step_s": 5e-5, "end_time_s": 1e-3},
        "components": [
            {"type": "three_phase_source", "name": "G1", "bus": "b1", "line_voltage_rms_v": 66000.0, "frequency_hz": 50.0,
             "impedance": {"type": "sequence", "r1_ohm": 0.5, "l1_h": 0.0159, "r0_ohm": 1.0, "l0_h": 0.0477}},
            {"type": "three_phase_ideal_transformer", "name": "PST1", "buses": ["b1", "b2"],
             "rated_voltage_kv": [66.0, 66.0], "phase_shift_deg": 2.0},
            {"type": "three_phase_transformer", "name": "T1", "buses": ["b2", "b3"], "vector_group": "YNyn0",
             "rated_voltage_kv": [66.0, 6.6], "rated_power_mva": 30.0, "rated_frequency_hz": 50.0,
             "impedance_percent": 10.0, "hv_neutral": "nh"},
            {"type": "resistor", "name": "RN", "nodes": ["nh", "gnd"], "resistance_ohm": 10.0},
            {"type": "three_phase_load", "name": "LD1", "bus": "b3", "rated_voltage_kv": 6.6, "rated_frequency_hz": 50.0,
             "active_power_mw": 5.0, "reactive_power_mvar": 1.5, "neutral": "nl"},
            {"type": "resistor", "name": "RL", "nodes": ["nl", "gnd"], "resistance_ohm": 1.0},
            {"type": "three_phase_current_source", "name": "PV1", "bus": ["b3.a", "b3.b", "b3.c"],
             "current_rms_a": 100.0, "frequency_hz": 50.0}
        ]
    });
    let text = model.to_string();
    let loaded = openkeito_model::load_str(&text).unwrap_or_else(|e| panic!("{e:?}"));
    assert_eq!(topology_mismatches(&loaded.model), Vec::<String>::new());
    let (document, code) = topology_report(&text);
    assert_eq!(code, 0);
    let t1 = &document["components"][2];
    let terminals: Vec<(&str, Vec<&str>)> = t1["terminals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| (t["name"].as_str().unwrap(), names(&t["nodes"])))
        .collect();
    assert_eq!(
        terminals,
        [
            ("buses[0]", vec!["b2.a", "b2.b", "b2.c"]),
            ("buses[1]", vec!["b3.a", "b3.b", "b3.c"]),
            ("hv_neutral", vec!["nh"]),
            // The star point that the model leaves at its default.
            ("lv_neutral", vec!["gnd"]),
        ]
    );
    assert_eq!(document["components"][6]["terminals"][0]["bus"], "b3");
}
